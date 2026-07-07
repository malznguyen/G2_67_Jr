//! Phase 3 — standalone cross-system drift reconciliation ops binary.
//!
//! Runs the OpenFGA and Qdrant reconcilers against the live stack and prints
//! a structured JSON report. Default mode is **dry-run / report-only** — it
//! never writes or deletes anything unless `--fix` is passed. Optional
//! `--only openfga|qdrant` limits scope to one subsystem.
//!
//! ```text
//! cargo run -p gmrag-api --bin reconcile-drift                 # dry-run both
//! cargo run -p gmrag-api --bin reconcile-drift -- --fix        # auto-fix both
//! cargo run -p gmrag-api --bin reconcile-drift -- --only qdrant
//! ```

use anyhow::Context as _;
use gmrag_api::authz::{AuthorizationService, OpenFgaAuthorizationService};
use gmrag_api::reconcile::{run_openfga_reconcile, run_qdrant_reconcile};
use gmrag_core::{init_pool, Config, QdrantStore};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let auto_fix = args.iter().any(|a| a == "--fix");
    let only = args
        .iter()
        .find_map(|a| a.strip_prefix("--only=").map(|s| s.to_string()));
    if let Some(scope) = &only {
        anyhow::ensure!(
            scope == "openfga" || scope == "qdrant",
            "--only must be 'openfga' or 'qdrant', got '{scope}'"
        );
    }

    let cfg = Config::from_env().context("loading application config")?;
    let pool = init_pool(&cfg.database_url)
        .await
        .context("initialising postgres pool")?;
    let qdrant = QdrantStore::new(&cfg.qdrant)
        .await
        .context("initialising qdrant store")?;
    let authz = OpenFgaAuthorizationService::new(&cfg.openfga)
        .context("initialising openfga authorization client")?;
    authz.health().await.context("checking openfga readiness")?;

    eprintln!(
        "reconcile-drift: mode={} subsystem={}",
        if auto_fix { "FIX" } else { "DRY-RUN" },
        only.as_deref().unwrap_or("both")
    );

    let mut out = serde_json::Map::new();
    out.insert("auto_fix".into(), serde_json::Value::Bool(auto_fix));
    out.insert(
        "mode".into(),
        serde_json::Value::String(if auto_fix {
            "fix".into()
        } else {
            "dry-run".into()
        }),
    );

    if only.as_deref() != Some("qdrant") {
        let report = run_openfga_reconcile(&pool, &authz, auto_fix)
            .await
            .context("openfga reconcile")?;
        out.insert("openfga".into(), serde_json::to_value(&report)?);
        eprintln!(
            "openfga: missing={} orphaned={} malformed={} written={} deleted={}",
            report.missing_in_openfga.count,
            report.orphaned_in_openfga.count,
            report.malformed.count,
            report.written,
            report.deleted
        );
    }
    if only.as_deref() != Some("openfga") {
        let report = run_qdrant_reconcile(&pool, &qdrant, auto_fix)
            .await
            .context("qdrant reconcile")?;
        out.insert("qdrant".into(), serde_json::to_value(&report)?);
        eprintln!(
            "qdrant: orphaned_chunks={} orphaned_graph={} missing_chunks={} missing_graph={} deleted_chunk_docs={} deleted_graph_nodes={}",
            report.orphaned_chunk_points.count,
            report.orphaned_graph_points.count,
            report.missing_chunk_points.count,
            report.missing_graph_points.count,
            report.deleted_chunk_docs,
            report.deleted_graph_nodes
        );
    }

    // Pretty-print the full structured report to stdout (the human-readable
    // per-category summary above goes to stderr so logs and report are
    // separable).
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::Value::Object(out))?
    );
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
