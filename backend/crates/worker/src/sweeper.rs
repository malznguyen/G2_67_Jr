//! T84D Phase 1.2 — Job recovery sweeper.
//!
//! A worker can crash while a job is in `processing`, leaving it stuck
//! forever (the SweeperP0 risk from the scalability audit). The sweeper
//! periodically scans `ingest_jobs` for rows whose `claimed_at` (or plain
//! `pending` rows with no claim) have aged beyond the lease window and
//! re-enqueues them onto Redis by LPUSH-ing a fresh `IngestJobPayload`.
//!
//! INVARIANT: the sweeper uses the `admin_pool` — a documented
//! exception to the project rule "worker uses app_pool for business
//! logic". Re-enqueuing stuck jobs is platform maintenance, not
//! tenant business data: the SELECT scans across all tenants (the
//! app_pool, RLS-scoped per tx, could only see one tenant). The
//! per-row `IngestJobPayload` is rebuilt from the document row using a
//! tenant_id-scoped query, so the enqueued payload is still
//! tenant-bound at the worker.

use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::queue::{INGEST_JOBS_KEY, JobQueue};

/// Default lease: a job whose `claimed_at` is older than 15 minutes is
/// considered stuck (its owning worker is presumed dead).
pub const DEFAULT_LEASE_SECS: i64 = 15 * 60;

/// Admin-row scan result. Stuck jobs are cross-tenant by design.
struct StuckRow {
    id: Uuid,
    document_id: Uuid,
    tenant_id: Uuid,
    attempts: i32,
}

/// Document fields needed to rebuild the payload (mirror of
/// `IngestJobPayload`).
struct DocRow {
    s3_key: String,
    workspace_id: Uuid,
    owner_id: Uuid,
    visibility: String,
    title: String,
}

/// Wire payload rebuilt by the sweeper. Mirrors the API's
/// `IngestJobPayload` and the worker's `IngestJob`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SweeperPayload {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub workspace_id: Uuid,
    pub document_id: Uuid,
    pub s3_key: String,
    pub filename: String,
    pub owner_id: Uuid,
    pub visibility: String,
    pub attempts: u32,
}

/// Re-enqueue stuck ingestion jobs onto Redis.
///
/// 1. SELECT pending/processing rows whose `claimed_at` is NULL or older
///    than `lease_secs` — admin_pool, cross-tenant.
/// 2. For each row, re-fetch the document row (tenant-scoped) to rebuild
///    the full payload.
/// 3. `redis.LPUSH(INGEST_JOBS_KEY, payload)` then re-mark the job
///    `pending` with `claimed_at=NULL`, `attempts=$attempts+1`.
///
/// Returns the number of jobs re-enqueued.
pub async fn requeue_stuck_jobs(
    pool: &PgPool,
    queue: &mut dyn JobQueue,
    lease_secs: i64,
) -> anyhow::Result<usize> {
    requeue_stuck_jobs_with_limit(pool, queue, lease_secs, 500).await
}

pub async fn requeue_stuck_jobs_with_limit(
    pool: &PgPool,
    queue: &mut dyn JobQueue,
    lease_secs: i64,
    limit: i64,
) -> anyhow::Result<usize> {
    let mut tx = pool.begin().await.context("begin sweep tx")?;

    let rows: Vec<StuckRow> = sqlx::query_as::<
        _,
        (Uuid, Uuid, Uuid, i32),
    >(
        r#"
        SELECT id, document_id, tenant_id, attempts
        FROM ingest_jobs
        WHERE status IN ('pending', 'processing')
          AND (claimed_at IS NULL OR claimed_at < now() - ($1 || ' seconds')::interval)
        ORDER BY created_at
        LIMIT $2
        "#,
    )
    .bind(lease_secs)
    .bind(limit)
    .fetch_all(&mut *tx)
    .await
    .context("select stuck ingest_jobs")?
    .into_iter()
    .map(|(id, document_id, tenant_id, attempts)| StuckRow {
        id,
        document_id,
        tenant_id,
        attempts,
    })
    .collect();

    let mut reenqueued = 0usize;
    for row in rows {
        // Re-fetch the document row scoped by tenant_id (RLS-enforced if the
        // admin role ever drops, but admin_pool bypasses RLS here by design —
        // the WHERE clause scopes manually).
        let doc: Option<DocRow> = sqlx::query_as::<
            _,
            (Option<String>, Uuid, Uuid, String, String),
        >(
            r#"
            SELECT s3_key, workspace_id, owner_id, visibility, title
            FROM documents
            WHERE id = $1 AND tenant_id = $2
            "#,
        )
        .bind(row.document_id)
        .bind(row.tenant_id)
        .fetch_optional(&mut *tx)
        .await
        .context("fetch document for sweeper")?
        .map(|(s3_key, workspace_id, owner_id, visibility, title)| DocRow {
            s3_key: s3_key.unwrap_or_default(),
            workspace_id,
            owner_id,
            visibility,
            title,
        });

        let Some(doc) = doc else {
            // Document deleted before the sweeper noticed — drop the job.
            tracing::warn!(
                job_id = %row.id,
                document_id = %row.document_id,
                "sweeper: document vanished — dropping stuck job"
            );
            sqlx::query("DELETE FROM ingest_jobs WHERE id = $1")
                .bind(row.id)
                .execute(&mut *tx)
                .await
                .context("delete orphaned ingest_job")?;
            continue;
        };

        let payload = SweeperPayload {
            id: row.id,
            tenant_id: row.tenant_id,
            workspace_id: doc.workspace_id,
            document_id: row.document_id,
            s3_key: doc.s3_key.clone(),
            filename: doc.title.clone(),
            owner_id: doc.owner_id,
            visibility: doc.visibility.clone(),
            attempts: (row.attempts.max(0) as u32) + 1,
        };

        let bytes = serde_json::to_vec(&payload).context("serialize sweeper payload")?;
        queue
            .lpush(INGEST_JOBS_KEY, bytes)
            .await
            .map_err(|e| anyhow::anyhow!("sweeper lpush: {e}"))?;

        sqlx::query(
            r#"
            UPDATE ingest_jobs
            SET status = 'pending', claimed_at = NULL,
                attempts = $1, updated_at = now()
            WHERE id = $2
            "#,
        )
        .bind((row.attempts.max(0) + 1) as i32)
        .bind(row.id)
        .execute(&mut *tx)
        .await
        .context("reset stuck ingest_job")?;

        reenqueued += 1;
    }

    tx.commit().await.context("commit sweep tx")?;
    Ok(reenqueued)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-time sanity: the sweeper payload schema matches the
    /// worker's [`IngestJob`](crate::job::IngestJob).
    #[test]
    fn sweeper_payload_roundtrips_ingest_job() {
        let p = SweeperPayload {
            id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            workspace_id: Uuid::nil(),
            document_id: Uuid::nil(),
            s3_key: "k".into(),
            filename: "f.pdf".into(),
            owner_id: Uuid::nil(),
            visibility: "private".into(),
            attempts: 1,
        };
        let bytes = serde_json::to_vec(&p).expect("serialize");
        let job: crate::job::IngestJob = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(job.s3_key, "k");
        assert_eq!(job.attempts, 1);
    }
}