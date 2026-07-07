//! T43 — integration tests for the retry wrapper (`process_job_with_retry`).
//!
//! Uses a `MockRunner` (no live S3/Qdrant/Ollama) + `#[sqlx::test]` DB to
//! verify `ingest_jobs.status` transitions and that the wrapper never
//! propagates a job error to the poll loop.

use gmrag_worker::{process_job_with_retry, IngestJob, JobRunner, MAX_ATTEMPTS};
use sqlx::PgPool;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use uuid::Uuid;

/// Mock runner that fails the first `fail_first` calls then succeeds.
struct MockRunner {
    fail_first: u32,
    calls: Arc<AtomicU32>,
    err_msg: String,
}

#[async_trait::async_trait]
impl JobRunner for MockRunner {
    async fn run(&self, _job: &IngestJob) -> Result<(), String> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if n <= self.fail_first {
            Err(self.err_msg.clone())
        } else {
            Ok(())
        }
    }
}

/// Runner that always fails.
struct AlwaysFailRunner {
    err: String,
}

#[async_trait::async_trait]
impl JobRunner for AlwaysFailRunner {
    async fn run(&self, _job: &IngestJob) -> Result<(), String> {
        Err(self.err.clone())
    }
}

async fn seed_job(pool: &PgPool) -> (Uuid, IngestJob) {
    seed_job_with_attempts(pool, 0).await
}

/// Seed a tenant/workspace/document/ingest_job with `initial_attempts` already
/// recorded on the `ingest_jobs` row (simulating one or more prior sweeper
/// requeues). The returned `IngestJob.attempts` mirrors this so the retry
/// wrapper sees the prior-attempt count.
async fn seed_job_with_attempts(pool: &PgPool, initial_attempts: i32) -> (Uuid, IngestJob) {
    let tenant = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, $2)")
        .bind(tenant)
        .bind("T43 Tenant")
        .execute(pool)
        .await
        .unwrap();
    let owner = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, email, name) VALUES ($1, $2, $3)")
        .bind(owner)
        .bind(format!("u{owner}@t43.test"))
        .bind("T43 Owner")
        .execute(pool)
        .await
        .unwrap();
    let ws = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workspaces (id, tenant_id, name, slug, created_by) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(ws)
    .bind(tenant)
    .bind("T43 WS")
    .bind(format!("t43-ws-{ws}"))
    .bind(owner)
    .execute(pool)
    .await
    .unwrap();
    let doc = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO documents (id, tenant_id, workspace_id, owner_id, title, status, visibility, s3_key)
           VALUES ($1, $2, $3, $4, 'T43 doc', 'uploaded', 'private', 'k')"#,
    )
    .bind(doc)
    .bind(tenant)
    .bind(ws)
    .bind(owner)
    .execute(pool)
    .await
    .unwrap();
    let job_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO ingest_jobs (id, tenant_id, document_id, status, attempts) VALUES ($1, $2, $3, 'pending', $4)",
    )
    .bind(job_id)
    .bind(tenant)
    .bind(doc)
    .bind(initial_attempts)
    .execute(pool)
    .await
    .unwrap();

    let job = IngestJob {
        id: job_id,
        tenant_id: tenant,
        workspace_id: ws,
        document_id: doc,
        s3_key: "k".into(),
        filename: "f.pdf".into(),
        owner_id: owner,
        visibility: "private".into(),
        attempts: initial_attempts.max(0) as u32,
    };
    (tenant, job)
}

async fn job_status(pool: &PgPool, job_id: Uuid) -> (String, i32, Option<String>) {
    let row: (String, i32, Option<String>) =
        sqlx::query_as("SELECT status, attempts, last_error FROM ingest_jobs WHERE id = $1")
            .bind(job_id)
            .fetch_one(pool)
            .await
            .unwrap();
    row
}

async fn doc_status(pool: &PgPool, doc_id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM documents WHERE id = $1")
        .bind(doc_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn retry_succeeds_on_second_attempt(pool: PgPool) {
    let (_tenant, job) = seed_job(&pool).await;
    let runner = MockRunner {
        fail_first: 1,
        calls: Arc::new(AtomicU32::new(0)),
        err_msg: "transient boom".into(),
    };

    // Patch backoff to ~0 by running directly (real backoff is 1s — acceptable
    // for one retry in CI).
    process_job_with_retry(&runner, &pool, &job)
        .await
        .expect("wrapper must not error on success");

    let (status, attempts, last_error) = job_status(&pool, job.id).await;
    assert_eq!(status, "completed");
    // Phase 1 corrected semantic: `attempts` counts every real run, so 1
    // failed attempt + 1 successful attempt = 2 total real attempts.
    assert_eq!(
        attempts, 2,
        "two real attempts (1 failure + 1 success) recorded"
    );
    assert!(last_error.is_none(), "completed job clears last_error");

    let doc_st = doc_status(&pool, job.document_id).await;
    assert_eq!(
        doc_st, "indexed",
        "document status must be 'indexed' on success"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn retry_marks_failed_after_three_attempts(pool: PgPool) {
    let (_tenant, job) = seed_job(&pool).await;
    let runner = AlwaysFailRunner {
        err: "permanent failure".into(),
    };

    // Override backoff to keep the test fast: we can't easily patch the const,
    // so this test pays the real backoff (1s + 2s = 3s). Acceptable.
    process_job_with_retry(&runner, &pool, &job)
        .await
        .expect("wrapper must not propagate job error (no worker crash)");

    let (status, attempts, last_error) = job_status(&pool, job.id).await;
    assert_eq!(status, "failed", "must be marked failed after 3 attempts");
    assert_eq!(attempts, 3, "attempts must reach MAX_ATTEMPTS");
    assert_eq!(
        last_error.as_deref(),
        Some("permanent failure"),
        "last_error must be the final error message"
    );

    let doc_st = doc_status(&pool, job.document_id).await;
    assert_eq!(
        doc_st, "failed",
        "document status must be 'failed' after max retries"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn retry_first_attempt_success_marks_completed_immediately(pool: PgPool) {
    let (_tenant, job) = seed_job(&pool).await;
    let runner = MockRunner {
        fail_first: 0,
        calls: Arc::new(AtomicU32::new(0)),
        err_msg: "unused".into(),
    };

    process_job_with_retry(&runner, &pool, &job)
        .await
        .expect("wrapper ok");

    let (status, attempts, last_error) = job_status(&pool, job.id).await;
    assert_eq!(status, "completed");
    // Phase 1 corrected semantic: `attempts` counts every real run, so a
    // first-try success still records 1 real attempt.
    assert_eq!(attempts, 1, "first-try success records 1 real attempt");
    assert!(last_error.is_none());

    let doc_st = doc_status(&pool, job.document_id).await;
    assert_eq!(
        doc_st, "indexed",
        "document status must be 'indexed' on first-try success"
    );
}

// ─── Phase 1, Task 2: retry attempts must account for prior attempts ──────
//
// A job requeued by the sweeper already has N recorded attempts. The in-memory
// retry loop must run at most `MAX_ATTEMPTS - N` more tries, not a fresh full
// set of MAX_ATTEMPTS.

/// Runner that records how many times it was invoked, always failing.
struct CountingFailRunner {
    calls: Arc<AtomicU32>,
    err: String,
}

#[async_trait::async_trait]
impl JobRunner for CountingFailRunner {
    async fn run(&self, _job: &IngestJob) -> Result<(), String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(self.err.clone())
    }
}

/// Runner that succeeds on the Nth invocation (1-indexed) and records calls.
struct SucceedOnNthRunner {
    succeed_at: u32,
    calls: Arc<AtomicU32>,
}

#[async_trait::async_trait]
impl JobRunner for SucceedOnNthRunner {
    async fn run(&self, _job: &IngestJob) -> Result<(), String> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if n >= self.succeed_at {
            Ok(())
        } else {
            Err(format!("transient fail at call {n}"))
        }
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn fresh_job_attempts_zero_gets_full_max_attempts(pool: PgPool) {
    // A fresh job (attempts=0) must get the full MAX_ATTEMPTS tries.
    let (_tenant, job) = seed_job_with_attempts(&pool, 0).await;
    assert_eq!(job.attempts, 0);
    let calls = Arc::new(AtomicU32::new(0));
    let runner = CountingFailRunner {
        calls: calls.clone(),
        err: "always fail".into(),
    };

    process_job_with_retry(&runner, &pool, &job)
        .await
        .expect("wrapper must not propagate job error");

    let invoked = calls.load(Ordering::SeqCst);
    assert_eq!(
        invoked, MAX_ATTEMPTS,
        "fresh job (attempts=0) must get exactly MAX_ATTEMPTS ({MAX_ATTEMPTS}) tries, got {invoked}"
    );
    let (status, attempts, _last_error) = job_status(&pool, job.id).await;
    assert_eq!(status, "failed");
    assert_eq!(
        attempts as u32, MAX_ATTEMPTS,
        "persisted attempts must reach MAX_ATTEMPTS"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn job_with_two_prior_attempts_gets_only_one_more_try(pool: PgPool) {
    // A job already requeued once (attempts=2 → simulating one sweeper
    // requeue after 2 prior failed attempts) must get at most
    // `MAX_ATTEMPTS - 2` more tries (1 more for MAX_ATTEMPTS=3), NOT a fresh
    // full set of 3.
    let (_tenant, job) = seed_job_with_attempts(&pool, 2).await;
    assert_eq!(job.attempts, 2);
    let calls = Arc::new(AtomicU32::new(0));
    let runner = CountingFailRunner {
        calls: calls.clone(),
        err: "still failing".into(),
    };

    process_job_with_retry(&runner, &pool, &job)
        .await
        .expect("wrapper must not propagate job error");

    let invoked = calls.load(Ordering::SeqCst);
    let expected = MAX_ATTEMPTS.saturating_sub(job.attempts);
    assert_eq!(
        invoked, expected,
        "job with 2 prior attempts must get exactly {expected} more try(s), got {invoked} (would be {MAX_ATTEMPTS} if bug regressed)"
    );
    let (status, attempts, _last_error) = job_status(&pool, job.id).await;
    assert_eq!(status, "failed");
    assert_eq!(
        attempts as u32, MAX_ATTEMPTS,
        "persisted attempts must total MAX_ATTEMPTS after final failure, got {attempts}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn job_with_two_prior_attempts_can_succeed_on_the_one_remaining_try(pool: PgPool) {
    // Mirrors the prior test, but the single remaining attempt succeeds —
    // proving the wrapper still allows the last attempt and marks completed.
    let (_tenant, job) = seed_job_with_attempts(&pool, 2).await;
    let calls = Arc::new(AtomicU32::new(0));
    let runner = SucceedOnNthRunner {
        succeed_at: 1,
        calls: calls.clone(),
    };

    process_job_with_retry(&runner, &pool, &job)
        .await
        .expect("wrapper ok");

    let invoked = calls.load(Ordering::SeqCst);
    assert_eq!(invoked, 1, "only the one remaining attempt should run");
    let (status, attempts, _last_error) = job_status(&pool, job.id).await;
    assert_eq!(status, "completed");
    // On success the wrapper records the attempt count at the point of
    // success; prior attempts (2) + this one successful attempt.
    assert_eq!(
        attempts as u32, MAX_ATTEMPTS,
        "persisted attempts should reflect total attempts at success ({MAX_ATTEMPTS})"
    );
    let doc_st = doc_status(&pool, job.document_id).await;
    assert_eq!(doc_st, "indexed");
}

#[sqlx::test(migrations = "../../migrations")]
async fn job_already_at_max_attempts_is_failed_immediately_without_running(pool: PgPool) {
    // A job whose attempts already reached MAX_ATTEMPTS (e.g. a race with the
    // sweeper) must be marked failed immediately and the runner must NOT be
    // invoked even once.
    let (_tenant, job) = seed_job_with_attempts(&pool, MAX_ATTEMPTS as i32).await;
    assert_eq!(job.attempts, MAX_ATTEMPTS);
    let calls = Arc::new(AtomicU32::new(0));
    let runner = CountingFailRunner {
        calls: calls.clone(),
        err: "should not run".into(),
    };

    process_job_with_retry(&runner, &pool, &job)
        .await
        .expect("wrapper must not propagate job error");

    let invoked = calls.load(Ordering::SeqCst);
    assert_eq!(
        invoked, 0,
        "runner must NOT be invoked when attempts >= MAX_ATTEMPTS, got {invoked} invocation(s)"
    );
    let (status, attempts, last_error) = job_status(&pool, job.id).await;
    assert_eq!(status, "failed", "must be marked failed immediately");
    assert_eq!(
        attempts as u32, MAX_ATTEMPTS,
        "attempts stays at MAX_ATTEMPTS, got {attempts}"
    );
    assert!(
        last_error.is_some(),
        "last_error must be set on immediate-fail path"
    );
    let doc_st = doc_status(&pool, job.document_id).await;
    assert_eq!(doc_st, "failed");
}

#[sqlx::test(migrations = "../../migrations")]
async fn job_above_max_attempts_is_failed_immediately_without_running(pool: PgPool) {
    // Defensive: even attempts > MAX_ATTEMPTS (corrupted row) must not run the
    // job and must be marked failed immediately.
    let (_tenant, job) = seed_job_with_attempts(&pool, (MAX_ATTEMPTS + 5) as i32).await;
    let calls = Arc::new(AtomicU32::new(0));
    let runner = CountingFailRunner {
        calls: calls.clone(),
        err: "should not run".into(),
    };

    process_job_with_retry(&runner, &pool, &job)
        .await
        .expect("wrapper ok");

    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "runner must not be invoked when attempts > MAX_ATTEMPTS"
    );
    let (status, _attempts, _last_error) = job_status(&pool, job.id).await;
    assert_eq!(status, "failed");
    let doc_st = doc_status(&pool, job.document_id).await;
    assert_eq!(doc_st, "failed");
}
