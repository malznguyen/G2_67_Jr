//! Qdrant vector DB store wrapper.
//!
//! T27: connection bootstrap (`new` + `health_check`).
//! T28: chunks collection schema (`create_chunks_collection`) plus shared
//!      `delete_collection` / `list_collection_names` helpers.
//! T29: graph collection schema (`create_graph_collection`).

use crate::config::QdrantConfig;
use crate::error::{Error, Result};
use std::sync::Arc;

use qdrant_client::qdrant::{
    CreateCollectionBuilder, CreateFieldIndexCollectionBuilder, DeleteCollectionBuilder, Distance,
    FieldType, PointStruct, UpsertPointsBuilder, VectorParamsBuilder,
};
use uuid::Uuid;

/// Vector dimension used by the project's embedding model (Ollama
/// `nomic-embed-text`). All collections (chunks + graph) share this size.
const EMBED_DIM: u64 = 768;

/// Wrapper around a `qdrant_client::Qdrant` client.
///
/// `qdrant_client::Qdrant` is not `Clone` in 1.12.1, so the client is held
/// behind an `Arc`. This keeps `QdrantStore` cheaply cloneable so the api and
/// worker crates can share one store across tasks. All collection-level
/// operations are tenant-scoped by convention: collection names are derived
/// from the tenant UUID (see `create_chunks_collection` / T29).
#[derive(Clone)]
pub struct QdrantStore {
    client: Arc<qdrant_client::Qdrant>,
}

impl QdrantStore {
    /// Build a `QdrantStore` from config and confirm the server is reachable.
    ///
    /// Mirrors `init_pool`'s fail-fast philosophy: a liveness probe is run
    /// so callers see a clean error early when Qdrant is down or the URL is
    /// wrong, instead of an opaque gRPC timeout on the first real op.
    pub async fn new(cfg: &QdrantConfig) -> Result<Self> {
        let mut builder = qdrant_client::Qdrant::from_url(&cfg.url);
        if let Some(key) = &cfg.api_key {
            builder = builder.api_key(key.clone());
        }
        let client = builder.build()?;
        let store = Self { client: Arc::new(client) };
        store.health_check().await?;
        Ok(store)
    }

    /// Liveness probe — wraps `qdrant_client::Qdrant::health_check`.
    pub async fn health_check(&self) -> Result<()> {
        self.client.health_check().await.map_err(Error::from).map(|_| ())
    }

    /// List the names of all collections on the server.
    pub async fn list_collection_names(&self) -> Result<Vec<String>> {
        let resp = self.client.list_collections().await?;
        Ok(resp.collections.into_iter().map(|c| c.name).collect())
    }

    /// Delete a collection by name. Idempotent: a missing collection is
    /// treated as success at the call site (the server returns an error for
    /// nonexistent collections, so callers that want "ensure gone" semantics
    /// should ignore the `Err` — see the test cleanup pattern).
    pub async fn delete_collection(&self, name: &str) -> Result<()> {
        self.client
            .delete_collection(DeleteCollectionBuilder::new(name))
            .await
            .map_err(Error::from)
            .map(|_| ())
    }

    /// Create the tenant-scoped chunks collection (idempotent).
    ///
    /// Collection name: `chunks_{tenant_id}`. Vector config: single vector,
    /// size [`EMBED_DIM`] (768), `Distance::Cosine`, Qdrant-default HNSW.
    /// Payload indexes (per spec):
    /// - `workspace_id` — Uuid
    /// - `document_id`  — Uuid
    /// - `chunk_index`  — Integer
    /// - `filename`     — Keyword
    /// - `owner_id`     — Uuid
    /// - `visibility`   — Keyword
    ///
    /// Idempotent (T30): `list_collection_names` is checked first and the
    /// collection is only created when missing. This avoids the
    /// "collection already exists" error that `create_collection` returns
    /// when called twice with the same name — see the T28/T29 blocker notes.
    ///
    /// Index creation uses `.wait(true)` so the index is materialized before
    /// the call returns (avoids a race where an immediate upsert lands
    /// before the index exists).
    async fn create_chunks_collection(&self, tenant_id: Uuid) -> Result<()> {
        let name = format!("chunks_{tenant_id}");
        if self.collection_exists(&name).await? {
            return Ok(());
        }
        self.create_collection_with_indexes(&name, &[
            ("workspace_id", FieldType::Uuid),
            ("document_id", FieldType::Uuid),
            ("chunk_index", FieldType::Integer),
            ("filename", FieldType::Keyword),
            ("owner_id", FieldType::Uuid),
            ("visibility", FieldType::Keyword),
        ])
        .await
    }

    /// Create the tenant-scoped graph collection (idempotent).
    ///
    /// Collection name: `graph_{tenant_id}`. Vector config: single vector,
    /// size [`EMBED_DIM`] (768), `Distance::Cosine`, Qdrant-default HNSW.
    /// Payload indexes (per spec):
    /// - `node_id`      — Uuid
    /// - `workspace_id` — Uuid
    /// - `entity_name`  — Keyword
    ///
    /// Idempotent (T30): same guard as [`create_chunks_collection`].
    async fn create_graph_collection(&self, tenant_id: Uuid) -> Result<()> {
        let name = format!("graph_{tenant_id}");
        if self.collection_exists(&name).await? {
            return Ok(());
        }
        self.create_collection_with_indexes(&name, &[
            ("node_id", FieldType::Uuid),
            ("workspace_id", FieldType::Uuid),
            ("entity_name", FieldType::Keyword),
        ])
        .await
    }

    /// Returns `true` if a collection with the given name exists on the
    /// server. Used by the idempotent `create_*` helpers (T30).
    async fn collection_exists(&self, name: &str) -> Result<bool> {
        Ok(self.list_collection_names().await?.contains(&name.to_string()))
    }

    /// Provision both tenant-scoped collections (`chunks_{tenant_id}` and
    /// `graph_{tenant_id}`) in one call. Idempotent: safe to call multiple
    /// times for the same tenant — existing collections are left untouched.
    ///
    /// This is the entry point for tenant provisioning (worker / tenant
    /// bootstrap). The underlying `create_chunks_collection` /
    /// `create_graph_collection` helpers check `list_collection_names`
    /// before issuing `create_collection`, so a repeated `setup` does not
    /// raise the Qdrant "collection already exists" error.
    pub async fn setup_tenant_collections(&self, tenant_id: Uuid) -> Result<()> {
        self.create_chunks_collection(tenant_id).await?;
        self.create_graph_collection(tenant_id).await?;
        Ok(())
    }

    /// Delete both tenant-scoped collections (`chunks_{tenant_id}` and
    /// `graph_{tenant_id}`). Idempotent: missing collections are treated as
    /// success (the server returns an error for nonexistent collections,
    /// so `delete_collection`'s `Err` is only surfaced when the name is
    /// present but deletion fails for another reason — handled by first
    /// checking `collection_exists`).
    ///
    /// This is the entry point for tenant teardown (tenant offboarding /
    /// test cleanup).
    pub async fn teardown_tenant_collections(&self, tenant_id: Uuid) -> Result<()> {
        let chunks = format!("chunks_{tenant_id}");
        let graph = format!("graph_{tenant_id}");
        let names = self.list_collection_names().await?;
        if names.contains(&chunks) {
            self.delete_collection(&chunks).await?;
        }
        if names.contains(&graph) {
            self.delete_collection(&graph).await?;
        }
        Ok(())
    }

    /// Upsert chunk points into the tenant-scoped `chunks_{tenant_id}`
    /// collection.
    ///
    /// Each point is expected to carry a 768-dim vector (matching
    /// [`EMBED_DIM`]) and a payload conforming to the T28 schema
    /// (`workspace_id`, `document_id`, `chunk_index`, `filename`,
    /// `owner_id`, `visibility`). The caller is responsible for embedding
    /// the chunk text into the vector and building the payload; this
    /// method only transports them to Qdrant.
    ///
    /// `.wait(true)` is set so the upsert is acknowledged (and indexed)
    /// before the call returns — a subsequent `search_chunks` will see the
    /// new points. For bulk ingestion of very large batches, consider
    /// splitting the points upstream and calling this method per chunk to
    /// avoid gRPC timeouts (Qdrant single-request limit).
    pub async fn upsert_chunks(
        &self,
        tenant_id: Uuid,
        points: Vec<PointStruct>,
    ) -> Result<()> {
        let name = format!("chunks_{tenant_id}");
        self.client
            .upsert_points(UpsertPointsBuilder::new(name, points).wait(true))
            .await
            .map_err(Error::from)?;
        Ok(())
    }

    /// Shared helper: create a 768-dim Cosine collection and attach a set of
    /// payload field indexes. Used by `create_chunks_collection` (T28) and
    /// `create_graph_collection` (T29).
    async fn create_collection_with_indexes(
        &self,
        name: &str,
        indexes: &[(&str, FieldType)],
    ) -> Result<()> {
        self.client
            .create_collection(
                CreateCollectionBuilder::new(name)
                    .vectors_config(VectorParamsBuilder::new(EMBED_DIM, Distance::Cosine)),
            )
            .await
            .map_err(Error::from)?;

        for (field, ftype) in indexes {
            self.client
                .create_field_index(
                    CreateFieldIndexCollectionBuilder::new(name, *field, *ftype).wait(true),
                )
                .await
                .map_err(Error::from)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qdrant_client::qdrant::{vectors_config::Config, CountPointsBuilder, Distance, PayloadSchemaType, PointStruct};
    use qdrant_client::Payload;
    use uuid::Uuid;

    /// Qdrant must be reachable for integration tests.
    ///
    /// `qdrant-client` uses the **gRPC** endpoint (port 6334), not the REST
    /// endpoint (port 6333). Both ports are exposed by the dev Qdrant
    /// container. Tests construct the config directly (no env) so they are
    /// deterministic and isolated from the operator's `.env`. If Qdrant is
    /// not running the test fails loudly with a connection error — this is
    /// intentional: the spec requires Qdrant local to pass.
    fn local_config() -> QdrantConfig {
        QdrantConfig {
            url: "http://localhost:6334".into(),
            api_key: None,
            collection_default: "gmrag_chunks".into(),
        }
    }

    async fn local_store() -> QdrantStore {
        QdrantStore::new(&local_config())
            .await
            .expect("Qdrant gRPC at localhost:6334 must be reachable for integration tests")
    }

    #[tokio::test]
    async fn qdrant_store_new_connects_to_local() {
        // Skip gracefully in CI without Qdrant — but surface the reason.
        let cfg = local_config();
        let store = QdrantStore::new(&cfg)
            .await
            .expect("QdrantStore::new must connect to Qdrant gRPC at localhost:6334");
        store
            .health_check()
            .await
            .expect("health_check must succeed on live Qdrant");
    }

    /// Read the payload index type for a field, or `None` if not indexed.
    fn payload_index_type(
        info: &qdrant_client::qdrant::CollectionInfo,
        field: &str,
    ) -> Option<PayloadSchemaType> {
        info.payload_schema.get(field).map(|v| v.data_type())
    }

    #[tokio::test]
    async fn create_chunks_collection_creates_with_payload_indexes() {
        let store = local_store().await;
        let tenant_id = Uuid::new_v4();
        let name = format!("chunks_{tenant_id}");

        // Cleanup any stale collection from a prior aborted run.
        let _ = store.delete_collection(&name).await;

        // Create.
        store
            .create_chunks_collection(tenant_id)
            .await
            .expect("create_chunks_collection must succeed on live Qdrant");

        // Verify the collection appears in the listing.
        let names = store
            .list_collection_names()
            .await
            .expect("list_collection_names must succeed");
        assert!(
            names.contains(&name),
            "collection '{name}' must be listed, got {names:?}"
        );

        // Verify payload indexes + vector config via collection_info.
        let resp = store
            .client
            .collection_info(&name)
            .await
            .expect("collection_info must succeed for a real collection");
        let info = resp
            .result
            .as_ref()
            .expect("collection_info result must be present");

        // 6 payload indexes with the exact schema from the spec.
        assert_eq!(
            payload_index_type(info, "workspace_id"),
            Some(PayloadSchemaType::Uuid),
            "workspace_id must be indexed as Uuid"
        );
        assert_eq!(
            payload_index_type(info, "document_id"),
            Some(PayloadSchemaType::Uuid),
            "document_id must be indexed as Uuid"
        );
        assert_eq!(
            payload_index_type(info, "chunk_index"),
            Some(PayloadSchemaType::Integer),
            "chunk_index must be indexed as Integer"
        );
        assert_eq!(
            payload_index_type(info, "filename"),
            Some(PayloadSchemaType::Keyword),
            "filename must be indexed as Keyword"
        );
        assert_eq!(
            payload_index_type(info, "owner_id"),
            Some(PayloadSchemaType::Uuid),
            "owner_id must be indexed as Uuid"
        );
        assert_eq!(
            payload_index_type(info, "visibility"),
            Some(PayloadSchemaType::Keyword),
            "visibility must be indexed as Keyword"
        );

        // Vector config: single vector, size 768, Cosine.
        let cfg = info
            .config
            .as_ref()
            .expect("CollectionConfig must be present");
        let params = cfg.params.as_ref().expect("CollectionParams must be present");
        let vc = params
            .vectors_config
            .as_ref()
            .expect("VectorsConfig must be present");
        let inner = vc.config.as_ref().expect("vectors_config::Config must be present");
        match inner {
            Config::Params(vp) => {
                assert_eq!(vp.size, 768, "vector size must be 768");
                assert_eq!(
                    vp.distance(),
                    Distance::Cosine,
                    "distance must be Cosine"
                );
            }
            Config::ParamsMap(_) => panic!("expected single (unnamed) vector config, got ParamsMap"),
        }

        // Cleanup.
        store
            .delete_collection(&name)
            .await
            .expect("delete_collection must succeed");
        let names_after = store
            .list_collection_names()
            .await
            .expect("list_collection_names must succeed after delete");
        assert!(
            !names_after.contains(&name),
            "collection '{name}' must be gone after delete, got {names_after:?}"
        );
    }

    #[tokio::test]
    async fn create_graph_collection_creates_with_payload_indexes() {
        let store = local_store().await;
        let tenant_id = Uuid::new_v4();
        let name = format!("graph_{tenant_id}");

        // Cleanup any stale collection from a prior aborted run.
        let _ = store.delete_collection(&name).await;

        // Create.
        store
            .create_graph_collection(tenant_id)
            .await
            .expect("create_graph_collection must succeed on live Qdrant");

        // Verify the collection appears in the listing.
        let names = store
            .list_collection_names()
            .await
            .expect("list_collection_names must succeed");
        assert!(
            names.contains(&name),
            "collection '{name}' must be listed, got {names:?}"
        );

        // Verify payload indexes + vector config via collection_info.
        let resp = store
            .client
            .collection_info(&name)
            .await
            .expect("collection_info must succeed for a real collection");
        let info = resp
            .result
            .as_ref()
            .expect("collection_info result must be present");

        // 3 payload indexes with the exact schema from the spec.
        assert_eq!(
            payload_index_type(info, "node_id"),
            Some(PayloadSchemaType::Uuid),
            "node_id must be indexed as Uuid"
        );
        assert_eq!(
            payload_index_type(info, "workspace_id"),
            Some(PayloadSchemaType::Uuid),
            "workspace_id must be indexed as Uuid"
        );
        assert_eq!(
            payload_index_type(info, "entity_name"),
            Some(PayloadSchemaType::Keyword),
            "entity_name must be indexed as Keyword"
        );

        // Vector config: single vector, size 768, Cosine.
        let cfg = info
            .config
            .as_ref()
            .expect("CollectionConfig must be present");
        let params = cfg.params.as_ref().expect("CollectionParams must be present");
        let vc = params
            .vectors_config
            .as_ref()
            .expect("VectorsConfig must be present");
        let inner = vc.config.as_ref().expect("vectors_config::Config must be present");
        match inner {
            Config::Params(vp) => {
                assert_eq!(vp.size, 768, "vector size must be 768");
                assert_eq!(vp.distance(), Distance::Cosine, "distance must be Cosine");
            }
            Config::ParamsMap(_) => panic!("expected single (unnamed) vector config, got ParamsMap"),
        }

        // Cleanup.
        store
            .delete_collection(&name)
            .await
            .expect("delete_collection must succeed");
        let names_after = store
            .list_collection_names()
            .await
            .expect("list_collection_names must succeed after delete");
        assert!(
            !names_after.contains(&name),
            "collection '{name}' must be gone after delete, got {names_after:?}"
        );
    }

    /// T30 — `setup_tenant_collections` must be idempotent (calling it twice
    /// for the same tenant does not error) and `teardown_tenant_collections`
    /// must remove both `chunks_{tenant_id}` and `graph_{tenant_id}`.
    ///
    /// This is the core blocker fix from T28/T29: the raw `create_*`
    /// helpers raised "collection already exists" on the second call. The
    /// `setup` wrapper guards with `list_collection_names` first.
    #[tokio::test]
    async fn setup_tenant_collections_is_idempotent() {
        let store = local_store().await;
        let tenant_id = Uuid::new_v4();
        let chunks_name = format!("chunks_{tenant_id}");
        let graph_name = format!("graph_{tenant_id}");

        // Cleanup any stale collections from a prior aborted run.
        let _ = store.teardown_tenant_collections(tenant_id).await;

        // First setup: both collections must be created.
        store
            .setup_tenant_collections(tenant_id)
            .await
            .expect("first setup_tenant_collections must succeed");
        let names_after_first = store
            .list_collection_names()
            .await
            .expect("list_collection_names must succeed after first setup");
        assert!(
            names_after_first.contains(&chunks_name),
            "chunks collection '{chunks_name}' must exist after setup, got {names_after_first:?}"
        );
        assert!(
            names_after_first.contains(&graph_name),
            "graph collection '{graph_name}' must exist after setup, got {names_after_first:?}"
        );

        // Second setup: MUST NOT error — this is the idempotency assertion
        // that failed before T30 (raw create_collection raised on duplicate).
        store
            .setup_tenant_collections(tenant_id)
            .await
            .expect("second setup_tenant_collections must be idempotent (no error)");
        let names_after_second = store
            .list_collection_names()
            .await
            .expect("list_collection_names must succeed after second setup");
        assert!(
            names_after_second.contains(&chunks_name)
                && names_after_second.contains(&graph_name),
            "both collections must still exist after idempotent second setup, got {names_after_second:?}"
        );

        // Teardown: both collections must be gone.
        store
            .teardown_tenant_collections(tenant_id)
            .await
            .expect("teardown_tenant_collections must succeed");
        let names_after_teardown = store
            .list_collection_names()
            .await
            .expect("list_collection_names must succeed after teardown");
        assert!(
            !names_after_teardown.contains(&chunks_name),
            "chunks collection must be gone after teardown, got {names_after_teardown:?}"
        );
        assert!(
            !names_after_teardown.contains(&graph_name),
            "graph collection must be gone after teardown, got {names_after_teardown:?}"
        );

        // Second teardown: MUST NOT error — idempotent on missing collections.
        store
            .teardown_tenant_collections(tenant_id)
            .await
            .expect("second teardown_tenant_collections must be idempotent (no error)");
    }

    /// Build a 768-dim unit vector with `1.0` at `pos` and `0.0` elsewhere.
    /// Used by upsert/search tests to craft orthogonal query vectors with
    /// deterministic cosine similarity (1.0 for same pos, 0.0 for diff).
    fn unit_vec(pos: usize) -> Vec<f32> {
        let mut v = vec![0.0f32; EMBED_DIM as usize];
        v[pos] = 1.0;
        v
    }

    /// T31 — `upsert_chunks` inserts points (768-dim vectors + full payload
    /// schema) into `chunks_{tenant_id}`. Verified via `count` after upsert.
    #[tokio::test]
    async fn upsert_chunks_inserts_points_into_chunks_collection() {
        let store = local_store().await;
        let tenant_id = Uuid::new_v4();
        let chunks_name = format!("chunks_{tenant_id}");

        // Provision via T30 pub API (idempotent setup).
        store
            .setup_tenant_collections(tenant_id)
            .await
            .expect("setup_tenant_collections must provision chunks collection");

        // Build 3 mock chunk points with the full T28 payload schema.
        let ws = Uuid::new_v4();
        let doc = Uuid::new_v4();
        let owner = Uuid::new_v4();
        let make_point = |id: u64, idx: i64, filename: &str, vis: &str, vec: Vec<f32>| {
            PointStruct::new(
                id,
                vec,
                Payload::try_from(serde_json::json!({
                    "workspace_id": ws.to_string(),
                    "document_id": doc.to_string(),
                    "chunk_index": idx,
                    "filename": filename,
                    "owner_id": owner.to_string(),
                    "visibility": vis,
                }))
                .expect("payload must build from json"),
            )
        };
        let points = vec![
            make_point(1, 0, "a.txt", "private", unit_vec(0)),
            make_point(2, 1, "a.txt", "private", unit_vec(1)),
            make_point(3, 0, "b.txt", "public", unit_vec(2)),
        ];

        store
            .upsert_chunks(tenant_id, points)
            .await
            .expect("upsert_chunks must succeed on live Qdrant");

        // Verify all 3 points landed via exact count.
        let resp = store
            .client
            .count(CountPointsBuilder::new(&chunks_name).exact(true))
            .await
            .expect("count must succeed after upsert");
        let count = resp
            .result
            .as_ref()
            .expect("count result must be present")
            .count;
        assert_eq!(
            count, 3,
            "exactly 3 points must be present in '{chunks_name}' after upsert, got {count}"
        );

        // Cleanup.
        store
            .teardown_tenant_collections(tenant_id)
            .await
            .expect("teardown_tenant_collections must clean up");
    }
}
