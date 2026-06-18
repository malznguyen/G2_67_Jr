//! gmrag-worker — background job runner library.
//!
//! T34: skeleton + Redis BRPOP poll loop. T43: full ingestion pipeline
//! (`IngestContext::process_job`) + retry wrapper (`process_job_with_retry`).
//!
//! Pool rule (per project invariant): the worker uses `init_app_pool`
//! (`gmrag_app` role, RLS enforced) for all business queries and sets
//! `SET LOCAL app.tenant_id = $job.tenant_id` per job. `admin_pool` is
//! never used for business logic.

pub mod chunking;
pub mod embedding;
pub mod graph;
pub mod job;
pub mod ocr;
pub mod pdf_parser;
pub mod qdrant_writer;
pub mod queue;
pub mod storage;

pub use chunking::{ChunkError, chunk_page_texts};
pub use embedding::{
    EmbedError, Embedder, OpenAiEmbedder, OllamaEmbedder, TenantLlmConfig, select_embedder,
};
pub use graph::{
    DeepSeekGraphExtractor, ExtractedEdge, ExtractedNode, GraphExtractError, GraphExtraction,
    GraphExtractor, parse_graph_json, select_graph_extractor,
};
pub use job::{IngestContext, IngestJob, JobRunner, MAX_ATTEMPTS, process_job_with_retry};
pub use ocr::{MockOcr, NoOcr, OcrClient, OcrError, OllamaVisionOcr};
pub use pdf_parser::{
    ExtractionMethod, MockRenderer, PageRenderer, ParsedDocument, PdfParseError, RenderError,
    parse_pdf, parse_pdf_with_ocr,
};
pub use qdrant_writer::{DualWriteInput, DualWriteResult, IngestError, dual_write_ingestion};
pub use queue::{JobQueue, MockQueue, RedisQueue, poll_once};
pub use storage::S3Client;

use anyhow::Context as _;
use gmrag_core::Config;
use tracing::info;

/// Boot the worker: load config, build the ingest context (app_pool +
/// Qdrant + S3), connect Redis, enter the poll loop.
///
/// Each polled job is run through [`process_job_with_retry`], which updates
/// `ingest_jobs.status` (processing → completed/failed) and never propagates
/// a job error to the loop (so a failing job does not crash the worker).
pub async fn run() -> anyhow::Result<()> {
    let cfg = Config::from_env().context("loading application config")?;
    info!(service = "gmrag-worker", "gmrag-worker starting");

    let ctx = IngestContext::from_config(&cfg)
        .await
        .context("building ingest context")?;
    info!("ingest context ready (app_pool + qdrant + s3)");

    let mut queue = RedisQueue::connect(&cfg.redis.url).await?;
    info!(redis_url = %cfg.redis.url, "redis connected, polling for jobs");

    loop {
        tokio::select! {
            biased;
            _ = tokio::signal::ctrl_c() => {
                info!("gmrag-worker shutting down");
                break;
            }
            res = poll_once(&mut queue) => {
                match res? {
                    Some(job) => {
                        info!(job_id = %job.id, tenant_id = %job.tenant_id, "processing job");
                        if let Err(e) = process_job_with_retry(&ctx, &ctx.pool, &job).await {
                            tracing::error!(job_id = %job.id, error = %e, "retry wrapper failed (db)");
                        }
                    }
                    None => {
                        // BRPOP timeout — loop continues.
                    }
                }
            }
        }
    }
    Ok(())
}
