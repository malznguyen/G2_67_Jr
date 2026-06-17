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
    FieldType, VectorParamsBuilder,
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

    /// Create the tenant-scoped chunks collection.
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
    /// Index creation uses `.wait(true)` so the index is materialized before
    /// the call returns (avoids a race where an immediate upsert lands
    /// before the index exists).
    pub async fn create_chunks_collection(&self, tenant_id: Uuid) -> Result<()> {
        let name = format!("chunks_{tenant_id}");
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
    use qdrant_client::qdrant::{vectors_config::Config, Distance, PayloadSchemaType};
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
}
