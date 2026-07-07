//! One-time OpenFGA tuple backfill/reconcile command.
//!
//! Run this before the cutover migration in environments that still have
//! `resource_acl` rows to preserve.

use std::time::Instant;

use anyhow::Context as _;
use gmrag_api::authz::{AuthorizationService, OpenFgaAuthorizationService};
use gmrag_api::reconcile::backfill as tuple_backfill;
use gmrag_core::{init_pool, Config};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

const WRITE_BATCH_SIZE: usize = 100;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    let dry_run = std::env::args().any(|arg| arg == "--dry-run");
    let started = Instant::now();

    let cfg = Config::from_env().context("loading application config")?;
    let pool = init_pool(&cfg.database_url)
        .await
        .context("initialising postgres pool")?;
    let authz = OpenFgaAuthorizationService::new(&cfg.openfga)
        .context("initialising openfga authorization client")?;
    authz.health().await.context("checking openfga readiness")?;

    // Reuse the shared tuple-derivation logic (also used by the Phase 3
    // reconciler) rather than a local copy.
    let (tuples, counts) = tuple_backfill::collect_tuples(&pool).await?;
    tracing::info!(
        tuples = tuples.len(),
        dry_run,
        elapsed_ms = started.elapsed().as_millis(),
        "collected openfga backfill tuples"
    );

    if dry_run {
        println!("dry-run: would write {} OpenFGA tuples", tuples.len());
        for (label, n) in &counts {
            println!("  {label}: {n}");
        }
        return Ok(());
    }

    for chunk in tuples.chunks(WRITE_BATCH_SIZE) {
        authz
            .write_relationships(chunk.to_vec(), Vec::new())
            .await
            .context("writing openfga tuples")?;
    }

    println!("wrote {} OpenFGA tuples", tuples.len());
    for (label, n) in &counts {
        println!("  {label}: {n}");
    }
    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new("info,gmrag_api=debug,gmrag_core=debug"))
        .expect("default log filter is valid");

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_target(true))
        .init();
}
