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
pub mod reconcile_loop;
pub mod relay;
pub mod retention;
pub mod storage;
pub mod sweeper;

pub use chunking::{chunk_page_texts, chunk_page_texts_with_pages, Chunk, ChunkError};
pub use embedding::{
    select_embedder, EmbedError, Embedder, OllamaEmbedder, OpenAiEmbedder, TenantLlmConfig,
};
pub use graph::{
    parse_graph_json, select_graph_extractor, DeepSeekGraphExtractor, ExtractedEdge, ExtractedNode,
    GraphExtractError, GraphExtraction, GraphExtractor,
};
pub use job::{process_job_with_retry, IngestContext, IngestJob, JobRunner, MAX_ATTEMPTS};
// Phase 1: `run_dispatcher`, `JobHandler`, `JobFut`, `DispatcherOutcome`
// are defined in this file and `pub`-scoped, so they're part of the crate
// root API automatically (no self:: reexport needed).
pub use ocr::{MockOcr, NoOcr, OcrClient, OcrError, OllamaVisionOcr};
pub use pdf_parser::{
    parse_pdf, parse_pdf_for_ingest, parse_pdf_with_ocr, ExtractionMethod, MockRenderer,
    PageRenderer, PageText, ParsedDocument, PdfParseError, RenderError,
};
pub use qdrant_writer::{dual_write_ingestion, DualWriteInput, DualWriteResult, IngestError};
pub use queue::{poll_once, JobQueue, MockQueue, RedisQueue};
pub use relay::{
    relay_outbox_once, relay_outbox_once_with_limit, DEFAULT_BATCH_SIZE as RELAY_DEFAULT_BATCH_SIZE,
};
pub use storage::S3Client;
pub use sweeper::{
    requeue_stuck_jobs, requeue_stuck_jobs_with_limit, SweeperPayload, DEFAULT_LEASE_SECS,
};

pub use reconcile_loop::{run_reconcile_loop, RealReconcileRunner, ReconcileRunner};
pub use retention::{
    delete_audit_older_than, delete_dispatched_outbox_older_than, delete_usage_older_than,
    run_retention_once,
};

use anyhow::Context as _;
use axum::extract::State;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use gmrag_core::Config;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tracing::info;

// `poll_once` is re-exported above (pub use) and used inside run_dispatcher.

/// Boxed future returned by a [`JobHandler`] — concrete enough to be passed
/// across spawn boundaries, opaque enough to swap the real pipeline for a
/// mock in tests.
pub type JobFut = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// Shared, cloneable handler invoked for each popped ingest job. `run()`
/// installs the real retry-wrapped pipeline; tests install a counter mock.
pub type JobHandler = Arc<dyn Fn(IngestJob) -> JobFut + Send + Sync>;

/// Outcome reported by [`run_dispatcher`] so callers/tests can confirm that
/// shutdown drained in-flight jobs (no silent drops).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DispatcherOutcome {
    /// Total jobs popped from the queue before shutdown.
    pub jobs_popped: usize,
    /// Jobs whose handler future resolved (Ok or Err) before dispatch ended.
    /// Equal to `jobs_popped` after a graceful drain.
    pub jobs_finished: usize,
}

/// Run a bounded pool of concurrent job processors over a [`JobQueue`].
///
/// `run_dispatcher` pops jobs with `BRPOP` (via [`poll_once`]) in a single
/// producer loop. For each popped job it spawns a worker task gated by a
/// `Semaphore` of `concurrency` permits, so at most `concurrency` job
/// handlers run at any time. When `shutdown` resolves, the dispatcher
/// stops accepting new jobs and waits for in-flight handlers to finish
/// before returning — a job that has already started is never silently
/// dropped.
///
/// This is the testable unit behind the worker's `run()`: callers inject any
/// `JobQueue` (e.g. `MockQueue` in tests) and any [`JobHandler`] (real
/// pipeline in `run()`, mock counters in tests). The `concurrency` value
/// is clamped to ≥ 1.
pub async fn run_dispatcher<Q>(
    mut queue: Q,
    concurrency: usize,
    shutdown: impl Future<Output = ()> + Send,
    handler: JobHandler,
) -> anyhow::Result<DispatcherOutcome>
where
    Q: crate::queue::JobQueue + Send,
{
    let pool_size = concurrency.max(1);
    let sem = Arc::new(Semaphore::new(pool_size));
    let mut workers: JoinSet<()> = JoinSet::new();
    let mut popped = 0usize;
    let mut finished = 0usize;
    tokio::pin!(shutdown);

    info!(concurrency = pool_size, "job dispatcher started");

    loop {
        tokio::select! {
            biased;
            _ = &mut shutdown => {
                info!(in_flight = workers.len(), "dispatcher shutdown: draining in-flight jobs");
                while let Some(res) = workers.join_next().await {
                    if res.is_ok() {
                        finished += 1;
                    }
                }
                info!(finished, "dispatcher drained; exiting");
                return Ok(DispatcherOutcome { jobs_popped: popped, jobs_finished: finished });
            }
            res = poll_once(&mut queue) => {
                match res? {
                    Some(job) => {
                        popped += 1;
                        info!(job_id = %job.id, tenant_id = %job.tenant_id, "processing job");
                        let h = handler.clone();
                        let sem = sem.clone();
                        workers.spawn(async move {
                            // Wait for a permit — this bounds concurrency to
                            // `pool_size`. If the semaphore is closed (only
                            // happens on dispatcher teardown in future tweaks)
                            // we exit without running the handler.
                            let _permit = match sem.acquire_owned().await {
                                Ok(p) => p,
                                Err(_) => return,
                            };
                            (h)(job).await;
                            // _permit drops here → release the slot.
                        });
                    }
                    None => {
                        // BRPOP timeout — the real Redis BRPOP blocks for
                        // POLL_TIMEOUT_SECS; the MockQueue returns `None`
                        // instantly, so sleep a short beat here to avoid a
                        // busy spin and let spawned worker tasks + shutdown
                        // get runtime time.
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    }
                }
            }
        }
    }
}

/// Boot the worker: load config, build the ingest context (app_pool +
/// Qdrant + S3), connect Redis, enter the poll loop.
///
/// Each polled job is run through [`process_job_with_retry`], which updates
/// `ingest_jobs.status` (processing → completed/failed) and never propagates
/// a job error to the loop (so a failing job does not crash the worker).
///
/// Phase 1: the poll loop is driven by [`run_dispatcher`], which runs up to
/// `cfg.worker_concurrency` (`GMRAG_WORKER_CONCURRENCY`, default 4) jobs
/// concurrently and drains them gracefully on Ctrl-C.
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

    let queue = RedisQueue::connect(&cfg.redis.url).await?;
    info!(redis_url = %cfg.redis.url, "redis connected, polling for jobs");

    spawn_metrics_server(cfg.worker_metrics_bind, ctx.admin_pool.clone());

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
            if let Err(e) = relay::relay_outbox_once(&admin_pool_for_relay, &mut relay_queue).await
            {
                tracing::warn!(error = %e, "outbox relay pass failed");
            }
            tokio::time::sleep(relay_interval).await;
        }
    });
    info!(
        poll_interval_secs = cfg.outbox_poll_interval_secs,
        "outbox relay task spawned (admin_pool)"
    );

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
    info!(
        sweep_interval_secs = cfg.sweep_interval_secs,
        "sweeper task spawned (admin_pool)"
    );

    // Phase 0 TASK-P0-04: retention loop (admin_pool, cross-tenant
    // maintenance). Deletes dispatched outbox rows > outbox_retention_days,
    // usage_events > usage_retention_days, and audit_log >
    // audit_retention_days, in bounded batches of retention_batch_size. The
    // loop sleeps OUTSIDE any transaction; duplicate runs across replicas
    // are harmless (idempotent bounded deletes).
    let admin_pool_for_retention = ctx.admin_pool.clone();
    let retention_interval = std::time::Duration::from_secs(cfg.retention_interval_secs);
    let outbox_days = cfg.outbox_retention_days;
    let usage_days = cfg.usage_retention_days;
    let audit_days = cfg.audit_retention_days;
    let retention_batch = cfg.retention_batch_size;
    tokio::spawn(async move {
        loop {
            if let Err(e) = retention::run_retention_once(
                &admin_pool_for_retention,
                outbox_days,
                usage_days,
                audit_days,
                retention_batch,
            )
            .await
            {
                tracing::warn!(error = %e, "retention pass failed");
            }
            tokio::time::sleep(retention_interval).await;
        }
    });
    info!(
        retention_interval_secs = cfg.retention_interval_secs,
        outbox_retention_days = cfg.outbox_retention_days,
        usage_retention_days = cfg.usage_retention_days,
        audit_retention_days = cfg.audit_retention_days,
        retention_batch_size = cfg.retention_batch_size,
        "retention task spawned (admin_pool)",
    );

    // Phase 3: cross-system drift reconciler background loop (admin_pool,
    // cross-tenant). Runs OpenFGA + Qdrant reconciliation every
    // GMRAG_RECONCILE_INTERVAL_SECS. Auto-fix defaults to OFF
    // (GMRAG_RECONCILE_AUTO_FIX=false) — in that mode the loop only
    // logs/reports and never writes or deletes. The loop honors the same
    // Ctrl-C shutdown as the dispatcher so an in-progress run is not
    // abandoned; it is spawned separately so a slow reconcile pass can never
    // block job dispatch.
    let reconcile_runner: std::sync::Arc<reconcile_loop::RealReconcileRunner> =
        std::sync::Arc::new(reconcile_loop::RealReconcileRunner {
            pool: ctx.admin_pool.clone(),
            qdrant: ctx.qdrant.clone(),
            authz: std::sync::Arc::new(
                gmrag_api::authz::OpenFgaAuthorizationService::new(&cfg.openfga)
                    .context("initialising openfga client for reconciler")?,
            ),
        });
    let reconcile_interval = std::time::Duration::from_secs(cfg.reconcile_interval_secs);
    let reconcile_auto_fix = cfg.reconcile_auto_fix;
    let runner_for_loop = reconcile_runner.clone();
    tokio::spawn(async move {
        let shutdown = async {
            if let Err(e) = tokio::signal::ctrl_c().await {
                tracing::warn!(error = %e, "reconcile loop ctrl_c handler error");
            }
        };
        run_reconcile_loop(
            runner_for_loop,
            reconcile_interval,
            reconcile_auto_fix,
            shutdown,
        )
        .await;
    });
    info!(
        reconcile_interval_secs = cfg.reconcile_interval_secs,
        reconcile_auto_fix = cfg.reconcile_auto_fix,
        "reconcile loop task spawned (admin_pool)",
    );

    // Phase 1: dispatch up to `worker_concurrency` jobs concurrently.
    // The handler runs the full retry-wrapped pipeline per popped job; the
    // dispatcher drains in-flight jobs on Ctrl-C instead of abandoning them.
    let worker_concurrency = cfg.worker_concurrency;
    info!(
        worker_concurrency,
        "main job loop running with bounded concurrency (GMRAG_WORKER_CONCURRENCY)"
    );
    let ctx_for_handler = Arc::new(ctx);
    let handler: JobHandler = Arc::new(move |job: IngestJob| {
        let ctx = ctx_for_handler.clone();
        Box::pin(async move {
            if let Err(e) = process_job_with_retry(&*ctx, &ctx.pool, &job).await {
                tracing::error!(job_id = %job.id, error = %e, "retry wrapper failed (db)");
            }
        })
    });
    let shutdown = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::warn!(error = %e, "ctrl_c signal handler error; dispatcher will run until queue drains");
        }
        info!("gmrag-worker shutting down");
    };
    run_dispatcher(queue, worker_concurrency, shutdown, handler).await?;
    Ok(())
}

#[derive(Clone)]
struct WorkerMetricsState {
    pool: sqlx::PgPool,
}

fn spawn_metrics_server(bind: std::net::SocketAddr, pool: sqlx::PgPool) {
    tokio::spawn(async move {
        let app = worker_metrics_app(pool);
        match tokio::net::TcpListener::bind(bind).await {
            Ok(listener) => {
                info!(addr = %bind, "worker metrics listener started");
                if let Err(e) = axum::serve(listener, app).await {
                    tracing::warn!(error = %e, "worker metrics listener stopped");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, addr = %bind, "worker metrics listener bind failed");
            }
        }
    });
}

pub fn worker_metrics_app(pool: sqlx::PgPool) -> Router {
    Router::new()
        .route("/metrics", get(worker_metrics_endpoint))
        .with_state(WorkerMetricsState { pool })
}

async fn worker_metrics_endpoint(State(state): State<WorkerMetricsState>) -> impl IntoResponse {
    if let Err(e) = gmrag_api::metrics::refresh_ingest_job_metrics(&state.pool).await {
        tracing::warn!(error = %e, "refresh worker ingest job metrics failed");
    }
    (
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        gmrag_api::metrics::render_prometheus(),
    )
}
