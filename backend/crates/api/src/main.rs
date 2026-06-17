//! gmrag-api — HTTP entry point.
//!
//! Boot sequence:
//!   1. Initialise tracing subscriber with the env-driven filter.
//!   2. Load `Config` from environment (fail fast on missing DATABASE_URL, ...).
//!   3. Initialise TWO Postgres pools:
//!        - `admin_pool` via `init_pool`  — superuser `gmrag`, bypasses RLS.
//!          Used for migrations, provisioning, cross-tenant endpoints.
//!        - `app_pool` via `init_app_pool` — every connection runs
//!          `SET ROLE gmrag_app`, so RLS policies are enforced. Used by
//!          `rls_middleware` for tenant-scoped handler queries.
//!   4. Run `sqlx::migrate!()` on the `admin_pool` (the `gmrag_app` role
//!      cannot CREATE tables).
//!   5. Build the axum router with three route groups (public `/health`,
//!      authed `/users/me`, and a tenant-scoped group wiring
//!      auth → tenant → rls middleware).
//!   6. Bind & serve with graceful shutdown on SIGINT / SIGTERM.

#![allow(dead_code)]

mod auth;
mod error;
mod middleware;
mod pool;
mod routes;

use std::time::Duration;

use anyhow::Context as _;
use auth::extractor::AuthState;
use auth::jwt::JwtValidator;
use auth::middleware::auth_middleware;
use auth::tenant::tenant_middleware;
use axum::{Extension, Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use gmrag_core::{Config, init_app_pool, init_pool};
use middleware::rls::rls_middleware;
use pool::{AdminPool, AppPool};
use serde_json::json;
use tokio::signal;
use tracing::{info, warn};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

#[derive(Clone)]
struct AppState {
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

    // 3a. Admin pool (superuser, bypasses RLS) — used for migrations +
    //     platform-level / cross-tenant operations.
    let admin_pool = init_pool(&cfg.database_url)
        .await
        .context("initialising admin postgres pool")?;
    info!("admin postgres pool ready");

    // 3b. App pool (gmrag_app role, RLS enforced) — used by rls_middleware
    //     for tenant-scoped handler queries.
    let app_pool = init_app_pool(&cfg.database_url)
        .await
        .context("initialising app postgres pool (RLS-enforced)")?;
    info!("app postgres pool ready (role=gmrag_app)");

    // 4. Migrations — MUST run on admin_pool; gmrag_app lacks CREATE.
    sqlx::migrate!("../../migrations")
        .run(&admin_pool)
        .await
        .context("running database migrations")?;
    info!("database migrations applied");

    // Auth state — JwtValidator seeded from OIDC config. JWKS is fetched
    // lazily on first token validation (see auth::jwt::JwtValidator::get_key).
    let jwt_validator = JwtValidator::new(cfg.oidc.issuer.clone(), cfg.oidc.client_id.clone());
    let auth_state = AuthState { jwt_validator };
    info!(issuer = %cfg.oidc.issuer, "auth state ready");

    // 5. Router.
    // All sub-routers share the same state type (`Router<AppState>`) so they
    // can be merged, then `.with_state(AppState)` converts the merged router
    // to `Router<()>` for serving.

    // Public group — no middleware.
    let public: Router<AppState> = Router::new()
        .route("/health", get(health))
        .route("/healthz", get(healthz));

    // Authed group — auth_middleware only (cross-tenant).
    let authed: Router<AppState> = Router::new()
        .route("/users/me", get(routes::users::get_me))
        .layer(axum::middleware::from_fn(auth_middleware));

    // Tenant-scoped group — auth → tenant → rls.
    // No route is mounted yet; the group is declared solely so the
    // middleware chain compiles and is ready for the first tenant-scoped
    // handler. The `from_fn` references below keep `tenant_middleware` and
    // `rls_middleware` "used" (no dead_code) until a real route is added.
    let tenant_scoped: Router<AppState> = Router::new()
        .layer(axum::middleware::from_fn(rls_middleware))
        .layer(axum::middleware::from_fn(tenant_middleware))
        .layer(axum::middleware::from_fn(auth_middleware));

    // Extensions shared by all groups.
    let merged: Router<AppState> = public
        .merge(authed)
        .merge(tenant_scoped)
        .layer(Extension(AppPool(app_pool.clone())))
        .layer(Extension(AdminPool(admin_pool.clone())))
        .layer(Extension(auth_state.clone()));

    let app = merged.with_state(AppState {
        started_at: chrono::Utc::now(),
    });

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

/// `GET /healthz` — readiness probe. Pings the DB via the admin pool to
/// confirm the service is ready to serve traffic. Aliased to `/health` for
/// docker-compose compat (compose file already commits to `/healthz`).
async fn healthz(Extension(AdminPool(pool)): Extension<AdminPool>) -> impl IntoResponse {
    match sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&pool)
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
        // /health must NOT touch the DB (liveness only) — no pool needed.
        let state = AppState {
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
}
