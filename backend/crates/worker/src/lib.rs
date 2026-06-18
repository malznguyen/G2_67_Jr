//! gmrag-worker — background job runner library.
//!
//! T34: worker crate skeleton + Redis BRPOP poll loop.
//! `process_job` is a stub — real ingestion pipeline lands in T37+.
//!
//! Pool rule (per project invariant): the worker currently uses `init_pool`
//! (admin / `gmrag` superuser) only for the boot-time liveness check.
//! **When T37+ implements `process_job` with Postgres dual-write, it MUST
//! switch to `init_app_pool` (`gmrag_app` role) + `SET LOCAL
//! app.tenant_id = $job.tenant_id` per job.** Using `admin_pool` for
//! business queries would bypass RLS and cause a data leak.

pub mod job;
pub mod pdf_parser;
pub mod queue;
pub mod storage;
pub mod embedding;

pub use embedding::{EmbedError, OllamaEmbedder};
pub use job::{IngestJob, process_job};
pub use pdf_parser::{ExtractionMethod, ParsedDocument, PdfParseError, parse_pdf};
pub use queue::{JobQueue, MockQueue, RedisQueue, poll_once};
pub use storage::S3Client;

use anyhow::Context as _;
use gmrag_core::{Config, init_pool};
use tracing::info;

/// Boot the worker: load config, init pools, connect Redis, enter poll loop.
///
/// The loop runs until `ctrl_c` (SIGTERM). Each iteration either processes
/// a job (currently a stub) or times out and continues.
pub async fn run() -> anyhow::Result<()> {
    let cfg = Config::from_env().context("loading application config")?;
    info!(service = "gmrag-worker", "gmrag-worker starting");

    // Boot-time liveness check only — no business queries on this pool.
    // T37+ dual-write MUST use init_app_pool (gmrag_app role) + SET LOCAL
    // app.tenant_id per job.
    let _pool = init_pool(&cfg.database_url)
        .await
        .context("initialising postgres pool")?;
    info!("postgres pool ready (worker)");

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
                        if let Err(e) = process_job(&job).await {
                            tracing::error!(job_id = %job.id, error = %e, "job failed");
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
