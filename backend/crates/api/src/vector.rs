//! Vector store cleanup abstraction for the API (T59).
//!
//! When a document is deleted, its chunk vectors in Qdrant
//! (`chunks_{tenant_id}`) would otherwise be orphaned (the Postgres rows are
//! cascade-deleted but Qdrant has no foreign keys). The delete endpoint
//! depends on the [`VectorCleaner`] trait (injected as
//! `Extension<Arc<dyn VectorCleaner>>`) so tests can substitute a mock that
//! records the cleanup call without a live Qdrant.
//!
//! The real implementation is [`gmrag_core::QdrantStore`] itself, delegating
//! to `delete_chunks_by_document` (a `document_id`-filtered point delete).
//! Graph points are intentionally not cleaned per-document — see the core
//! method docs and the T59 progress notes.

use gmrag_core::QdrantStore;
use uuid::Uuid;

/// Vector-store cleanup used by the document delete endpoint.
#[async_trait::async_trait]
pub trait VectorCleaner: Send + Sync {
    /// Remove all chunk vectors belonging to `document_id` from the
    /// tenant's chunk collection.
    async fn delete_document_chunks(
        &self,
        tenant_id: Uuid,
        document_id: Uuid,
    ) -> Result<(), String>;
}

#[async_trait::async_trait]
impl VectorCleaner for QdrantStore {
    async fn delete_document_chunks(
        &self,
        tenant_id: Uuid,
        document_id: Uuid,
    ) -> Result<(), String> {
        self.delete_chunks_by_document(tenant_id, document_id)
            .await
            .map_err(|e| format!("qdrant chunk cleanup: {e}"))
    }
}
