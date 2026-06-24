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
pub mod relay;
pub mod storage;
pub mod sweeper;

pub use chunking::{Chunk, ChunkError, chunk_page_texts, chunk_page_texts_with_pages};
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
    ExtractionMethod, MockRenderer, PageRenderer, PageText, ParsedDocument, PdfParseError,
    RenderError, parse_pdf, parse_pdf_for_ingest, parse_pdf_with_ocr,
};
pub use qdrant_writer::{DualWriteInput, DualWriteResult, IngestError, dual_write_ingestion};
pub use queue::{JobQueue, MockQueue, RedisQueue, poll_once};
pub use relay::{DEFAULT_BATCH_SIZE as RELAY_DEFAULT_BATCH_SIZE, relay_outbox_once,
    relay_outbox_once_with_limit};
pub use sweeper::{DEFAULT_LEASE_SECS, requeue_stuck_jobs, requeue_stuck_jobs_with_limit,
    SweeperPayload};
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
///
/// T84D Phase 1.1/1.2: also spawns two background maintenance tasks:
///   - the outbox relay (`relay_outbox_once` every `GMRAG_OUTBOX_POLL_INTERVAL_SECS`).
///   - the job recovery sweeper (`requeue_stuck_jobs` every `GMRAG_SWEEP_INTERVAL_SECS`).
///
/// Both use `admin_pool` (not the worker's app_pool). The relay SELECT-s
/// `ingest_outbox` across tenants (RLS on a per-tx basis isn't enough — the
/// rows belong to many tenants); the sweeper scans `ingest_jobs` cross-
/// tenant. This is the explicit, documented exception to the "worker uses
/// app_pool for business logic" invariant (see plan §1.2).
pub async fn run() -> anyhow::Result<()> {
    let cfg = Config::from_env().context("loading application config")?;
    info!(service = "gmrag-worker", "gmrag-worker starting");

    let ctx = IngestContext::from_config(&cfg)
        .await
        .context("building ingest context")?;
    info!("ingest context ready (app_pool + qdrant + s3)");

    let mut queue = RedisQueue::connect(&cfg.redis.url).await?;
    info!(redis_url = %cfg.redis.url, "redis connected, polling for jobs");

    // T84D Phase 1.1: outbox relay task (admin_pool, drains pending rows
    // every GMRAG_OUTBOX_POLL_INTERVAL_SECS).
    let admin_pool_for_relay = ctx.admin_pool.clone();
    let redis_url_for_relay = cfg.redis.url.clone();
    let relay_interval = std::time::Duration::from_secs(cfg.outbox_poll_interval_secs);
    tokio::spawn(async move {
        let mut relay_queue = match RedisQueue::connect(&redis_url_for_relay).await {
            Ok(q) => q,
            Err(e) => {
                tracing::error!(error = %e, "outbox relay: redis connect failed; relay disabled");
                return;
            }
        };
        loop {
            if let Err(e) =
                relay::relay_outbox_once(&admin_pool_for_relay, &mut relay_queue).await
            {
                tracing::warn!(error = %e, "outbox relay pass failed");
            }
            tokio::time::sleep(relay_interval).await;
        }
    });
    info!(poll_interval_secs = cfg.outbox_poll_interval_secs, "outbox relay task spawned (admin_pool)");

    // T84D Phase 1.2: job recovery sweeper task (admin_pool, re-enqueues
    // stuck ingest_jobs every GMRAG_SWEEP_INTERVAL_SECS).
    let admin_pool_for_sweeper = ctx.admin_pool.clone();
    let redis_url_for_sweeper = cfg.redis.url.clone();
    let sweep_interval = std::time::Duration::from_secs(cfg.sweep_interval_secs);
    tokio::spawn(async move {
        let mut sweeper_queue = match RedisQueue::connect(&redis_url_for_sweeper).await {
            Ok(q) => q,
            Err(e) => {
                tracing::error!(error = %e, "sweeper: redis connect failed; sweeper disabled");
                return;
            }
        };
        loop {
            if let Err(e) = sweeper::requeue_stuck_jobs(
                &admin_pool_for_sweeper,
                &mut sweeper_queue,
                sweeper::DEFAULT_LEASE_SECS,
            )
            .await
            {
                tracing::warn!(error = %e, "sweeper pass failed");
            }
            tokio::time::sleep(sweep_interval).await;
        }
    });
    info!(sweep_interval_secs = cfg.sweep_interval_secs, "sweeper task spawned (admin_pool)");

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
