//! gmrag-api — binary entry point.

use tracing_subscriber::{EnvFilter, fmt, prelude::*};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    gmrag_api::run().await
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new("info,gmrag_core=debug,gmrag_api=debug"))
        .expect("default log filter is valid");

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_target(true))
        .init();
}
