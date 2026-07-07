//! T84D Phase 1.1 — Outbox relay: drain `ingest_outbox` → Redis LPUSH.
//!
//! The RLS middleware owns BEGIN/COMMIT, so the upload handler cannot
//! LPUSH after commit. Instead the handler inserts a row into
//! `ingest_outbox` inside its tx, and a worker relay task poll-driven by
//! [`relay_outbox_once`] runs `SELECT ... FOR UPDATE SKIP LOCKED` against
//! the pending rows and LPUSHes each payload onto `gmrag:ingest_jobs`.
//!
//! `FOR UPDATE SKIP LOCKED` makes multi-worker replicas safe (no two
//! workers LPUSH the same row).

use anyhow::Context as _;
use gmrag_core::status::ingest_outbox as outbox_status;
use sqlx::PgPool;
use uuid::Uuid;

use crate::queue::{JobQueue, INGEST_JOBS_KEY};

/// Default batch size per relay pass.
pub const DEFAULT_BATCH_SIZE: i64 = 100;

/// Drain one batch of pending `ingest_outbox` rows into the queue.
///
/// Runs `SELECT ... FOR UPDATE SKIP LOCKED` inside an admin tx so the
/// worker's app-pool invariant stays unchanged elsewhere (this admin tx
/// is the sanctioned relay exception — see `lib.rs`), then for each row:
///   1. `queue.lpush(INGEST_JOBS_KEY, payload)`
///   2. `UPDATE ingest_outbox SET status='dispatched', dispatched_at=now()`
///   3. `COMMIT`. Returns the number of rows dispatched.
pub async fn relay_outbox_once(pool: &PgPool, queue: &mut dyn JobQueue) -> anyhow::Result<usize> {
    relay_outbox_once_with_limit(pool, queue, DEFAULT_BATCH_SIZE).await
}

pub async fn relay_outbox_once_with_limit(
    pool: &PgPool,
    queue: &mut dyn JobQueue,
    limit: i64,
) -> anyhow::Result<usize> {
    let mut tx = pool.begin().await.context("begin tx")?;
    let rows: Vec<(Uuid, serde_json::Value)> = sqlx::query_as(
        r#"
        SELECT id, payload
        FROM ingest_outbox
        WHERE status = $1
        ORDER BY created_at
        LIMIT $2
        FOR UPDATE SKIP LOCKED
        "#,
    )
    .bind(outbox_status::PENDING)
    .bind(limit)
    .fetch_all(&mut *tx)
    .await
    .context("select pending outbox rows")?;

    let mut dispatched = 0usize;
    for (id, payload) in rows {
        let bytes = serde_json::to_vec(&payload).context("serialize outbox payload")?;
        queue
            .lpush(INGEST_JOBS_KEY, bytes)
            .await
            .map_err(|e| anyhow::anyhow!("relay lpush: {e}"))?;
        sqlx::query(
            r#"
            UPDATE ingest_outbox
            SET status = $1, dispatched_at = now()
            WHERE id = $2
            "#,
        )
        .bind(outbox_status::DISPATCHED)
        .bind(id)
        .execute(&mut *tx)
        .await
        .with_context(|| format!("mark outbox row {id} dispatched"))?;
        dispatched += 1;
    }

    tx.commit().await.context("commit relay tx")?;
    Ok(dispatched)
}

#[cfg(test)]
mod tests {
    #[test]
    fn relay_payload_roundtrips_ingest_job_payload() {
        let payload = serde_json::json!({
            "id": "550e8400-e29b-41d4-a716-446655440000",
            "tenant_id": "660e8400-e29b-41d4-a716-446655440000",
            "workspace_id": "770e8400-e29b-41d4-a716-446655440000",
            "document_id": "880e8400-e29b-41d4-a716-446655440000",
            "s3_key": "uploads/doc.pdf",
            "filename": "doc.pdf",
            "owner_id": "990e8400-e29b-41d4-a716-446655440000",
            "visibility": "private",
            "attempts": 0
        });
        let bytes = serde_json::to_vec(&payload).expect("serialize");
        let job: crate::job::IngestJob = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(job.filename, "doc.pdf");
        assert_eq!(job.visibility, "private");
    }
}
