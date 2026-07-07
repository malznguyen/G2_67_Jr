//! Phase 3 — periodic cross-system drift reconciliation loop.
//!
//! Follows the same "background task spawned from `run()`" shape as
//! `retention.rs`, but — unlike retention — honors a shutdown signal so an
//! in-progress reconcile run is never abandoned. The loop sleeps outside any
//! reconcile call; on each tick it runs both the OpenFGA and Qdrant
//! reconcilers and logs a structured summary.
//!
//! `auto_fix = false` (the default, from `GMRAG_RECONCILE_AUTO_FIX`) means
//! report-only: the loop never writes or deletes anything. This is enforced
//! inside the reconcilers themselves and asserted by tests.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use gmrag_api::authz::AuthzService;
use gmrag_core::QdrantStore;
use sqlx::PgPool;
use tracing::info;

/// Pluggable runner so the loop can be tested with a mock instead of the full
/// live reconciler (mirrors `job::JobRunner` / `lib::JobHandler`).
#[async_trait::async_trait]
pub trait ReconcileRunner: Send + Sync {
    /// Run one reconciliation pass. `auto_fix` is forwarded from the loop
    /// config so the runner — and tests — can observe which mode ran.
    async fn run_once(&self, auto_fix: bool) -> anyhow::Result<()>;
}

/// Real runner: delegates to `gmrag_api::reconcile::run_reconcile_once`.
pub struct RealReconcileRunner {
    pub pool: PgPool,
    pub qdrant: QdrantStore,
    pub authz: AuthzService,
}

#[async_trait::async_trait]
impl ReconcileRunner for RealReconcileRunner {
    async fn run_once(&self, auto_fix: bool) -> anyhow::Result<()> {
        let summary = gmrag_api::reconcile::run_reconcile_once(
            &self.pool,
            &self.qdrant,
            &*self.authz,
            auto_fix,
        )
        .await?;
        gmrag_api::metrics::metrics().record_reconcile_success(&summary);
        Ok(())
    }
}

/// Run the reconcile loop until `shutdown` resolves.
///
/// - Ticks every `interval`; each tick runs one `run_once(auto_fix)`.
/// - `select!` is `biased` with the shutdown arm first, but the tick arm's
///   `.await` runs to completion before the next `select!` polls shutdown, so
///   an in-progress reconcile run is **never abandoned** — it finishes, then
///   the loop observes shutdown and exits.
/// - `auto_fix = false` → the runner (and the reconcilers under it) never
///   write or delete; verified by the reconciler unit tests.
pub async fn run_reconcile_loop<R, S>(
    runner: Arc<R>,
    interval: Duration,
    auto_fix: bool,
    shutdown: S,
) where
    R: ReconcileRunner + 'static,
    S: Future<Output = ()>,
{
    use tokio::pin;
    pin!(shutdown);

    let mut ticker = tokio::time::interval(interval);
    // The first tick fires immediately; consume it so the loop waits one
    // full interval before the first reconcile (matches "periodic" intent
    // and avoids a reconcile-on-start stampede with the sweeper/retention).
    ticker.tick().await;

    info!(
        interval_secs = interval.as_secs(),
        auto_fix, "reconcile loop started"
    );
    loop {
        tokio::select! {
            biased;
            _ = &mut shutdown => {
                info!("reconcile loop shutting down");
                break;
            }
            _ = ticker.tick() => {
                info!(auto_fix, "reconcile tick: starting pass");
                match runner.run_once(auto_fix).await {
                    Ok(()) => info!(auto_fix, "reconcile tick: pass complete"),
                    Err(e) => {
                        gmrag_api::metrics::metrics().record_reconcile_failure();
                        tracing::warn!(error = %e, "reconcile tick: pass failed");
                    }
                }
            }
        }
    }
    info!("reconcile loop stopped");
}
