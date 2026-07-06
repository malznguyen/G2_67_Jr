//! T84D Phase 1.2 — Job recovery sweeper (Phase 2: lease-safe claim).
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
//!
//! # Phase 2 — lease-safe claim
//!
//! Three bugs fixed (see `docs/PHASE2_REPORT.md`):
//! 1. **Duplicate requeue:** the SELECT now uses `FOR UPDATE SKIP LOCKED`
//!    so two concurrent sweeper ticks can never claim the same row.
//! 2. **LPUSH-before-commit:** the DB transaction (claim + attempts
//!    increment + status/`claimed_at` update) now COMMITS before any
//!    `LPUSH`. If the commit fails, no push happens. If the `LPUSH` fails
//!    after a successful commit, the row is left as `status='pending',
//!    claimed_at=now()` (option a) — it becomes re-eligible for sweeping
//!    only after the lease expires, at which point it is re-pushed. This
//!    is safe recovery, not duplication (`FOR UPDATE SKIP LOCKED` prevents
//!    concurrent double-claims; the lease gap prevents immediate re-claim).
//! 3. **Cap-at-sweep:** if `row.attempts + 1 > MAX_ATTEMPTS` the job is
//!    marked `failed` directly inside the claim transaction (with a clear
//!    `last_error`) and is NOT pushed to Redis — no pointless work for an
//!    already-exhausted job.
//!
//! The "requeue consumes one attempt" semantic is preserved: a normal
//! requeue still sets `attempts = row.attempts + 1`.

use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::job::MAX_ATTEMPTS;
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
/// See the module docs for the Phase 2 lease-safe claim semantics.
/// Returns the number of jobs successfully re-enqueued (LPUSHed).
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
    // ── Phase 2 Task 1: atomic claim with FOR UPDATE SKIP LOCKED ──────
    //
    // The SELECT, the attempts increment, and the status/claimed_at update
    // all happen inside ONE transaction. Two concurrent sweeper ticks can
    // never select the same row: `FOR UPDATE SKIP LOCKED` skips any row
    // already locked by another tx.
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
        FOR UPDATE SKIP LOCKED
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

    // Payloads to LPUSH AFTER the tx commits (Phase 2 Task 2: DB commit
    // before Redis push).
    let mut pending_pushes: Vec<Vec<u8>> = Vec::new();
    let mut reenqueued = 0usize;

    for row in rows {
        let row_attempts = row.attempts.max(0) as u32;

        // ── Phase 2 Task 1: cap-at-sweep ──────────────────────────────
        //
        // If this requeue would push the job over MAX_ATTEMPTS, mark it
        // failed directly inside the claim transaction. Do NOT LPUSH.
        if row_attempts + 1 > MAX_ATTEMPTS {
            let msg = format!(
                "exhausted retries while stuck/sweeper-claimed (attempts={row_attempts}, max={MAX_ATTEMPTS})"
            );
            sqlx::query(
                r#"
                UPDATE ingest_jobs
                SET status = 'failed',
                    claimed_at = NULL,
                    last_error = $1,
                    updated_at = now()
                WHERE id = $2
                "#,
            )
            .bind(&msg)
            .bind(row.id)
            .execute(&mut *tx)
            .await
            .context("mark stuck job failed at cap")?;

            // Mark the document failed too so it's not orphaned in
            // 'processing'. This is a direct consequence of marking the
            // job failed, not a separate feature.
            sqlx::query("UPDATE documents SET status = 'failed', updated_at = now() WHERE id = $1")
                .bind(row.document_id)
                .execute(&mut *tx)
                .await
                .context("mark document failed at sweeper cap")?;

            tracing::warn!(
                job_id = %row.id,
                attempts = row_attempts,
                max = MAX_ATTEMPTS,
                "sweeper: stuck job exhausted retries — marked failed without requeue"
            );
            continue;
        }

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

        let new_attempts = row_attempts + 1;
        let payload = SweeperPayload {
            id: row.id,
            tenant_id: row.tenant_id,
            workspace_id: doc.workspace_id,
            document_id: row.document_id,
            s3_key: doc.s3_key.clone(),
            filename: doc.title.clone(),
            owner_id: doc.owner_id,
            visibility: doc.visibility.clone(),
            attempts: new_attempts,
        };

        let bytes = serde_json::to_vec(&payload).context("serialize sweeper payload")?;

        // ── Phase 2 Task 1+2: claim inside the tx ─────────────────────
        //
        // Set claimed_at = now() (NOT NULL) so the row is NOT immediately
        // re-eligible for sweeping. This closes the post-commit-pre-LPUSH
        // window: even if the LPUSH fails after commit, the row won't be
        // re-swept until the lease expires (option a).
        sqlx::query(
            r#"
            UPDATE ingest_jobs
            SET status = 'pending',
                claimed_at = now(),
                attempts = $1,
                updated_at = now()
            WHERE id = $2
            "#,
        )
        .bind(new_attempts as i32)
        .bind(row.id)
        .execute(&mut *tx)
        .await
        .context("reset stuck ingest_job")?;

        pending_pushes.push(bytes);
    }

    // ── Phase 2 Task 2: commit BEFORE any LPUSH ───────────────────────
    //
    // If the commit fails, no LPUSH happens — the job remains in its prior
    // (stuck) state for the next sweeper tick to reconsider. A job that
    // fails to be claimed this tick looks, from the DB's perspective,
    // exactly like a job that was never picked up this tick.
    tx.commit().await.context("commit sweep tx")?;

    // ── Phase 2 Task 2: LPUSH after successful commit ─────────────────
    //
    // If an LPUSH fails (Redis unavailable, network blip), we log the
    // error and continue (option a). The DB row is already committed as
    // `status='pending', claimed_at=now()`, so it will be re-swept after
    // the lease expires and re-pushed then. This is safe recovery, not
    // duplication.
    for bytes in pending_pushes {
        if let Err(e) = queue.lpush(INGEST_JOBS_KEY, bytes).await {
            tracing::error!(
                error = %e,
                "sweeper: LPUSH failed after DB commit; row will be re-swept after lease expires (option a)"
            );
            // Don't return Err — the worker loop must not abort on a Redis
            // blip. The committed row is safe (claimed, re-eligible later).
            continue;
        }
        reenqueued += 1;
    }

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