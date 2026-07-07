//! Phase 0 (TASK-P0-04) — bounded periodic retention task.
//!
//! Deletes old rows from `ingest_outbox` (dispatched), `usage_events`, and
//! `audit_log` in bounded batches. Runs on the worker's admin pool because
//! it is cross-tenant maintenance (the same sanctioned exception as the
//! outbox relay and the job sweeper — see `worker/src/lib.rs`).
//!
//! Safety properties:
//! - Each run deletes at most `retention_batch_size` rows per table using
//!   `DELETE ... USING (SELECT id FROM ... LIMIT $n) AS batch` — never an
//!   unbounded DELETE.
//! - No transaction is held while sleeping between runs.
//! - Multi-replica execution is safe: duplicate retention runs are
//!   harmless (idempotent deletes) and no row requires exactly-once
//!   deletion.
//! - `ingest_outbox` retention only ever touches `status='dispatched'`
//!   rows; pending rows are preserved regardless of age.
//! - Failures in one table do not abort the others; each pass warn-logs
//!   and continues.

use sqlx::PgPool;

use gmrag_core::status::ingest_outbox as outbox_status;

/// One retention pass: delete a bounded batch of expired rows from each of
/// the three retention-managed tables. Returns the total number of rows
/// deleted across all tables this pass.
pub async fn run_retention_once(
    pool: &PgPool,
    outbox_days: u32,
    usage_days: u32,
    audit_days: u32,
    batch_size: u64,
) -> anyhow::Result<u64> {
    let mut deleted = 0;
    let mut first_error = None;

    match delete_dispatched_outbox_older_than(pool, outbox_days, batch_size).await {
        Ok(n) => deleted += n,
        Err(e) => {
            tracing::warn!(error = %e, table = "ingest_outbox", "retention pass failed");
            first_error = Some(e);
        }
    }
    match delete_usage_older_than(pool, usage_days, batch_size).await {
        Ok(n) => deleted += n,
        Err(e) => {
            tracing::warn!(error = %e, table = "usage_events", "retention pass failed");
            if first_error.is_none() {
                first_error = Some(e);
            }
        }
    }
    match delete_audit_older_than(pool, audit_days, batch_size).await {
        Ok(n) => deleted += n,
        Err(e) => {
            tracing::warn!(error = %e, table = "audit_log", "retention pass failed");
            if first_error.is_none() {
                first_error = Some(e);
            }
        }
    }

    if let Some(e) = first_error {
        Err(e)
    } else {
        Ok(deleted)
    }
}

/// Delete up to `batch_size` dispatched `ingest_outbox` rows whose
/// `dispatched_at` is older than `days` days. Pending rows are never
/// touched (the `status='dispatched'` filter guards them regardless of age).
pub async fn delete_dispatched_outbox_older_than(
    pool: &PgPool,
    days: u32,
    batch_size: u64,
) -> anyhow::Result<u64> {
    let res = sqlx::query(
        r#"
        WITH batch AS (
            SELECT id
            FROM ingest_outbox
            WHERE status = $1
              AND dispatched_at IS NOT NULL
              AND dispatched_at < now() - ($2::int * INTERVAL '1 day')
            ORDER BY dispatched_at
            LIMIT $3
            FOR UPDATE SKIP LOCKED
        )
        DELETE FROM ingest_outbox o
        USING batch
        WHERE o.id = batch.id
        "#,
    )
    .bind(outbox_status::DISPATCHED)
    .bind(days as i32)
    .bind(batch_size as i64)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// Delete up to `batch_size` `usage_events` rows older than `days` days.
pub async fn delete_usage_older_than(
    pool: &PgPool,
    days: u32,
    batch_size: u64,
) -> anyhow::Result<u64> {
    let res = sqlx::query(
        r#"
        WITH batch AS (
            SELECT id
            FROM usage_events
            WHERE created_at < now() - ($1::int * INTERVAL '1 day')
            ORDER BY created_at
            LIMIT $2
            FOR UPDATE SKIP LOCKED
        )
        DELETE FROM usage_events u
        USING batch
        WHERE u.id = batch.id
        "#,
    )
    .bind(days as i32)
    .bind(batch_size as i64)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// Delete up to `batch_size` `audit_log` rows older than `days` days.
pub async fn delete_audit_older_than(
    pool: &PgPool,
    days: u32,
    batch_size: u64,
) -> anyhow::Result<u64> {
    let res = sqlx::query(
        r#"
        WITH batch AS (
            SELECT id
            FROM audit_log
            WHERE created_at < now() - ($1::int * INTERVAL '1 day')
            ORDER BY created_at
            LIMIT $2
            FOR UPDATE SKIP LOCKED
        )
        DELETE FROM audit_log a
        USING batch
        WHERE a.id = batch.id
        "#,
    )
    .bind(days as i32)
    .bind(batch_size as i64)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}
