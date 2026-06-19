//! Ingest job model + full ingestion pipeline + retry wrapper.
//!
//! T34: `IngestJob` wire format. T43: `IngestContext` chains the real
//! pipeline (S3 → PDF parse → chunk → embed → graph extract → dual-write)
//! and `process_job_with_retry` wraps it with in-memory backoff + DB
//! status tracking on `ingest_jobs`.

use std::time::Duration;

use gmrag_core::config::{DeepSeekConfig, OllamaConfig};
use gmrag_core::{QdrantStore, init_app_pool};
use sqlx::PgPool;
use uuid::Uuid;

use crate::qdrant_writer::{DualWriteInput, dual_write_ingestion};
use crate::{S3Client, chunk_page_texts, parse_pdf, select_embedder, select_graph_extractor};

/// Maximum number of attempts before a job is marked `failed`.
pub const MAX_ATTEMPTS: u32 = 3;
/// Base backoff in ms (`base * 2^attempt`, capped).
const BACKOFF_BASE_MS: u64 = 1000;
/// Backoff cap — 16s, mirroring T39's `2^6 * 250ms`.
const BACKOFF_CAP_MS: u64 = 16000;
/// PDF parse timeout (seconds).
const PDF_PARSE_TIMEOUT_SECS: u64 = 30;

/// A single ingestion job dequeued from Redis (`gmrag:ingest_jobs`).
///
/// `owner_id` and `visibility` are populated by the API at enqueue time so
/// the stateless worker can build Qdrant payloads without an extra
/// `documents` lookup (per T43 design decision).
#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct IngestJob {
    pub id: uuid::Uuid,
    pub tenant_id: uuid::Uuid,
    pub workspace_id: uuid::Uuid,
    pub document_id: uuid::Uuid,
    pub s3_key: String,
    pub filename: String,
    pub owner_id: uuid::Uuid,
    pub visibility: String,
    pub attempts: u32,
}

/// Dependencies needed to run one ingestion job end-to-end.
///
/// Built once in `run()` and reused for every polled job.
pub struct IngestContext {
    pub pool: PgPool,
    pub qdrant: QdrantStore,
    pub s3: S3Client,
    pub ollama: OllamaConfig,
    pub deepseek: DeepSeekConfig,
    pub enc_key: Option<[u8; 32]>,
}

impl IngestContext {
    /// Build from app config: app_pool (RLS-enforced), Qdrant store, S3
    /// client, and LLM/embedding configs.
    pub async fn from_config(cfg: &gmrag_core::Config) -> anyhow::Result<Self> {
        let pool = init_app_pool(&cfg.database_url)
            .await
            .map_err(|e| anyhow::anyhow!("init_app_pool failed: {e}"))?;
        let qdrant = QdrantStore::new(&cfg.qdrant)
            .await
            .map_err(|e| anyhow::anyhow!("QdrantStore::new failed: {e}"))?;
        let s3 = S3Client::new(&cfg.s3);
        Ok(Self {
            pool,
            qdrant,
            s3,
            ollama: cfg.ollama.clone(),
            deepseek: cfg.deepseek.clone(),
            enc_key: cfg.tenant_key_encryption_key,
        })
    }
}

/// Pluggable job runner so the retry wrapper can be tested with a mock
/// instead of the full live pipeline (which needs S3 + Qdrant + Ollama +
/// DeepSeek). `IngestContext` implements this with the real pipeline.
#[async_trait::async_trait]
pub trait JobRunner: Send + Sync {
    /// Run one attempt. `Err(msg)` signals a transient failure to retry.
    async fn run(&self, job: &IngestJob) -> Result<(), String>;
}

#[async_trait::async_trait]
impl JobRunner for IngestContext {
    async fn run(&self, job: &IngestJob) -> Result<(), String> {
        self.process_job(job).await
    }
}

impl IngestContext {
    /// The full ingestion pipeline for a single job.
    ///
    /// S3 download → PDF parse (text path) → chunk → embed chunks →
    /// graph extract → embed node descriptions → idempotent dual-write.
    /// Any error is returned as `Err(String)` so the retry wrapper can
    /// record it and back off.
    pub async fn process_job(&self, job: &IngestJob) -> Result<(), String> {
        // 1. Download the document from S3.
        let bytes = self
            .s3
            .download(&job.s3_key)
            .await
            .map_err(|e| format!("s3 download: {e}"))?;

        // 2. Parse PDF (text path; OCR fallback wiring lands with
        //    PdfiumRenderer — see T37 blocker).
        let parsed = parse_pdf(bytes, PDF_PARSE_TIMEOUT_SECS)
            .await
            .map_err(|e| format!("pdf parse: {e}"))?;
        let full_text = parsed.text.clone();
        let page_texts = vec![parsed.text];

        // 3. Chunk.
        let chunks = chunk_page_texts(&page_texts)
            .map_err(|e| format!("chunking: {e}"))?;

        // 4. Embed chunks (per-tenant BYOK or Ollama).
        let embedder = select_embedder(&self.pool, job.tenant_id, &self.ollama, self.enc_key.as_ref())
            .await
            .map_err(|e| format!("select_embedder: {e}"))?;
        let chunk_vectors = embedder
            .embed_batch(&chunks)
            .await
            .map_err(|e| format!("embed chunks: {e}"))?;

        // 5. Graph extract via DeepSeek (global) or tenant BYOK LLM.
        let extractor = select_graph_extractor(&self.pool, job.tenant_id, &self.deepseek, self.enc_key.as_ref())
            .await
            .map_err(|e| format!("select_graph_extractor: {e}"))?;
        let extraction = extractor
            .extract(&full_text)
            .await
            .map_err(|e| format!("graph extract: {e}"))?;

        // 6. Embed node descriptions (same embedder → shared semantic space).
        let node_texts: Vec<String> = extraction
            .nodes
            .iter()
            .map(|n| {
                if n.description.trim().is_empty() {
                    n.label.clone()
                } else {
                    n.description.clone()
                }
            })
            .collect();
        let node_vectors = if node_texts.is_empty() {
            Vec::new()
        } else {
            embedder
                .embed_batch(&node_texts)
                .await
                .map_err(|e| format!("embed nodes: {e}"))?
        };

        // 7. Idempotent dual-write (Postgres metadata + Qdrant vectors).
        let input = DualWriteInput {
            tenant_id: job.tenant_id,
            workspace_id: job.workspace_id,
            document_id: job.document_id,
            owner_id: job.owner_id,
            visibility: &job.visibility,
            filename: &job.filename,
            chunks: &chunks,
            chunk_vectors,
            extraction: &extraction,
            node_vectors,
        };
        dual_write_ingestion(&self.pool, &self.qdrant, input)
            .await
            .map_err(|e| format!("dual_write: {e}"))?;

        Ok(())
    }
}

/// Run a job with up to [`MAX_ATTEMPTS`] attempts, in-memory exponential
/// backoff, and `ingest_jobs` status tracking.
///
/// - On success: `status='completed'`.
/// - On each failure: `attempts++`, `last_error` recorded, backoff sleep.
/// - After exhausting attempts: `status='failed'`.
///
/// Returns `Ok(())` once the job is either completed or marked failed
/// (so the poll loop never crashes the worker). Returns `Err` only if the
/// DB status update itself fails.
pub async fn process_job_with_retry(
    runner: &dyn JobRunner,
    pool: &PgPool,
    job: &IngestJob,
) -> anyhow::Result<()> {
    update_job_status(pool, job.tenant_id, job.id, "processing", job.attempts as i32, None)
        .await?;

    let mut last_error = String::new();
    for attempt in 0..MAX_ATTEMPTS {
        match runner.run(job).await {
            Ok(()) => {
                update_job_status(pool, job.tenant_id, job.id, "completed", attempt as i32, None)
                    .await?;
                tracing::info!(job_id = %job.id, attempt, "job completed");
                return Ok(());
            }
            Err(e) => {
                last_error = e;
                let failures = attempt + 1;
                tracing::warn!(job_id = %job.id, attempt = failures, error = %last_error, "job attempt failed");
                update_job_status(
                    pool,
                    job.tenant_id,
                    job.id,
                    "processing",
                    failures as i32,
                    Some(&last_error),
                )
                .await?;

                if failures < MAX_ATTEMPTS {
                    let delay = (BACKOFF_BASE_MS * 2_u64.pow(attempt)).min(BACKOFF_CAP_MS);
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                }
            }
        }
    }

    update_job_status(
        pool,
        job.tenant_id,
        job.id,
        "failed",
        MAX_ATTEMPTS as i32,
        Some(&last_error),
    )
    .await?;
    tracing::error!(job_id = %job.id, error = %last_error, "job marked failed after max attempts");
    Ok(())
}

/// Update `ingest_jobs` status/attempts/last_error inside an RLS-scoped tx.
async fn update_job_status(
    pool: &PgPool,
    tenant_id: Uuid,
    job_id: Uuid,
    status: &str,
    attempts: i32,
    last_error: Option<&str>,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::Executor::execute(&mut *tx, "SET LOCAL ROLE gmrag_app")
        .await
        .map_err(|e| anyhow::anyhow!("SET ROLE: {e}"))?;
    sqlx::query(&format!("SET LOCAL app.tenant_id = '{tenant_id}'"))
        .execute(&mut *tx)
        .await
        .map_err(|e| anyhow::anyhow!("SET tenant: {e}"))?;
    sqlx::query(
        r#"
        UPDATE ingest_jobs
        SET status = $1, attempts = $2, last_error = $3, updated_at = now()
        WHERE id = $4
        "#,
    )
    .bind(status)
    .bind(attempts)
    .bind(last_error)
    .bind(job_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| anyhow::anyhow!("UPDATE ingest_jobs: {e}"))?;
    tx.commit()
        .await
        .map_err(|e| anyhow::anyhow!("commit: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_job() -> IngestJob {
        IngestJob {
            id: uuid::Uuid::new_v4(),
            tenant_id: uuid::Uuid::new_v4(),
            workspace_id: uuid::Uuid::new_v4(),
            document_id: uuid::Uuid::new_v4(),
            s3_key: "uploads/doc.pdf".into(),
            filename: "doc.pdf".into(),
            owner_id: uuid::Uuid::new_v4(),
            visibility: "private".into(),
            attempts: 0,
        }
    }

    #[test]
    fn ingest_job_roundtrips_with_new_fields() {
        let job = sample_job();
        let json = serde_json::to_string(&job).expect("serialize");
        let back: IngestJob = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.id, job.id);
        assert_eq!(back.owner_id, job.owner_id);
        assert_eq!(back.visibility, "private");
    }

    #[test]
    fn ingest_job_deserializes_legacy_without_owner_visibility_fails() {
        // New fields are required — legacy payloads without owner_id /
        // visibility must be rejected so the API enqueues complete jobs.
        let json = r#"{
            "id": "550e8400-e29b-41d4-a716-446655440000",
            "tenant_id": "660e8400-e29b-41d4-a716-446655440000",
            "workspace_id": "770e8400-e29b-41d4-a716-446655440000",
            "document_id": "880e8400-e29b-41d4-a716-446655440000",
            "s3_key": "k",
            "filename": "f.pdf",
            "attempts": 0
        }"#;
        let res: Result<IngestJob, _> = serde_json::from_str(json);
        assert!(res.is_err(), "missing owner_id/visibility must fail");
    }
}
