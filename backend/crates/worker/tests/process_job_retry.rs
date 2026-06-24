//! T43 — integration tests for the retry wrapper (`process_job_with_retry`).
//!
//! Uses a `MockRunner` (no live S3/Qdrant/Ollama) + `#[sqlx::test]` DB to
//! verify `ingest_jobs.status` transitions and that the wrapper never
//! propagates a job error to the poll loop.

use gmrag_worker::{IngestJob, JobRunner, process_job_with_retry};
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
        "INSERT INTO ingest_jobs (id, tenant_id, document_id, status, attempts) VALUES ($1, $2, $3, 'pending', 0)",
    )
    .bind(job_id)
    .bind(tenant)
    .bind(doc)
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
        attempts: 0,
    };
    (tenant, job)
}

async fn job_status(pool: &PgPool, job_id: Uuid) -> (String, i32, Option<String>) {
    let row: (String, i32, Option<String>) = sqlx::query_as(
        "SELECT status, attempts, last_error FROM ingest_jobs WHERE id = $1",
    )
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
    assert_eq!(attempts, 1, "one failed attempt recorded before success");
    assert!(last_error.is_none(), "completed job clears last_error");

    let doc_st = doc_status(&pool, job.document_id).await;
    assert_eq!(doc_st, "indexed", "document status must be 'indexed' on success");
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
    assert_eq!(doc_st, "failed", "document status must be 'failed' after max retries");
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
    assert_eq!(attempts, 0, "no failures on first-try success");
    assert!(last_error.is_none());

    let doc_st = doc_status(&pool, job.document_id).await;
    assert_eq!(
        doc_st, "indexed",
        "document status must be 'indexed' on first-try success"
    );
}
