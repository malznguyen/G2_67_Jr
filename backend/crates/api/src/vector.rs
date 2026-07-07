//! Vector store cleanup abstractions for the API (T59 + Phase 0 TASK-P0-04).
//!
//! When a document is deleted, its chunk vectors in Qdrant
//! (`chunks_{tenant_id}`) would otherwise be orphaned (the Postgres rows are
//! cascade-deleted but Qdrant has no foreign keys). The delete endpoint
//! depends on the [`VectorCleaner`] trait (injected as
//! `Extension<Arc<dyn VectorCleaner>>`) so tests can substitute a mock that
//! records the cleanup call without a live Qdrant.
//!
//! Phase 0 TASK-P0-04 extends this with:
//! - [`VectorCleaner::delete_workspace_chunks`] — workspace-scoped chunk
//!   deletion used by the workspace delete path.
//! - [`GraphCleaner`] — bulk deletion of graph node points in
//!   `graph_{tenant_id}`, used by the document delete path when an orphan
//!   graph node (no remaining provenance) is removed from Postgres.
//!
//! The real implementation is [`gmrag_core::QdrantStore`] itself, delegating
//! to `delete_chunks_by_document` / `delete_chunks_by_workspace` /
//! `delete_graph_nodes`.

use gmrag_core::QdrantStore;
use uuid::Uuid;

/// Vector-store cleanup used by the document + workspace delete endpoints.
#[async_trait::async_trait]
pub trait VectorCleaner: Send + Sync {
    /// Remove all chunk vectors belonging to `document_id` from the
    /// tenant's chunk collection.
    async fn delete_document_chunks(
        &self,
        tenant_id: Uuid,
        document_id: Uuid,
    ) -> Result<(), String>;

    /// Remove all chunk vectors whose `workspace_id` payload matches
    /// `workspace_id` from the tenant's chunk collection (Phase 0
    /// TASK-P0-04 workspace teardown).
    async fn delete_workspace_chunks(
        &self,
        tenant_id: Uuid,
        workspace_id: Uuid,
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

    async fn delete_workspace_chunks(
        &self,
        tenant_id: Uuid,
        workspace_id: Uuid,
    ) -> Result<(), String> {
        self.delete_chunks_by_workspace(tenant_id, workspace_id)
            .await
            .map_err(|e| format!("qdrant workspace chunk cleanup: {e}"))
    }
}

/// Graph-vector cleanup used by the document delete endpoint (Phase 0
/// TASK-P0-04). Removes the Qdrant graph points for the given SQL graph
/// node ids in one bulk request.
#[async_trait::async_trait]
pub trait GraphCleaner: Send + Sync {
    /// Remove the graph node points listed in `node_ids` from the tenant's
    /// `graph_{tenant_id}` collection. An empty slice is a no-op.
    async fn delete_graph_nodes(&self, tenant_id: Uuid, node_ids: &[Uuid]) -> Result<(), String>;
}

#[async_trait::async_trait]
impl GraphCleaner for QdrantStore {
    async fn delete_graph_nodes(&self, tenant_id: Uuid, node_ids: &[Uuid]) -> Result<(), String> {
        self.delete_graph_nodes(tenant_id, node_ids)
            .await
            .map_err(|e| format!("qdrant graph node cleanup: {e}"))
    }
}
