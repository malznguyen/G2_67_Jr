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
    Condition, CreateCollectionBuilder, CreateFieldIndexCollectionBuilder, DeleteCollectionBuilder,
    Distance, FieldType, Filter, PointStruct, ScoredPoint, SearchPointsBuilder,
    UpsertPointsBuilder, VectorParamsBuilder,
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

    /// kNN search over the tenant-scoped `chunks_{tenant_id}` collection.
    ///
    /// Returns the `top_k` closest points to `query_vector` (cosine
    /// similarity, per the T28 vector config), optionally restricted to
    /// points matching `filter`. Results are sorted by score descending
    /// (Qdrant default). Payload is included in every `ScoredPoint` so the
    /// caller can read `workspace_id`, `document_id`, `chunk_index`, etc.
    /// without a follow-up `get_points` call.
    ///
    /// The `filter` is `Option<Filter>` — pass `None` for an unfiltered
    /// kNN, or `Some(Filter::must([Condition::matches("workspace_id",
    /// ws.to_string())]))` to scope the search to a workspace within the
    /// tenant. Per the multi-tenant invariant, the `tenant_id` IS the
    /// collection boundary (hard isolation), while `workspace_id` is a
    /// softer payload-level partition inside the tenant.
    pub async fn search_chunks(
        &self,
        tenant_id: Uuid,
        query_vector: Vec<f32>,
        filter: Option<Filter>,
        top_k: u64,
    ) -> Result<Vec<ScoredPoint>> {
        let name = format!("chunks_{tenant_id}");
        let mut req = SearchPointsBuilder::new(name, query_vector, top_k).with_payload(true);
        if let Some(f) = filter {
            req = req.filter(f);
        }
        let resp = self
            .client
            .search_points(req)
            .await
            .map_err(Error::from)?;
        Ok(resp.result)
    }

    /// Upsert graph nodes into the tenant-scoped `graph_{tenant_id}`
    /// collection.
    ///
    /// Each point is expected to carry a 768-dim vector (matching
    /// [`EMBED_DIM`]) and a payload conforming to the T29 schema
    /// (`node_id`, `workspace_id`, `entity_name`). The caller is
    /// responsible for embedding the entity description into the vector
    /// and building the payload; this method only transports them to
    /// Qdrant.
    ///
    /// `.wait(true)` is set so the upsert is acknowledged (and indexed)
    /// before the call returns — a subsequent `search_graph_nodes` will
    /// see the new nodes.
    ///
    /// Mirrors [`upsert_chunks`] (T31) but targets `graph_{tenant_id}`.
    pub async fn upsert_graph_nodes(
        &self,
        tenant_id: Uuid,
        points: Vec<PointStruct>,
    ) -> Result<()> {
        let name = format!("graph_{tenant_id}");
        self.client
            .upsert_points(UpsertPointsBuilder::new(name, points).wait(true))
            .await
            .map_err(Error::from)?;
        Ok(())
    }

    /// kNN search over the tenant-scoped `graph_{tenant_id}` collection,
    /// scoped to a single `workspace_id` within the tenant.
    ///
    /// Unlike [`search_chunks`] (T32, which takes an `Option<Filter>` so
    /// the caller can compose arbitrary filters), `search_graph_nodes`
    /// takes `workspace_id` directly and builds the `workspace_id` filter
    /// internally. Per the T33 spec, graph search is always
    /// workspace-scoped inside a tenant — there is no use case for
    /// cross-workspace graph queries.
    ///
    /// Returns the `top_k` closest nodes to `query_vector` (cosine
    /// similarity, per the T29 vector config), sorted by score descending.
    /// Payload (`node_id`, `workspace_id`, `entity_name`) is included in
    /// every `ScoredPoint` so the caller can join back to the
    /// `graph_nodes` table in PostgreSQL (T21 migration) via `node_id`.
    pub async fn search_graph_nodes(
        &self,
        tenant_id: Uuid,
        workspace_id: Uuid,
        query_vector: Vec<f32>,
        top_k: u64,
    ) -> Result<Vec<ScoredPoint>> {
        let name = format!("graph_{tenant_id}");
        let filter =
            Filter::must([Condition::matches("workspace_id", workspace_id.to_string())]);
        let resp = self
            .client
            .search_points(
                SearchPointsBuilder::new(name, query_vector, top_k)
                    .filter(filter)
                    .with_payload(true),
            )
            .await
            .map_err(Error::from)?;
        Ok(resp.result)
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
    use qdrant_client::qdrant::{
        vectors_config::Config, Condition, CountPointsBuilder, Distance, Filter, PayloadSchemaType,
        PointStruct,
    };
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

    /// T32 — `search_chunks` returns kNN results filtered by payload
    /// (`workspace_id`). Inserts 3 chunks across 2 workspaces with known
    /// orthogonal vectors, then searches with a `workspace_id` filter and
    /// asserts: (1) only the filtered workspace's points are returned,
    /// (2) the top hit is the exact-match vector (recall), (3) results are
    /// sorted by score descending.
    #[tokio::test]
    async fn search_chunks_returns_filtered_knn_results() {
        let store = local_store().await;
        let tenant_id = Uuid::new_v4();

        // Provision via T30 pub API.
        store
            .setup_tenant_collections(tenant_id)
            .await
            .expect("setup_tenant_collections must provision chunks collection");

        // Two workspaces; doc + owner shared for simplicity.
        let ws_a = Uuid::new_v4();
        let ws_b = Uuid::new_v4();
        let doc = Uuid::new_v4();
        let owner = Uuid::new_v4();

        let make_point = |id: u64, ws: Uuid, idx: i64, vec: Vec<f32>| {
            PointStruct::new(
                id,
                vec,
                Payload::try_from(serde_json::json!({
                    "workspace_id": ws.to_string(),
                    "document_id": doc.to_string(),
                    "chunk_index": idx,
                    "filename": "a.txt",
                    "owner_id": owner.to_string(),
                    "visibility": "private",
                }))
                .expect("payload must build from json"),
            )
        };

        // chunk 1: ws_a, vec A (unit_vec(0)) — should be the top hit for query A.
        // chunk 2: ws_a, vec B (unit_vec(1)) — orthogonal to A, same workspace.
        // chunk 3: ws_b, vec A (unit_vec(0)) — same vec as chunk 1 but DIFFERENT
        //          workspace; must be filtered OUT by the workspace_id filter.
        let points = vec![
            make_point(1, ws_a, 0, unit_vec(0)),
            make_point(2, ws_a, 1, unit_vec(1)),
            make_point(3, ws_b, 0, unit_vec(0)),
        ];
        store
            .upsert_chunks(tenant_id, points)
            .await
            .expect("upsert_chunks must seed search corpus");

        // Search with workspace_id = ws_a filter, query vector = A (unit_vec(0)).
        let ws_a_str = ws_a.to_string();
        let filter = Filter::must([Condition::matches("workspace_id", ws_a_str.clone())]);
        let results = store
            .search_chunks(tenant_id, unit_vec(0), Some(filter), 3)
            .await
            .expect("search_chunks must succeed");

        // (1) Only ws_a points are returned — chunk 3 (ws_b) is filtered out.
        assert!(
            !results.is_empty(),
            "search_chunks must return at least one result"
        );
        for point in &results {
            let got = point.get("workspace_id").as_str();
            assert_eq!(
                got,
                Some(&ws_a_str),
                "filtered search must only return ws_a points, got workspace_id = {got:?}"
            );
        }
        // chunk 1 + chunk 2 are in ws_a → 2 results.
        assert_eq!(
            results.len(),
            2,
            "expected 2 ws_a points (chunks 1 and 2), got {} results",
            results.len()
        );

        // (2) Recall: top hit is chunk 1 (vec A == query A, cosine = 1.0).
        let top = &results[0];
        let top_idx = top.get("chunk_index").as_integer();
        assert_eq!(
            top_idx,
            Some(0),
            "top result must be chunk 1 (chunk_index 0, exact vector match), got chunk_index = {top_idx:?}"
        );

        // (3) Results sorted by score descending.
        assert!(
            results[0].score >= results[1].score,
            "results must be sorted by score descending; got [0].score={} > [1].score={}",
            results[0].score,
            results[1].score
        );

        // Sanity: unfiltered search returns all 3 (proves filter is what
        // removed chunk 3, not a missing point).
        let all = store
            .search_chunks(tenant_id, unit_vec(0), None, 5)
            .await
            .expect("unfiltered search_chunks must succeed");
        assert_eq!(
            all.len(),
            3,
            "unfiltered search must return all 3 points, got {}",
            all.len()
        );

        // Cleanup.
        store
            .teardown_tenant_collections(tenant_id)
            .await
            .expect("teardown_tenant_collections must clean up");
    }

    /// T33 — `upsert_graph_nodes` + `search_graph_nodes` on
    /// `graph_{tenant_id}`. Inserts 3 graph nodes across 2 workspaces with
    /// orthogonal vectors, then searches scoped to a workspace and
    /// asserts: (1) only the filtered workspace's nodes are returned,
    /// (2) the top hit is the exact-match vector (recall), (3) results
    /// sorted by score descending.
    ///
    /// Unlike `search_chunks` (T32, caller-supplied `Option<Filter>`),
    /// `search_graph_nodes` takes `workspace_id` directly and builds the
    /// filter internally — per the T33 spec, graph search is always
    /// workspace-scoped inside a tenant.
    #[tokio::test]
    async fn upsert_and_search_graph_nodes_filters_by_workspace() {
        let store = local_store().await;
        let tenant_id = Uuid::new_v4();

        // Provision via T30 pub API (creates both chunks_* and graph_*).
        store
            .setup_tenant_collections(tenant_id)
            .await
            .expect("setup_tenant_collections must provision graph collection");

        let ws_a = Uuid::new_v4();
        let ws_b = Uuid::new_v4();

        let make_node = |id: u64, node_id: Uuid, ws: Uuid, entity: &str, vec: Vec<f32>| {
            PointStruct::new(
                id,
                vec,
                Payload::try_from(serde_json::json!({
                    "node_id": node_id.to_string(),
                    "workspace_id": ws.to_string(),
                    "entity_name": entity,
                }))
                .expect("graph node payload must build from json"),
            )
        };

        // node 1: ws_a, vec A (unit_vec(0)), entity "Person" — top hit for query A.
        // node 2: ws_a, vec B (unit_vec(1)), entity "Org" — orthogonal to A, same ws.
        // node 3: ws_b, vec A (unit_vec(0)), entity "Person" — same vec as node 1
        //         but DIFFERENT workspace; must be filtered OUT.
        let nodes = vec![
            make_node(1, Uuid::new_v4(), ws_a, "Person", unit_vec(0)),
            make_node(2, Uuid::new_v4(), ws_a, "Org", unit_vec(1)),
            make_node(3, Uuid::new_v4(), ws_b, "Person", unit_vec(0)),
        ];
        store
            .upsert_graph_nodes(tenant_id, nodes)
            .await
            .expect("upsert_graph_nodes must seed graph corpus");

        // Search scoped to ws_a with query vector A.
        let results = store
            .search_graph_nodes(tenant_id, ws_a, unit_vec(0), 3)
            .await
            .expect("search_graph_nodes must succeed");

        // (1) Only ws_a nodes returned — node 3 (ws_b) filtered out.
        assert!(
            !results.is_empty(),
            "search_graph_nodes must return at least one result"
        );
        let ws_a_str = ws_a.to_string();
        for point in &results {
            let got = point.get("workspace_id").as_str();
            assert_eq!(
                got,
                Some(&ws_a_str),
                "graph search must only return ws_a nodes, got workspace_id = {got:?}"
            );
        }
        assert_eq!(
            results.len(),
            2,
            "expected 2 ws_a nodes (1 and 2), got {} results",
            results.len()
        );

        // (2) Recall: top hit is node 1 (vec A == query A, cosine = 1.0).
        // Distinguish node 1 from node 2 via entity_name.
        let top = &results[0];
        let top_entity = top.get("entity_name").as_str();
        assert_eq!(
            top_entity.map(|s| s.as_str()),
            Some("Person"),
            "top result must be node 1 (entity 'Person', exact vector match), got entity_name = {top_entity:?}"
        );

        // (3) Results sorted by score descending.
        assert!(
            results[0].score >= results[1].score,
            "graph results must be sorted by score descending; got [0].score={} >= [1].score={}",
            results[0].score,
            results[1].score
        );

        // Cleanup.
        store
            .teardown_tenant_collections(tenant_id)
            .await
            .expect("teardown_tenant_collections must clean up");
    }
}
