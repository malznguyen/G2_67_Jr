//! Phase 2 — integration tests for the lease-safe sweeper.
//!
//! Covers the three known bugs fixed in Phase 2:
//! 1. Duplicate requeue when two sweeper ticks overlap (no row locking).
//! 2. LPUSH-before-commit leaving orphan Redis entries on crash/DB failure.
//! 3. Sweeper requeuing jobs already at/past MAX_ATTEMPTS instead of
//!    marking them failed directly.
//!
//! Also confirms the sweeper↔retry interaction: a job requeued by the
//! sweeper then run through `process_job_with_retry` never exceeds
//! MAX_ATTEMPTS total real attempts.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use gmrag_core::status::{document as doc_status, ingest_job as job_status};
use gmrag_worker::{
    process_job_with_retry, requeue_stuck_jobs_with_limit, IngestJob, JobQueue, JobRunner,
    MockQueue, MAX_ATTEMPTS,
};
use sqlx::PgPool;
use uuid::Uuid;

// ─── helpers ─────────────────────────────────────────────────────────────

/// Seed a tenant/workspace/user/document and return their IDs.
async fn seed_tenant_doc(
    pool: &PgPool,
) -> (Uuid, Uuid, Uuid, Uuid) {
    let tenant = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, $2)")
        .bind(tenant)
        .bind("P2 Tenant")
        .execute(pool)
        .await
        .unwrap();
    let owner = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, email, name) VALUES ($1, $2, $3)")
        .bind(owner)
        .bind(format!("u{owner}@p2.test"))
        .bind("P2 Owner")
        .execute(pool)
        .await
        .unwrap();
    let ws = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workspaces (id, tenant_id, name, slug, created_by) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(ws)
    .bind(tenant)
    .bind("P2 WS")
    .bind(format!("p2-ws-{ws}"))
    .bind(owner)
    .execute(pool)
    .await
    .unwrap();
    let doc = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO documents (id, tenant_id, workspace_id, owner_id, title, status, visibility, s3_key)
           VALUES ($1, $2, $3, $4, 'P2 doc', 'processing', 'private', 'k')"#,
    )
    .bind(doc)
    .bind(tenant)
    .bind(ws)
    .bind(owner)
    .execute(pool)
    .await
    .unwrap();
    (tenant, ws, owner, doc)
}

/// Seed a stuck `ingest_jobs` row: `status='processing'`, `claimed_at` far
/// in the past (older than the lease), `attempts = initial_attempts`.
async fn seed_stuck_job(
    pool: &PgPool,
    tenant: Uuid,
    doc: Uuid,
    initial_attempts: i32,
) -> Uuid {
    let job_id = Uuid::new_v4();
    // claimed_at set 1 hour ago — well past any reasonable lease.
    sqlx::query(
        r#"INSERT INTO ingest_jobs (id, tenant_id, document_id, status, attempts, claimed_at)
           VALUES ($1, $2, $3, 'processing', $4, now() - interval '1 hour')"#,
    )
    .bind(job_id)
    .bind(tenant)
    .bind(doc)
    .bind(initial_attempts)
    .execute(pool)
    .await
    .unwrap();
    job_id
}

async fn job_row(pool: &PgPool, job_id: Uuid) -> (String, i32, Option<String>, Option<chrono::DateTime<chrono::Utc>>) {
    sqlx::query_as(
        "SELECT status, attempts, last_error, claimed_at FROM ingest_jobs WHERE id = $1",
    )
    .bind(job_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

// ─── Test 1: duplicate-requeue (the core Phase 2 bug) ────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn sweeper_concurrent_no_duplicate_requeue(pool: PgPool) {
    let (tenant, _ws, _owner, doc) = seed_tenant_doc(&pool).await;
    let job_id = seed_stuck_job(&pool, tenant, doc, 0).await;

    // Two concurrent sweeps against the same stuck job, each with its own
    // MockQueue so we can count total LPUSHes across both.
    let mut q1 = MockQueue::new(vec![]);
    let mut q2 = MockQueue::new(vec![]);

    // Use a tiny lease so the 1-hour-old claimed_at is definitely stale.
    let (r1, r2) = tokio::join!(
        requeue_stuck_jobs_with_limit(&pool, &mut q1, 1, 500),
        requeue_stuck_jobs_with_limit(&pool, &mut q2, 1, 500),
    );
    let n1 = r1.expect("sweep 1 ok");
    let n2 = r2.expect("sweep 2 ok");

    // Exactly one sweep should have claimed the row.
    let total_pushes = q1.pushed().len() + q2.pushed().len();
    assert_eq!(
        total_pushes, 1,
        "two concurrent sweeps must not double-push the same job (got {total_pushes} pushes, n1={n1}, n2={n2})"
    );
    assert_eq!(n1 + n2, 1, "exactly one sweep claims the row");

    // DB reflects a single requeue: attempts = 0 + 1 = 1.
    let (status, attempts, _err, _claimed) = job_row(&pool, job_id).await;
    assert_eq!(status, "pending");
    assert_eq!(attempts, 1, "attempts must be original+1 after one requeue");
}

// ─── Test 2: LPUSH failure → option (a) leave row claimed ────────────────

/// A JobQueue whose LPUSH always fails (simulates Redis unavailable).
struct FailingQueue;

#[async_trait::async_trait]
impl JobQueue for FailingQueue {
    async fn brpop_timeout(
        &mut self,
        _key: &str,
        _timeout_secs: u64,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(None)
    }
    async fn lpush(&mut self, _key: &str, _payload: Vec<u8>) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("simulated redis unavailable"))
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn sweeper_lpush_failure_leaves_row_claimed_for_next_tick(pool: PgPool) {
    let (tenant, _ws, _owner, doc) = seed_tenant_doc(&pool).await;
    let job_id = seed_stuck_job(&pool, tenant, doc, 0).await;

    let mut q = FailingQueue;
    // The sweeper must not panic / return Err that aborts the worker loop;
    // it logs and continues (option a: row stays claimed, re-swept later).
    let n = requeue_stuck_jobs_with_limit(&pool, &mut q, 1, 500)
        .await
        .expect("sweeper must return Ok even when LPUSH fails (option a)");

    // No successful push this tick.
    assert_eq!(n, 0, "zero jobs re-enqueued when LPUSH fails");

    // Option (a): the DB row was committed as claimed (status='pending',
    // claimed_at = now()), so it is NOT immediately re-eligible — it will be
    // picked up again only after the lease expires.
    let (status, attempts, _err, claimed_at) = job_row(&pool, job_id).await;
    assert_eq!(
        status, "pending",
        "row must be committed as pending (claim won)"
    );
    assert!(
        claimed_at.is_some(),
        "option (a): claimed_at must be set so the row is not immediately re-eligible"
    );
    assert_eq!(attempts, 1, "attempts was incremented in the committed claim");
}

// ─── Test 3: cap-at-sweep — one below cap requeues normally ──────────────

#[sqlx::test(migrations = "../../migrations")]
async fn sweeper_requeues_job_one_below_cap(pool: PgPool) {
    let (tenant, _ws, _owner, doc) = seed_tenant_doc(&pool).await;
    // attempts = MAX_ATTEMPTS - 1 → one more requeue is allowed.
    let job_id = seed_stuck_job(&pool, tenant, doc, (MAX_ATTEMPTS - 1) as i32).await;

    let mut q = MockQueue::new(vec![]);
    let n = requeue_stuck_jobs_with_limit(&pool, &mut q, 1, 500)
        .await
        .expect("sweep ok");
    assert_eq!(n, 1, "job one below cap must be requeued");

    let pushed = q.pushed();
    assert_eq!(pushed.len(), 1, "exactly one LPUSH");
    let payload: IngestJob = serde_json::from_slice(&pushed[0]).expect("deserialize");
    assert_eq!(payload.attempts, MAX_ATTEMPTS, "payload attempts = row+1 = MAX_ATTEMPTS");

    let (status, attempts, _err, _claimed) = job_row(&pool, job_id).await;
    assert_eq!(status, "pending");
    assert_eq!(attempts, MAX_ATTEMPTS as i32);
}

// ─── Test 4: cap-at-sweep — at cap marks failed, no LPUSH ────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn sweeper_marks_failed_at_cap_without_pushing(pool: PgPool) {
    let (tenant, _ws, _owner, doc) = seed_tenant_doc(&pool).await;
    // attempts = MAX_ATTEMPTS → already exhausted; sweeper must mark failed
    // directly and NOT push to Redis.
    let job_id = seed_stuck_job(&pool, tenant, doc, MAX_ATTEMPTS as i32).await;

    let mut q = MockQueue::new(vec![]);
    let n = requeue_stuck_jobs_with_limit(&pool, &mut q, 1, 500)
        .await
        .expect("sweep ok");
    assert_eq!(n, 0, "exhausted job must not be re-enqueued");

    let pushed = q.pushed();
    assert!(pushed.is_empty(), "zero LPUSH calls for an exhausted job");

    let (status, attempts, last_error, claimed_at) = job_row(&pool, job_id).await;
    assert_eq!(status, "failed", "exhausted job must be marked failed by sweeper");
    assert_eq!(
        attempts, MAX_ATTEMPTS as i32,
        "attempts must not be incremented past MAX_ATTEMPTS at sweep-fail"
    );
    assert!(
        last_error
            .as_deref()
            .is_some_and(|e| e.contains("exhausted")),
        "last_error must mention exhaustion, got {last_error:?}"
    );
    assert!(
        claimed_at.is_none(),
        "failed job must clear claimed_at"
    );

    // Document must be marked failed too (no orphan 'processing' doc).
    let doc_st: String = sqlx::query_scalar("SELECT status FROM documents WHERE id = $1")
        .bind(doc)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(doc_st, doc_status::FAILED, "document must be marked failed");
}

// ─── Test 5: sweeper + retry interaction — total ≤ MAX_ATTEMPTS ──────────

/// Runner that always fails (we want to exhaust the budget).
struct AlwaysFailRunner {
    err: String,
    calls: Arc<AtomicU32>,
}

#[async_trait::async_trait]
impl JobRunner for AlwaysFailRunner {
    async fn run(&self, _job: &IngestJob) -> Result<(), String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(self.err.clone())
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn sweeper_then_retry_total_attempts_never_exceeds_max(pool: PgPool) {
    let (tenant, _ws, _owner, doc) = seed_tenant_doc(&pool).await;
    // Start the job at attempts=1, processing, stale claim.
    let job_id = seed_stuck_job(&pool, tenant, doc, 1).await;

    // 1. Sweeper requeues it: attempts 1 → 2, status pending, pushed to Redis.
    let mut q = MockQueue::new(vec![]);
    let n = requeue_stuck_jobs_with_limit(&pool, &mut q, 1, 500)
        .await
        .expect("sweep ok");
    assert_eq!(n, 1);
    let pushed = q.pushed();
    assert_eq!(pushed.len(), 1);
    let job: IngestJob = serde_json::from_slice(&pushed[0]).expect("deserialize");
    assert_eq!(job.attempts, 2, "sweeper sets payload attempts = row+1 = 2");

    // 2. Worker pops the payload and runs it through process_job_with_retry
    //    with a runner that always fails. Phase 1 bounds the in-memory loop
    //    by MAX_ATTEMPTS - start_attempts = 3 - 2 = 1 more try.
    let calls = Arc::new(AtomicU32::new(0));
    let runner = AlwaysFailRunner {
        err: "always fail".into(),
        calls: calls.clone(),
    };
    process_job_with_retry(&runner, &pool, &job)
        .await
        .expect("wrapper must not propagate job error");

    // The runner must have been invoked exactly once (the one remaining try).
    let invoked = calls.load(Ordering::SeqCst);
    assert_eq!(
        invoked, 1,
        "sweeper-requeued job (attempts=2) must get exactly 1 more try, got {invoked}"
    );

    // Final DB state: failed, attempts == MAX_ATTEMPTS (2 from sweeper + 1
    // from the retry session = 3 = MAX_ATTEMPTS).
    let (status, attempts, _err, _claimed) = job_row(&pool, job_id).await;
    assert_eq!(status, job_status::FAILED);
    assert_eq!(
        attempts, MAX_ATTEMPTS as i32,
        "total attempts across sweeper + retry must equal MAX_ATTEMPTS, got {attempts}"
    );

    let doc_st: String = sqlx::query_scalar("SELECT status FROM documents WHERE id = $1")
        .bind(doc)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(doc_st, doc_status::FAILED);
}