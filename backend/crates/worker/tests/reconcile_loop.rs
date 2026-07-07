//! Phase 3 — reconcile background loop tests.
//!
//! Verifies:
//! - the loop runs `run_once` on the configured interval;
//! - an in-progress run is NOT abandoned on shutdown (graceful drain);
//! - `auto_fix = false` is forwarded to the runner;
//! - end-to-end: a real OpenFGA reconcile with `auto_fix = false` over seeded
//!   drift makes zero writes (the critical default-OFF gate, through the loop
//!   flag plumbing).

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use gmrag_worker::reconcile_loop::{run_reconcile_loop, ReconcileRunner};

struct MockRunner {
    calls: Mutex<Vec<bool>>,
    delay: Duration,
}

impl MockRunner {
    fn new(delay: Duration) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            delay,
        }
    }
    fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

#[async_trait]
impl ReconcileRunner for MockRunner {
    async fn run_once(&self, auto_fix: bool) -> anyhow::Result<()> {
        self.calls.lock().unwrap().push(auto_fix);
        tokio::time::sleep(self.delay).await;
        Ok(())
    }
}

#[tokio::test]
async fn reconcile_loop_runs_on_interval_and_respects_shutdown() {
    let runner = Arc::new(MockRunner::new(Duration::from_millis(5)));
    let interval = Duration::from_millis(20);
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();

    let r = runner.clone();
    let handle = tokio::spawn(async move {
        run_reconcile_loop(r, interval, false, async {
            let _ = rx.await;
        })
        .await;
    });

    tokio::time::sleep(Duration::from_millis(80)).await;
    let _ = tx.send(());
    handle.await.unwrap();

    assert!(
        runner.call_count() >= 2,
        "loop should have run at least twice, got {}",
        runner.call_count()
    );
    assert!(
        runner.calls.lock().unwrap().iter().all(|&f| !f),
        "auto_fix=false must be forwarded to every run"
    );
}

#[tokio::test]
async fn reconcile_loop_does_not_abandon_in_progress_run() {
    // Interval 10ms (first real tick ~10ms after the immediate tick is
    // consumed); each run sleeps 150ms. We send shutdown at ~40ms — i.e.
    // while the first run is in progress. The run must finish (graceful
    // drain) before the loop exits, proving an in-progress run is never
    // abandoned. The 150ms run window is wide enough to be robust against
    // CI scheduling jitter.
    let runner = Arc::new(MockRunner::new(Duration::from_millis(150)));
    let interval = Duration::from_millis(10);
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();

    let r = runner.clone();
    let handle = tokio::spawn(async move {
        run_reconcile_loop(r, interval, true, async {
            let _ = rx.await;
        })
        .await;
    });

    tokio::time::sleep(Duration::from_millis(40)).await; // first run is in progress
    let _ = tx.send(());
    handle.await.unwrap();

    assert!(
        runner.call_count() >= 1,
        "in-progress run must complete (graceful drain), got {} calls",
        runner.call_count()
    );
    assert!(
        runner.calls.lock().unwrap().iter().all(|&f| f),
        "auto_fix=true must be forwarded"
    );
}

// ── end-to-end gate: real reconciler, auto_fix=false → zero writes ───────

use gmrag_api::authz::{
    AuthorizationService, AuthzError, CheckRequest, CheckResult, Consistency, RelationshipTuple,
};
use gmrag_api::reconcile::run_openfga_reconcile;
use sqlx::PgPool;
use uuid::Uuid;

struct NoopAuthz;
#[async_trait]
impl AuthorizationService for NoopAuthz {
    async fn check(&self, _r: CheckRequest) -> Result<bool, AuthzError> {
        Ok(true)
    }
    async fn batch_check(&self, req: Vec<CheckRequest>) -> Result<Vec<CheckResult>, AuthzError> {
        Ok(req
            .into_iter()
            .map(|r| CheckResult {
                request: r,
                allowed: true,
            })
            .collect())
    }
    async fn list_objects(
        &self,
        _u: &str,
        _r: &str,
        _t: &str,
        _c: Consistency,
    ) -> Result<Vec<String>, AuthzError> {
        Ok(Vec::new())
    }
    async fn read_direct_relationships(
        &self,
        _o: &str,
    ) -> Result<Vec<RelationshipTuple>, AuthzError> {
        Ok(Vec::new())
    }
    async fn read_all_direct_relationships(&self) -> Result<Vec<RelationshipTuple>, AuthzError> {
        Ok(Vec::new())
    }
    async fn write_relationships(
        &self,
        _w: Vec<RelationshipTuple>,
        _d: Vec<RelationshipTuple>,
    ) -> Result<(), AuthzError> {
        Ok(())
    }
    async fn delete_all_direct_relationships_for_object(&self, _o: &str) -> Result<(), AuthzError> {
        Ok(())
    }
    async fn health(&self) -> Result<(), AuthzError> {
        Ok(())
    }
}

async fn seed_tenant_and_doc(pool: &PgPool) -> Uuid {
    let tenant = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, 'loop')")
        .bind(tenant)
        .execute(pool)
        .await
        .unwrap();
    let owner = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, email, name) VALUES ($1, 'o@loop', 'o')")
        .bind(owner)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO tenant_members (tenant_id, user_id, role) VALUES ($1, $2, 'owner')")
        .bind(tenant)
        .bind(owner)
        .execute(pool)
        .await
        .unwrap();
    let doc = Uuid::new_v4();
    sqlx::query("INSERT INTO documents (id, tenant_id, owner_id, title, status, visibility, s3_key) VALUES ($1, $2, $3, 'd', 'indexed', 'private', 'k')")
        .bind(doc)
        .bind(tenant)
        .bind(owner)
        .execute(pool)
        .await
        .unwrap();
    tenant
}

#[sqlx::test(migrations = "../../migrations")]
async fn reconcile_loop_auto_fix_false_never_writes_through_reconciler(pool: PgPool) {
    // Seed drift (tenant + doc present in Postgres → expected tuples exist,
    // but the OpenFGA backend returns NONE → all expected are "missing").
    // auto_fix=false → the reconciler must report drift but make zero writes.
    let _tenant = seed_tenant_and_doc(&pool).await;
    let authz = NoopAuthz;

    let report = run_openfga_reconcile(&pool, &authz, false)
        .await
        .expect("reconcile");

    assert!(
        report.missing_in_openfga.count >= 1,
        "drift should be present with an empty backend"
    );
    assert!(!report.auto_fix_ran, "auto_fix=false must not run repairs");
    assert_eq!(report.written, 0, "no writes in dry-run");
    assert_eq!(report.deleted, 0, "no deletes in dry-run");
}
