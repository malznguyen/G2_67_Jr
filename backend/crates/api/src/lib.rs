//! gmrag-api library — HTTP service modules and bootstrap.

pub mod auth;
pub mod authz;
pub mod chat;
pub mod error;
pub mod llm;
pub mod metering;
pub mod metrics;
pub mod middleware;
pub mod openapi;
pub mod pool;
pub mod queue;
pub mod reconcile;
pub mod roles;
pub mod routes;
pub mod storage;
pub mod vector;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use auth::extractor::AuthState;
use auth::jwt::JwtValidator;
use auth::middleware::auth_middleware;
use auth::tenant::{tenant_middleware, TenantHeaderName};
use authz::{AuthzService, OpenFgaAuthorizationService};
use axum::{
    extract::DefaultBodyLimit,
    extract::State,
    http::{header, StatusCode},
    response::IntoResponse,
    routing::{delete, get, patch, post},
    Extension, Router,
};
use gmrag_core::{init_app_pool, init_pool, Config, QdrantStore};
use middleware::rate_limit::{RedisRateLimiter, SharedRateLimiter, SseConnectionLimiter};
use middleware::rls::rls_middleware;
use pool::{AdminPool, AppPool};
use queue::RedisEnqueuer;
use routes::chat::LlmRuntime;
use serde_json::json;
use storage::S3ObjectStore;
use tokio::signal;
use tracing::{info, warn};

#[derive(Clone)]
struct AppState {
    started_at: chrono::DateTime<chrono::Utc>,
}

/// Boot and serve the HTTP API.
pub async fn run() -> anyhow::Result<()> {
    let cfg = Config::from_env().context("loading application config")?;
    info!(
        service = %cfg.service_name,
        bind = %cfg.bind_address(),
        "gmrag-api starting"
    );
    let tenant_header = TenantHeaderName::from_config(&cfg.tenant_header)
        .with_context(|| format!("invalid GMRAG_TENANT_HEADER '{}'", cfg.tenant_header))?;

    let admin_pool = init_pool(&cfg.database_url)
        .await
        .context("initialising admin postgres pool")?;
    info!("admin postgres pool ready");

    let app_pool = init_app_pool(&cfg.database_url)
        .await
        .context("initialising app postgres pool (RLS-enforced)")?;
    info!("app postgres pool ready (role=gmrag_app)");

    sqlx::migrate!("../../migrations")
        .run(&admin_pool)
        .await
        .context("running database migrations")?;
    info!("database migrations applied");

    let qdrant = QdrantStore::new(&cfg.qdrant)
        .await
        .context("initialising qdrant store")?;
    info!(qdrant_url = %cfg.qdrant.url, "qdrant store ready");

    let authz: AuthzService = Arc::new(
        OpenFgaAuthorizationService::new(&cfg.openfga)
            .context("initialising openfga authorization client")?,
    );
    authz.health().await.context("checking openfga readiness")?;
    info!(openfga_url = %cfg.openfga.api_url, "openfga authorization ready");

    // T58/T59: document upload/delete dependencies (S3, Redis, vector cleanup).
    let object_store: Arc<dyn storage::ObjectStore> = Arc::new(S3ObjectStore::new(&cfg.s3));
    info!(bucket = %cfg.s3.bucket, "s3 object store ready");

    let enqueuer: Arc<dyn queue::JobEnqueuer> = Arc::new(
        RedisEnqueuer::connect(&cfg.redis.url)
            .await
            .context("connecting redis enqueuer")?,
    );
    info!(redis_url = %cfg.redis.url, "redis enqueuer ready");
    let rate_limiter: SharedRateLimiter = Arc::new(
        RedisRateLimiter::connect(&cfg.redis.url)
            .await
            .context("connecting redis rate limiter")?,
    );
    let sse_limiter = SseConnectionLimiter::default();

    let vector_cleaner: Arc<dyn vector::VectorCleaner> = Arc::new(qdrant.clone());
    let graph_cleaner: Arc<dyn vector::GraphCleaner> = Arc::new(qdrant.clone());

    let llm_runtime = LlmRuntime {
        deepseek: cfg.deepseek.clone(),
        ollama: cfg.ollama.clone(),
        tenant_key_encryption_key: cfg.tenant_key_encryption_key,
        chat_history_limit: cfg.chat_history_limit,
    };

    let jwt_validator = JwtValidator::new(
        cfg.oidc.issuer.clone(),
        cfg.oidc.issuer_verify.clone(),
        vec![
            cfg.oidc.client_id.clone(),
            cfg.oidc.frontend_client_id.clone(),
        ],
        cfg.oidc.client_id.clone(),
    );
    let auth_state = AuthState { jwt_validator };
    info!(issuer = %cfg.oidc.issuer, "auth state ready");

    let public: Router<AppState> = Router::new()
        .route("/health", get(health))
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics_endpoint))
        .merge(openapi::swagger_router());

    let authed: Router<AppState> = Router::new()
        .route("/users/me", get(routes::users::get_me))
        .route(
            "/tenants",
            get(routes::tenants::list_tenants).post(routes::tenants::create_tenant),
        )
        .layer(axum::middleware::from_fn(
            middleware::rate_limit::rate_limit_middleware,
        ))
        .layer(axum::middleware::from_fn(auth_middleware));

    let tenant_scoped: Router<AppState> = Router::new()
        .route(
            "/tenants/:tid",
            patch(routes::tenants::update_tenant).delete(routes::tenants::delete_tenant),
        )
        .route(
            "/tenants/:tid/members",
            get(routes::tenant_members::list_members).post(routes::tenant_members::invite_member),
        )
        .route(
            "/tenants/:tid/members/:user_id",
            delete(routes::tenant_members::remove_member),
        )
        .route(
            "/tenants/:tid/workspaces",
            get(routes::workspaces::list_workspaces).post(routes::workspaces::create_workspace),
        )
        .route(
            "/tenants/:tid/workspaces/:wid",
            patch(routes::workspaces::update_workspace)
                .delete(routes::workspaces::delete_workspace),
        )
        .route(
            "/tenants/:tid/workspaces/:wid/members",
            get(routes::ws_members::list_members).post(routes::ws_members::add_member),
        )
        .route(
            "/tenants/:tid/workspaces/:wid/members/:user_id",
            delete(routes::ws_members::remove_member),
        )
        .route(
            "/tenants/:tid/documents",
            get(routes::documents::list_documents).post(routes::documents::upload_document),
        )
        .route(
            "/tenants/:tid/documents/:did",
            delete(routes::documents::delete_document),
        )
        .route(
            "/tenants/:tid/documents/:did/preview",
            get(routes::documents::preview_document),
        )
        .route(
            "/tenants/:tid/acl",
            get(routes::acl::list_grants).post(routes::acl::create_grant),
        )
        .route(
            "/tenants/:tid/acl/:grant_id",
            delete(routes::acl::revoke_grant),
        )
        .route(
            "/tenants/:tid/chat_sessions",
            get(routes::chat::list_sessions).post(routes::chat::create_session),
        )
        .route(
            "/tenants/:tid/chat_sessions/:sid",
            delete(routes::chat::delete_session),
        )
        .route(
            "/tenants/:tid/chat_sessions/:sid/messages",
            get(routes::chat::list_messages),
        )
        .route(
            "/tenants/:tid/chat_sessions/:sid/chat",
            post(routes::chat::post_chat),
        )
        .route(
            "/tenants/:tid/workspaces/:wid/graph",
            get(routes::graph::get_workspace_graph),
        )
        .route(
            "/tenants/:tid/settings/llm",
            get(routes::settings::get_llm_settings).put(routes::settings::put_llm_settings),
        )
        .route(
            "/tenants/:tid/metering/usage",
            get(routes::metering::get_usage),
        )
        .route("/tenants/:tid/quotas", get(routes::metering::get_quotas))
        .route(
            "/tenants/:tid/audit_logs",
            get(routes::metering::get_audit_logs),
        )
        // Allow large multipart document uploads (default axum limit is 2 MiB).
        .layer(DefaultBodyLimit::max(50 * 1024 * 1024))
        .layer(axum::middleware::from_fn(rls_middleware))
        .layer(axum::middleware::from_fn(
            middleware::rate_limit::rate_limit_middleware,
        ))
        .layer(axum::middleware::from_fn(tenant_middleware))
        .layer(axum::middleware::from_fn(auth_middleware));

    let merged: Router<AppState> = public
        .merge(authed)
        .merge(tenant_scoped)
        .layer(Extension(AppPool(app_pool.clone())))
        .layer(Extension(AdminPool(admin_pool.clone())))
        .layer(Extension(authz))
        .layer(Extension(qdrant))
        .layer(Extension(object_store))
        .layer(Extension(enqueuer))
        .layer(Extension(vector_cleaner))
        .layer(Extension(graph_cleaner))
        .layer(Extension(llm_runtime))
        .layer(Extension(auth_state.clone()))
        .layer(Extension(cfg.rate_limit.clone()))
        .layer(Extension(tenant_header.clone()))
        .layer(Extension(rate_limiter))
        .layer(Extension(sse_limiter));

    let app = merged
        .layer(middleware::cors::layer_from_env(&tenant_header.0))
        .layer(axum::middleware::from_fn(metrics::http_metrics_middleware))
        .with_state(AppState {
            started_at: chrono::Utc::now(),
        });

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

pub async fn metrics_endpoint(
    Extension(AdminPool(pool)): Extension<AdminPool>,
) -> impl IntoResponse {
    if let Err(e) = metrics::refresh_ingest_job_metrics(&pool).await {
        warn!(error = %e, "refresh ingest job metrics failed");
    }
    (
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        metrics::render_prometheus(),
    )
}

/// Liveness probe — process is up.
#[utoipa::path(
    get,
    path = "/health",
    tag = "Health",
    responses(
        (status = 200, description = "Service is running", body = crate::openapi::schemas::HealthResponse),
    )
)]
async fn health(State(state): State<AppState>) -> impl IntoResponse {
    let body = json!({
        "status": "ok",
        "service": "gmrag-api",
        "uptime_ms": (chrono::Utc::now() - state.started_at).num_milliseconds(),
    });
    (StatusCode::OK, axum::Json(body))
}

/// Readiness probe — database connectivity check.
#[utoipa::path(
    get,
    path = "/healthz",
    tag = "Health",
    responses(
        (status = 200, description = "Database reachable", body = crate::openapi::schemas::HealthzResponse),
        (status = 503, description = "Database unreachable", body = crate::openapi::schemas::HealthzResponse),
    )
)]
async fn healthz(
    Extension(AdminPool(pool)): Extension<AdminPool>,
    Extension(authz): Extension<AuthzService>,
) -> impl IntoResponse {
    let db = sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&pool)
        .await;
    let fga = authz.health().await;

    match (db, fga) {
        (Ok(_), Ok(())) => (
            StatusCode::OK,
            axum::Json(json!({ "status": "ready", "db": "ok", "openfga": "ok" })),
        )
            .into_response(),
        (db_res, fga_res) => {
            if let Err(e) = db_res {
                warn!(error = %e, "healthz: db ping failed");
            }
            if let Err(e) = fga_res {
                warn!(error = %e, "healthz: openfga ping failed");
            }
            (
                StatusCode::SERVICE_UNAVAILABLE,
                axum::Json(json!({ "status": "degraded", "db": "checked", "openfga": "checked" })),
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
