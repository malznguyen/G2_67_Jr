//! gmrag-api — HTTP entry point.
//!
//! Boot sequence (T5-T7 scope, exactly the order required by the task):
//!   1. Initialise tracing subscriber with the env-driven filter.
//!   2. Load `Config` from environment (fail fast on missing DATABASE_URL, ...).
//!   3. Initialise the Postgres connection pool (`gmrag_core::init_pool`).
//!   4. Run `sqlx::migrate!()` — applies any pending migrations.
//!   5. Build the axum router with `GET /health` (liveness) and
//!      `GET /healthz` (alias kept for the docker-compose healthcheck which
//!      already commits to `/healthz`).
//!   6. Bind & serve with graceful shutdown on SIGINT / SIGTERM.

mod auth;
mod error;
mod middleware;
mod routes;

use std::time::Duration;

use anyhow::Context as _;
use axum::{Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use gmrag_core::{Config, DbPool, init_pool};
use serde_json::json;
use tokio::signal;
use tracing::{info, warn};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

#[derive(Clone)]
struct AppState {
    pool: DbPool,
    started_at: chrono::DateTime<chrono::Utc>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Tracing.
    init_tracing();

    // 2. Config.
    let cfg = Config::from_env().context("loading application config")?;
    info!(
        service = %cfg.service_name,
        bind = %cfg.bind_address(),
        "gmrag-api starting"
    );

    // 3. DB pool.
    let pool = init_pool(&cfg.database_url)
        .await
        .context("initialising postgres pool")?;
    info!("postgres pool ready");

    // 4. Migrations. The macro embeds SQL files at compile time, so the
    //    path is relative to CARGO_MANIFEST_DIR (= crates/api). Resolving
    //    `../../migrations` points at the workspace-root migrations dir,
    //    which is the shared location for all backend crates.
    sqlx::migrate!("../../migrations")
        .run(&pool)
        .await
        .context("running database migrations")?;
    info!("database migrations applied");

    let state = AppState {
        pool,
        started_at: chrono::Utc::now(),
    };

    // 5. Router.
    let app = Router::new()
        .route("/health", get(health))
        .route("/healthz", get(healthz))
        .route("/users/me", get(routes::users::get_me))
        .with_state(state);

    // 6. Serve.
    let listener = tokio::net::TcpListener::bind(cfg.http_bind)
        .await
        .with_context(|| format!("binding to {}", cfg.bind_address()))?;
    info!(addr = %cfg.bind_address(), "gmrag-api listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("axum serve")?;

    Ok(())
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

/// `GET /health` — liveness probe. Returns 200 OK with a small JSON body.
/// Does NOT touch the database — a separate readiness endpoint handles that.
async fn health(State(state): State<AppState>) -> impl IntoResponse {
    let body = json!({
        "status": "ok",
        "service": "gmrag-api",
        "uptime_ms": (chrono::Utc::now() - state.started_at).num_milliseconds(),
    });
    (StatusCode::OK, axum::Json(body))
}

/// `GET /healthz` — readiness probe. Pings the DB to confirm the service is
/// ready to serve traffic. Aliased to `/health` for docker-compose compat
/// (compose file already commits to `/healthz`).
async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    match sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&state.pool)
        .await
    {
        Ok(_) => (
            StatusCode::OK,
            axum::Json(json!({ "status": "ready", "db": "ok" })),
        )
            .into_response(),
        Err(e) => {
            warn!(error = %e, "healthz: db ping failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                axum::Json(json!({ "status": "degraded", "db": "down" })),
            )
                .into_response()
        }
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c().await.expect("install ctrl-c handler");
    };
    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => info!("ctrl-c received, shutting down"),
        _ = terminate => info!("SIGTERM received, shutting down"),
    }

    // Give in-flight requests a brief grace period.
    tokio::time::sleep(Duration::from_millis(250)).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_returns_200() {
        // Build a router with a stub pool that is never queried by /health.
        // /health must NOT touch the DB (liveness only).
        let state = AppState {
            // Constructing a real PgPool needs a DB; use sqlx::Any-free stub.
            // Easiest: build a router that only registers /health, no pool.
            pool: stub_pool().await,
            started_at: chrono::Utc::now(),
        };
        let app = Router::new()
            .route("/health", get(health))
            .with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// Build a `PgPool` that is never used by the test (pool is lazily
    /// connected on first use; `connect_lazy` succeeds without a live DB).
    async fn stub_pool() -> DbPool {
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy("postgres://stub:stub@127.0.0.1:1/stub")
            .expect("lazy pool")
    }
}
