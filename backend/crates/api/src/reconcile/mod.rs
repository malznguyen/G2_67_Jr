//! Phase 3 — cross-system drift reconciler (Postgres ↔ OpenFGA ↔ Qdrant).
//!
//! Postgres is the source of truth. The reconcilers compare it against
//! OpenFGA and Qdrant, report drift, and — only when `auto_fix` is explicitly
//! enabled — repair it. Default mode is **dry-run / report-only**. The worker
//! runs both reconcilers on a periodic background loop; the `reconcile-drift`
//! ops binary runs them standalone (default dry-run, `--fix` to enable).
//!
//! See [`openfga`] for why orphan detection is resource-existence-based, not
//! a set-difference (dynamic ACL grants live only in OpenFGA).

pub mod backfill;
pub mod openfga;
pub mod qdrant;

pub use openfga::run_openfga_reconcile;
pub use qdrant::run_qdrant_reconcile;

use gmrag_core::QdrantStore;
use serde::Serialize;
use sqlx::PgPool;

use crate::authz::AuthorizationService;

/// Combined summary of one full reconciliation pass (both subsystems).
#[derive(Debug, Clone, Serialize)]
pub struct ReconcileSummary {
    pub openfga: openfga::OpenFgaReport,
    pub qdrant: qdrant::QdrantReport,
    pub auto_fix: bool,
}

/// Run both reconcilers once. `auto_fix = false` (the default everywhere) →
/// report-only; neither OpenFGA nor Qdrant is written to or deleted from.
pub async fn run_reconcile_once(
    pool: &PgPool,
    qdrant: &QdrantStore,
    authz: &dyn AuthorizationService,
    auto_fix: bool,
) -> anyhow::Result<ReconcileSummary> {
    tracing::info!(auto_fix, "reconcile pass starting");
    let openfga = run_openfga_reconcile(pool, authz, auto_fix).await?;
    let qdrant_report = run_qdrant_reconcile(pool, qdrant, auto_fix).await?;
    tracing::info!(
        auto_fix,
        openfga_missing = openfga.missing_in_openfga.count,
        openfga_orphaned = openfga.orphaned_in_openfga.count,
        openfga_written = openfga.written,
        openfga_deleted = openfga.deleted,
        qdrant_orphaned_chunks = qdrant_report.orphaned_chunk_points.count,
        qdrant_orphaned_graph = qdrant_report.orphaned_graph_points.count,
        qdrant_missing_chunks = qdrant_report.missing_chunk_points.count,
        qdrant_missing_graph = qdrant_report.missing_graph_points.count,
        qdrant_deleted_chunk_docs = qdrant_report.deleted_chunk_docs,
        qdrant_deleted_graph_nodes = qdrant_report.deleted_graph_nodes,
        "reconcile pass complete"
    );
    Ok(ReconcileSummary {
        openfga,
        qdrant: qdrant_report,
        auto_fix,
    })
}
