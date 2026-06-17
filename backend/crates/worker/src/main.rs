//! gmrag-worker — background job runner.
//!
//! Scope (T5-T7): minimal binary that proves the workspace wiring and the
//! config / pool boot path shared with `gmrag-api`. No job dispatcher yet —
//! that lands with the ingest pipeline in later sprints.

use anyhow::Context as _;
use gmrag_core::{Config, init_pool};
use tracing::info;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let cfg = Config::from_env().context("loading application config")?;
    info!(
        service = "gmrag-worker",
        "gmrag-worker starting"
    );

    let _pool = init_pool(&cfg.database_url)
        .await
        .context("initialising postgres pool")?;
    info!("postgres pool ready (worker)");

    // Placeholder event loop: future tasks will plug a job dispatcher here
    // (Redis BLPOP / sqlx-backed queue / cron). For now, sleep until SIGTERM
    // so the docker-compose healthcheck `pgrep -f gmrag-worker` stays green.
    info!("gmrag-worker idle — awaiting jobs");
    tokio::signal::ctrl_c()
        .await
        .context("waiting for shutdown signal")?;
    info!("gmrag-worker shutting down");
    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new("info,gmrag_core=debug,gmrag_worker=debug"))
        .expect("default log filter is valid");

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_target(true))
        .init();
}
