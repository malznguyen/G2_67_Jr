//! Ingest job model + full ingestion pipeline + retry wrapper.
//!
//! T34: `IngestJob` wire format. T43: `IngestContext` chains the real
//! pipeline (S3 → PDF parse → chunk → embed → graph extract → dual-write)
//! and `process_job_with_retry` wraps it with in-memory backoff + DB
//! status tracking on `ingest_jobs`.

use std::time::Duration;

use gmrag_core::config::{DeepSeekConfig, OllamaConfig};
use gmrag_core::status::{document as doc_status, ingest_job as job_status};
use gmrag_core::{init_app_pool, init_pool, QdrantStore};
use sqlx::PgPool;
use uuid::Uuid;

use crate::qdrant_writer::{dual_write_ingestion, DualWriteInput};
use crate::{
    chunk_page_texts_with_pages, parse_pdf_for_ingest, select_embedder, select_graph_extractor,
    S3Client,
};

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
///
/// T84D Phase 1.1/1.2: also exposes an `admin_pool` (role-unscoped,
/// bypasses RLS) used by the relay + sweeper background tasks. Business
/// code in `process_job` never touches `admin_pool`; per-job queries go
/// through `pool` (`gmrag_app` role, RLS-scoped per tx).
pub struct IngestContext {
    pub pool: PgPool,
    /// Admin pool (bypasses RLS). Used ONLY by the outbox relay and the
    /// job recovery sweeper — both are cross-tenant platform maintenance
    /// ops, the explicit sanctioned exception to the "worker uses
    /// app_pool" invariant (see plan §1.2).
    pub admin_pool: PgPool,
    pub qdrant: QdrantStore,
    pub s3: S3Client,
    pub ollama: OllamaConfig,
    pub deepseek: DeepSeekConfig,
    pub enc_key: Option<[u8; 32]>,
    /// T84D Phase 1.3: OCR feature flag (`GMRAG_OCR_ENABLED`). Oxygen on
    /// the OCR pipeline; the `ocr-pdfium` Cargo feature still gates the
    /// native renderer.
    pub ocr_enabled: bool,
}

impl IngestContext {
    /// Build from app config: app_pool (RLS-enforced), Qdrant store, S3
    /// client, and LLM/embedding configs.
    pub async fn from_config(cfg: &gmrag_core::Config) -> anyhow::Result<Self> {
        let pool = init_app_pool(&cfg.database_url)
            .await
            .map_err(|e| anyhow::anyhow!("init_app_pool failed: {e}"))?;
        let admin_pool = init_pool(&cfg.database_url)
            .await
            .map_err(|e| anyhow::anyhow!("init_pool (admin) failed: {e}"))?;
        let qdrant = QdrantStore::new(&cfg.qdrant)
            .await
            .map_err(|e| anyhow::anyhow!("QdrantStore::new failed: {e}"))?;
        let s3 = S3Client::new(&cfg.s3);
        Ok(Self {
            pool,
            admin_pool,
            qdrant,
            s3,
            ollama: cfg.ollama.clone(),
            deepseek: cfg.deepseek.clone(),
            enc_key: cfg.tenant_key_encryption_key,
            ocr_enabled: cfg.ocr_enabled,
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

        // 2. Parse PDF per-page (text path by default; OCR falls in via
        //    GMRAG_OCR_ENABLED + the `ocr-pdfium` Cargo feature — Phase 1.3).
        //    When OCR is requested we hand the dispatcher an Ollama vision
        //    client built from the context's Ollama config.
        let ocr_client: Option<crate::ocr::OllamaVisionOcr> = if self.ocr_enabled {
            Some(crate::ocr::OllamaVisionOcr::new(&self.ollama))
        } else {
            None
        };
        let ocr_ref = ocr_client.as_ref().map(|c| c as &dyn crate::ocr::OcrClient);
        let (page_texts, _method) =
            parse_pdf_for_ingest(bytes, PDF_PARSE_TIMEOUT_SECS, self.ocr_enabled, ocr_ref)
                .await
                .map_err(|e| format!("pdf parse: {e}"))?;

        let full_text = page_texts
            .iter()
            .map(|p| p.text.trim())
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");

        // T84D Phase 3.1: per-page strings typed as `Vec<String>` for the
        // page-aware chunker. The chunker maps each chunk's byte range back
        // to page numbers and emits `Chunk { text, page_start, page_end }`.
        let chunk_inputs: Vec<String> = page_texts.iter().map(|p| p.text.clone()).collect();

        // 3. Chunk (page-aware — feeds Phase 3 page metadata).
        let chunks =
            chunk_page_texts_with_pages(&chunk_inputs).map_err(|e| format!("chunking: {e}"))?;

        // 4. Embed chunks (per-tenant BYOK or Ollama).
        let embedder = select_embedder(
            &self.pool,
            job.tenant_id,
            &self.ollama,
            self.enc_key.as_ref(),
        )
        .await
        .map_err(|e| format!("select_embedder: {e}"))?;
        let chunk_texts: Vec<String> = chunks.iter().map(|c| c.text.clone()).collect();
        let chunk_vectors = embedder
            .embed_batch(&chunk_texts)
            .await
            .map_err(|e| format!("embed chunks: {e}"))?;

        // 5. Graph extract via DeepSeek (global) or tenant BYOK LLM.
        let extractor = select_graph_extractor(
            &self.pool,
            job.tenant_id,
            &self.deepseek,
            self.enc_key.as_ref(),
        )
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

/// Run a job with up to [`MAX_ATTEMPTS`] total attempts, in-memory
/// exponential backoff, and `ingest_jobs` status tracking.
///
/// # Prior attempts accounting (Phase 1, Task 2)
/// A job requeued by the sweeper arrives with `job.attempts` already
/// reflecting prior real attempts (the sweeper does
/// `attempts = row.attempts + 1` at requeue time). This wrapper must NOT
/// give such a job a fresh full set of `MAX_ATTEMPTS` in-memory retries —
/// it would let a requeued job retry far past the intended cap, wasting
/// external API calls (embeddings, graph LLM). The in-memory loop is
/// therefore bounded by `MAX_ATTEMPTS - job.attempts` (the remaining
/// budget), not `0..MAX_ATTEMPTS`.
///
/// # Persisted `attempts` semantics
/// The persisted `ingest_jobs.attempts` counter counts EVERY real
/// `runner.run()` invocation (failures AND the successful attempt), so it
/// stays accurate across multiple sweeper requeues — not just one. On
/// success it equals `start_attempts + attempts_made_this_session`; on
/// final exhaustion it equals `MAX_ATTEMPTS`.
///
/// # Fail-fast at the cap
/// If `job.attempts >= MAX_ATTEMPTS` when picked up (e.g. a race with the
/// sweeper), the job is marked `failed` immediately without invoking the
/// runner — no external API calls are spent on a job that has already
/// exhausted its budget.
///
/// Returns `Ok(())` once the job is either completed or marked failed
/// (so the poll loop never crashes the worker). Returns `Err` only if the
/// DB status update itself fails.
pub async fn process_job_with_retry(
    runner: &dyn JobRunner,
    pool: &PgPool,
    job: &IngestJob,
) -> anyhow::Result<()> {
    let started = std::time::Instant::now();
    let start_attempts = job.attempts;

    // Fail-fast: a job already at/over the attempt cap (e.g. a sweeper race)
    // must not run again — mark it failed immediately.
    if start_attempts >= MAX_ATTEMPTS {
        let msg = format!("job already reached max attempts ({MAX_ATTEMPTS}); not retrying");
        update_job_status(
            pool,
            job.tenant_id,
            job.id,
            job_status::FAILED,
            start_attempts as i32,
            Some(&msg),
        )
        .await?;
        update_document_status(pool, job.tenant_id, job.document_id, doc_status::FAILED).await?;
        tracing::error!(
            job_id = %job.id,
            attempts = start_attempts,
            "job marked failed: already at/over max attempts"
        );
        gmrag_api::metrics::metrics().inc_job_outcome(
            "ingest",
            "failure",
            started.elapsed().as_secs_f64(),
        );
        return Ok(());
    }

    update_job_status(
        pool,
        job.tenant_id,
        job.id,
        job_status::PROCESSING,
        start_attempts as i32,
        None,
    )
    .await?;
    update_document_status(pool, job.tenant_id, job.document_id, doc_status::PROCESSING).await?;

    // In-memory budget: the number of NEW real attempts this session may run.
    // A fresh job (start=0) gets the full MAX_ATTEMPTS; a requeued job with
    // start=2 gets MAX_ATTEMPTS - 2 = 1 more try.
    let remaining = MAX_ATTEMPTS - start_attempts;
    let mut last_error = String::new();
    for i in 0..remaining {
        // Persisted attempts = start + (i + 1) — every real run counts.
        let attempts_after = start_attempts + i + 1;
        match runner.run(job).await {
            Ok(()) => {
                update_job_status(
                    pool,
                    job.tenant_id,
                    job.id,
                    job_status::COMPLETED,
                    attempts_after as i32,
                    None,
                )
                .await?;
                update_document_status(pool, job.tenant_id, job.document_id, doc_status::INDEXED)
                    .await?;
                tracing::info!(job_id = %job.id, attempt = attempts_after, "job completed");
                gmrag_api::metrics::metrics().inc_job_outcome(
                    "ingest",
                    "success",
                    started.elapsed().as_secs_f64(),
                );
                return Ok(());
            }
            Err(e) => {
                last_error = e;
                tracing::warn!(
                    job_id = %job.id,
                    attempt = attempts_after,
                    error = %last_error,
                    "job attempt failed"
                );
                // Persist the cumulative attempt count so it survives a
                // crash and is visible to the sweeper.
                update_job_status(
                    pool,
                    job.tenant_id,
                    job.id,
                    job_status::PROCESSING,
                    attempts_after as i32,
                    Some(&last_error),
                )
                .await?;

                if i + 1 < remaining {
                    // Backoff is indexed by the session-local attempt so it
                    // keeps growing across the in-memory retries.
                    let delay = (BACKOFF_BASE_MS * 2_u64.pow(i)).min(BACKOFF_CAP_MS);
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                }
            }
        }
    }

    update_job_status(
        pool,
        job.tenant_id,
        job.id,
        job_status::FAILED,
        MAX_ATTEMPTS as i32,
        Some(&last_error),
    )
    .await?;
    update_document_status(pool, job.tenant_id, job.document_id, doc_status::FAILED).await?;
    tracing::error!(job_id = %job.id, error = %last_error, "job marked failed after max attempts");
    gmrag_api::metrics::metrics().inc_job_outcome(
        "ingest",
        "failure",
        started.elapsed().as_secs_f64(),
    );
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
        SET status = $1,
            attempts = $2,
            last_error = $3,
            updated_at = now(),
            claimed_at = CASE WHEN $1 = 'processing' THEN now() ELSE claimed_at END
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

/// Update `documents.status` inside an RLS-scoped tx (C7 lifecycle:
/// `uploaded` → `processing` → `indexed` / `failed`).
async fn update_document_status(
    pool: &PgPool,
    tenant_id: Uuid,
    document_id: Uuid,
    status: &str,
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
        UPDATE documents
        SET status = $1, updated_at = now()
        WHERE id = $2
        "#,
    )
    .bind(status)
    .bind(document_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| anyhow::anyhow!("UPDATE documents: {e}"))?;
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
