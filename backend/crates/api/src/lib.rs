//! gmrag-api library — HTTP service modules and bootstrap.

pub mod auth;
pub mod chat;
pub mod error;
pub mod llm;
pub mod metering;
pub mod middleware;
pub mod pool;
pub mod queue;
pub mod rbac;
pub mod routes;
pub mod storage;
pub mod vector;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use auth::extractor::AuthState;
use auth::jwt::JwtValidator;
use auth::middleware::auth_middleware;
use auth::tenant::tenant_middleware;
use axum::{
    Extension, Router, extract::DefaultBodyLimit, extract::State, http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, patch, post},
};
use gmrag_core::{Config, QdrantStore, init_app_pool, init_pool};
use routes::chat::LlmRuntime;
use middleware::rls::rls_middleware;
use pool::{AdminPool, AppPool};
use queue::RedisEnqueuer;
use storage::S3ObjectStore;
use serde_json::json;
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

    // T58/T59: document upload/delete dependencies (S3, Redis, vector cleanup).
    let object_store: Arc<dyn storage::ObjectStore> = Arc::new(S3ObjectStore::new(&cfg.s3));
    info!(bucket = %cfg.s3.bucket, "s3 object store ready");

    let enqueuer: Arc<dyn queue::JobEnqueuer> = Arc::new(
        RedisEnqueuer::connect(&cfg.redis.url)
            .await
            .context("connecting redis enqueuer")?,
    );
    info!(redis_url = %cfg.redis.url, "redis enqueuer ready");

    let vector_cleaner: Arc<dyn vector::VectorCleaner> = Arc::new(qdrant.clone());

    let llm_runtime = LlmRuntime {
        deepseek: cfg.deepseek.clone(),
        ollama: cfg.ollama.clone(),
        tenant_key_encryption_key: cfg.tenant_key_encryption_key,
    };

    let jwt_validator = JwtValidator::new(cfg.oidc.issuer.clone(), cfg.oidc.client_id.clone());
    let auth_state = AuthState { jwt_validator };
    info!(issuer = %cfg.oidc.issuer, "auth state ready");

    let public: Router<AppState> = Router::new()
        .route("/health", get(health))
        .route("/healthz", get(healthz));

    let authed: Router<AppState> = Router::new()
        .route("/users/me", get(routes::users::get_me))
        .route(
            "/tenants",
            get(routes::tenants::list_tenants).post(routes::tenants::create_tenant),
        )
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
            patch(routes::workspaces::update_workspace).delete(routes::workspaces::delete_workspace),
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
        .layer(axum::middleware::from_fn(tenant_middleware))
        .layer(axum::middleware::from_fn(auth_middleware));

    let merged: Router<AppState> = public
        .merge(authed)
        .merge(tenant_scoped)
        .layer(Extension(AppPool(app_pool.clone())))
        .layer(Extension(AdminPool(admin_pool.clone())))
        .layer(Extension(qdrant))
        .layer(Extension(object_store))
        .layer(Extension(enqueuer))
        .layer(Extension(vector_cleaner))
        .layer(Extension(llm_runtime))
        .layer(Extension(auth_state.clone()));

    let app = merged.with_state(AppState {
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

async fn health(State(state): State<AppState>) -> impl IntoResponse {
    let body = json!({
        "status": "ok",
        "service": "gmrag-api",
        "uptime_ms": (chrono::Utc::now() - state.started_at).num_milliseconds(),
    });
    (StatusCode::OK, axum::Json(body))
}

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
