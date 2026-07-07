//! Redis-backed job queue with BRPOP poll loop.
//!
//! T34: defines a `JobQueue` trait so the poll loop can be tested with an
//! in-memory `MockQueue` (no live Redis required). `RedisQueue` wraps a
//! single `redis::aio::Connection` and issues `BRPOP gmrag:ingest_jobs 5`
//! (blocking, 5-second timeout). On timeout the call returns `None` and the
//! loop continues; on success the raw bytes are deserialized into an
//! [`IngestJob`](crate::job::IngestJob).

use std::collections::VecDeque;
use std::sync::Mutex;

use anyhow::Context as _;
use redis::AsyncCommands;

use crate::job::IngestJob;

/// Redis list key for ingest jobs.
pub const INGEST_JOBS_KEY: &str = "gmrag:ingest_jobs";

/// BRPOP timeout in seconds. When no job arrives within this window the call
/// returns `None` and the loop iterates, giving `tokio::select!` a chance to
/// observe `ctrl_c`.
pub const POLL_TIMEOUT_SECS: u64 = 5;

/// Abstract queue backend — allows `MockQueue` in tests.
#[async_trait::async_trait]
pub trait JobQueue: Send {
    /// Blocking pop with timeout. Returns `Ok(None)` on timeout, `Ok(Some(bytes))`
    /// when a job is available.
    async fn brpop_timeout(
        &mut self,
        key: &str,
        timeout_secs: u64,
    ) -> anyhow::Result<Option<Vec<u8>>>;

    /// T84D Phase 1.1: non-blocking push used by the outbox relay to
    /// dispatch pending payloads onto `gmrag:ingest_jobs`. The default
    /// impl returns `Err("not implemented")` so backends that never push
    /// (none in production) stay trivial.
    async fn lpush(&mut self, key: &str, payload: Vec<u8>) -> anyhow::Result<()> {
        let _ = (key, payload);
        Err(anyhow::anyhow!("lpush not implemented for this JobQueue"))
    }
}

/// Single-connection Redis queue backed by `BRPOP`.
///
/// Uses `MultiplexedConnection` (the recommended async connection type in
/// redis 0.25+). BRPOP with a finite timeout is safe on a multiplexed
/// connection — the worker issues no other concurrent commands on it.
pub struct RedisQueue {
    conn: redis::aio::MultiplexedConnection,
}

impl RedisQueue {
    /// Open a Redis connection from a `redis://…` URL.
    pub async fn connect(url: &str) -> anyhow::Result<Self> {
        let client = redis::Client::open(url).context("redis client open")?;
        let conn = client
            .get_multiplexed_async_connection()
            .await
            .context("redis connect")?;
        Ok(Self { conn })
    }
}

#[async_trait::async_trait]
impl JobQueue for RedisQueue {
    async fn brpop_timeout(
        &mut self,
        key: &str,
        timeout_secs: u64,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        let result: Option<(String, Vec<u8>)> = redis::cmd("BRPOP")
            .arg(key)
            .arg(timeout_secs)
            .query_async(&mut self.conn)
            .await
            .map_err(|e| anyhow::anyhow!("redis BRPOP error: {e}"))?;
        Ok(result.map(|(_key, value)| value))
    }

    async fn lpush(&mut self, key: &str, payload: Vec<u8>) -> anyhow::Result<()> {
        let mut conn = self.conn.clone();
        conn.lpush::<_, _, ()>(key, payload)
            .await
            .map_err(|e| anyhow::anyhow!("redis LPUSH '{key}': {e}"))?;
        Ok(())
    }
}

/// In-memory mock queue for tests — no live Redis required.
///
/// `brpop_timeout` pops the front item immediately. When the deque is empty
/// it returns `Ok(None)` (simulating a BRPOP timeout).
///
/// `lpush` prepends to the deque (mirrors Redis LPUSH semantics). Pushed
/// payloads are also recorded in `pushed` so the outbox relay test can
/// assert what was LPUSHed without consuming the deque.
pub struct MockQueue {
    items: Mutex<VecDeque<Vec<u8>>>,
    pushed: Mutex<Vec<Vec<u8>>>,
}

impl MockQueue {
    pub fn new(items: Vec<Vec<u8>>) -> Self {
        Self {
            items: Mutex::new(items.into()),
            pushed: Mutex::new(Vec::new()),
        }
    }

    /// Snapshot of every payload ever LPUSHed since the queue was constructed
    /// (in insertion order). Used by the outbox relay test.
    pub fn pushed(&self) -> Vec<Vec<u8>> {
        self.pushed
            .lock()
            .expect("mock queue mutex poisoned")
            .clone()
    }
}

#[async_trait::async_trait]
impl JobQueue for MockQueue {
    async fn brpop_timeout(
        &mut self,
        _key: &str,
        _timeout_secs: u64,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        let mut items = self.items.lock().expect("mock queue mutex poisoned");
        Ok(items.pop_front())
    }

    async fn lpush(&mut self, key: &str, payload: Vec<u8>) -> anyhow::Result<()> {
        if key != INGEST_JOBS_KEY {
            return Err(anyhow::anyhow!("unexpected lpush key '{key}'"));
        }
        {
            let mut items = self.items.lock().expect("mock queue mutex poisoned");
            items.push_front(payload.clone());
        }
        self.pushed
            .lock()
            .expect("mock queue pushed poisoned")
            .push(payload);
        Ok(())
    }
}

/// Poll the queue once. Returns `Ok(Some(job))` when a job is available,
/// `Ok(None)` on timeout (queue empty / BRPOP timed out).
///
/// This is the testable unit — it accepts any `JobQueue` implementation,
/// so tests inject `MockQueue` without touching real Redis.
pub async fn poll_once<Q: JobQueue + Send>(queue: &mut Q) -> anyhow::Result<Option<IngestJob>> {
    let raw = queue
        .brpop_timeout(INGEST_JOBS_KEY, POLL_TIMEOUT_SECS)
        .await?;
    match raw {
        Some(data) => {
            let job: IngestJob = serde_json::from_slice(&data)
                .map_err(|e| anyhow::anyhow!("failed to deserialize ingest job: {e}"))?;
            Ok(Some(job))
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_job_json() -> Vec<u8> {
        r#"{
            "id": "550e8400-e29b-41d4-a716-446655440000",
            "tenant_id": "660e8400-e29b-41d4-a716-446655440000",
            "workspace_id": "770e8400-e29b-41d4-a716-446655440000",
            "document_id": "880e8400-e29b-41d4-a716-446655440000",
            "s3_key": "tenant-66/document-88/report.pdf",
            "filename": "report.pdf",
            "owner_id": "990e8400-e29b-41d4-a716-446655440000",
            "visibility": "private",
            "attempts": 0
        }"#
        .as_bytes()
        .to_vec()
    }

    #[tokio::test]
    async fn poll_once_returns_job_from_mock_queue() {
        let mut q = MockQueue::new(vec![sample_job_json()]);
        let job = poll_once(&mut q).await.expect("poll should succeed");
        assert!(job.is_some(), "should receive a job");
        let job = job.unwrap();
        assert_eq!(job.filename, "report.pdf");
    }

    #[tokio::test]
    async fn poll_once_returns_none_on_empty_queue() {
        let mut q = MockQueue::new(vec![]);
        let job = poll_once(&mut q).await.expect("poll should succeed");
        assert!(job.is_none(), "empty queue should return None");
    }

    #[tokio::test]
    async fn poll_once_drains_jobs_in_fifo_order() {
        let mut q = MockQueue::new(vec![sample_job_json(), {
            let json = sample_job_json();
            let s = String::from_utf8(json)
                .unwrap()
                .replace("report.pdf", "second.pdf");
            s.into_bytes()
        }]);
        let first = poll_once(&mut q).await.unwrap().unwrap();
        assert_eq!(first.filename, "report.pdf");
        let second = poll_once(&mut q).await.unwrap().unwrap();
        assert_eq!(second.filename, "second.pdf");
        let third = poll_once(&mut q).await.unwrap();
        assert!(third.is_none(), "queue should be empty after 2 pops");
    }
}
