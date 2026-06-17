//! Qdrant vector DB store wrapper.
//!
//! T27 scope: connection bootstrap only. `QdrantStore::new` builds a
//! `qdrant_client::Qdrant` client from `QdrantConfig` and `health_check`
//! probes the live server. Collection schema helpers (chunks / graph) are
//! added in T28 and T29.

use crate::config::QdrantConfig;
use crate::error::{Error, Result};
use std::sync::Arc;

/// Wrapper around a `qdrant_client::Qdrant` client.
///
/// `qdrant_client::Qdrant` is not `Clone` in 1.12.1, so the client is held
/// behind an `Arc`. This keeps `QdrantStore` cheaply cloneable so the api and
/// worker crates can share one store across tasks. All collection-level
/// operations are tenant-scoped by convention: collection names are derived
/// from the tenant UUID (see T28 / T29).
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
