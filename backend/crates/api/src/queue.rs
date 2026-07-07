//! Ingest-job enqueue abstraction for the API (T58).
//!
//! After the document row + `ingest_jobs` row are written, the upload
//! endpoint pushes a fully-populated [`IngestJobPayload`] onto the Redis list
//! `gmrag:ingest_jobs` (LPUSH). The worker consumes it via BRPOP
//! (`backend/crates/worker/src/queue.rs`) and deserializes it into its own
//! `IngestJob`, so the field set here MUST stay in sync with that struct —
//! in particular `owner_id` and `visibility` are required (T43 design).
//!
//! Handlers depend on the [`JobEnqueuer`] trait (injected as
//! `Extension<Arc<dyn JobEnqueuer>>`) so tests can substitute a mock that
//! records the pushed payload and can force a failure (to exercise the S3 +
//! DB rollback path) without a live Redis.

use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Redis list key for ingest jobs. Mirrors `worker::queue::INGEST_JOBS_KEY`.
pub const INGEST_JOBS_KEY: &str = "gmrag:ingest_jobs";

/// Wire payload for a single ingestion job — must match the worker's
/// `IngestJob` field-for-field (serde).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestJobPayload {
    /// Same UUID as the inserted `ingest_jobs.id` so the worker's status
    /// updates target the right row.
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

/// Enqueue operation used by the document upload endpoint.
///
/// Errors are returned as `String` to keep the trait backend-agnostic and
/// mocks trivial; the handler maps them to `ApiError` and triggers rollback.
#[async_trait::async_trait]
pub trait JobEnqueuer: Send + Sync {
    /// LPUSH the JSON-serialized job onto `gmrag:ingest_jobs`.
    async fn enqueue(&self, job: &IngestJobPayload) -> Result<(), String>;
}

/// Redis-backed enqueuer over a cloneable multiplexed connection.
#[derive(Clone)]
pub struct RedisEnqueuer {
    conn: redis::aio::MultiplexedConnection,
}

impl RedisEnqueuer {
    /// Open a Redis connection from a `redis://…` URL.
    pub async fn connect(url: &str) -> anyhow::Result<Self> {
        let client = redis::Client::open(url)?;
        let conn = client.get_multiplexed_async_connection().await?;
        Ok(Self { conn })
    }
}

#[async_trait::async_trait]
impl JobEnqueuer for RedisEnqueuer {
    async fn enqueue(&self, job: &IngestJobPayload) -> Result<(), String> {
        let payload = serde_json::to_vec(job).map_err(|e| format!("serialize ingest job: {e}"))?;
        let mut conn = self.conn.clone();
        conn.lpush::<_, _, ()>(INGEST_JOBS_KEY, payload)
            .await
            .map_err(|e| format!("redis LPUSH '{INGEST_JOBS_KEY}': {e}"))?;
        Ok(())
    }
}
