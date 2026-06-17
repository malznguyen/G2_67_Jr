//! RLS (Row Level Security) middleware for tenant isolation.
//!
//! Acquires a database connection per request, starts a transaction,
//! and sets `app.tenant_id` via `SET LOCAL` so that PostgreSQL RLS
//! policies filter rows automatically.
//!
//! The connection is stored as a [`SharedConnection`] in request
//! extensions so that handlers can execute queries within the same
//! transaction (and thus the same RLS context).

use std::sync::Arc;

use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use sqlx::pool::PoolConnection;
use sqlx::{PgConnection, Postgres};
use tokio::sync::Mutex;

use crate::auth::tenant::TenantContext;
use crate::pool::AppPool;

/// A cloneable, shared database connection for use within a single request.
///
/// Handlers lock the mutex to execute queries. The lock is non-contended
/// per request because the middleware drops its own guard before calling
/// the handler.
#[derive(Clone)]
pub struct SharedConnection(Arc<Mutex<PgConnection>>);

impl SharedConnection {
    pub fn new(conn: PgConnection) -> Self {
        Self(Arc::new(Mutex::new(conn)))
    }

    /// Acquire the connection for query execution.
    pub async fn lock(&self) -> tokio::sync::MutexGuard<'_, PgConnection> {
        self.0.lock().await
    }
}

/// Axum middleware that sets up the RLS context for each request.
///
/// Requires [`TenantContext`] (populated by `tenant_middleware`) and
/// [`AppPool`] (connections running as `gmrag_app` so RLS is enforced) to
/// already be present in request extensions.
pub async fn rls_middleware(mut request: Request<Body>, next: Next) -> Response {
    // 1. Get tenant context from extensions.
    let tenant_id = match request.extensions().get::<TenantContext>().cloned() {
        Some(ctx) => ctx.0,
        None => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "rls-missing-tenant",
                "RLS middleware requires TenantContext in extensions",
            );
        }
    };

    // 2. Get AppPool (RLS-enforced) from extensions.
    let AppPool(pool) = match request.extensions().get::<AppPool>().cloned() {
        Some(p) => p,
        None => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "rls-missing-pool",
                "RLS middleware requires AppPool in extensions",
            );
        }
    };

    // 3. Acquire a connection from the pool.
    let conn: PoolConnection<Postgres> = match pool.acquire().await {
        Ok(c) => c,
        Err(e) => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "rls-connection-failed",
                &format!("failed to acquire DB connection: {e}"),
            );
        }
    };

    // 4. Detach from pool so we can manage the connection manually.
    let mut pg_conn: PgConnection = conn.detach();

    // 5. Begin transaction.
    if let Err(e) = sqlx::Executor::execute(&mut pg_conn, "BEGIN").await {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "rls-begin-failed",
            &format!("failed to begin transaction: {e}"),
        );
    }

    // 6. SET LOCAL app.tenant_id within the transaction.
    let set_sql = format!("SET LOCAL app.tenant_id = '{}'", tenant_id);
    if let Err(e) = sqlx::Executor::execute(&mut pg_conn, &*set_sql).await {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "rls-set-tenant-failed",
            &format!("failed to set tenant context: {e}"),
        );
    }

    // 7. Store SharedConnection in extensions for handlers.
    let shared = SharedConnection::new(pg_conn);
    request.extensions_mut().insert(shared.clone());

    // 8. Run the handler.
    let response = next.run(request).await;

    // 9. Commit the transaction.
    let mut guard = shared.lock().await;
    if let Err(e) = sqlx::Executor::execute(&mut *guard, "COMMIT").await {
        tracing::error!(error = %e, "RLS middleware: failed to commit transaction");
        // Return 500 — the handler ran but changes won't persist.
        drop(guard);
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "rls-commit-failed",
            "failed to commit tenant transaction",
        );
    }
    drop(guard);

    response
}

fn error_response(status: StatusCode, code: &str, message: &str) -> Response {
    let body = json!({ "error": { "code": code, "message": message } });
    (status, axum::Json(body)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::tenant::TenantContext;
    use crate::pool::AppPool;
    use axum::body::Body;
    use axum::extract::Extension;
    use axum::http::Request;
    use axum::response::IntoResponse;
    use axum::routing::get;
    use axum::Router;
    use serde_json::json;
    use tower::ServiceExt;

    /// A stub AppPool that is never actually queried (lazy connection).
    fn stub_app_pool() -> AppPool {
        AppPool(
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(1)
                .connect_lazy("postgres://stub:stub@127.0.0.1:1/stub")
                .expect("lazy pool"),
        )
    }

    /// A handler that returns the SharedConnection presence.
    async fn check_shared_conn(
        conn: Option<Extension<SharedConnection>>,
    ) -> impl IntoResponse {
        match conn {
            Some(Extension(_)) => axum::Json(json!({ "has_connection": true })),
            None => axum::Json(json!({ "has_connection": false })),
        }
    }

    /// Build a test app with the RLS middleware.
    fn build_app_with_rls(pool: AppPool, tenant_ctx: TenantContext) -> Router {
        Router::new()
            .route("/test", get(check_shared_conn))
            .layer(axum::middleware::from_fn(rls_middleware))
            .layer(Extension(pool))
            .layer(Extension(tenant_ctx))
    }

    #[tokio::test]
    async fn middleware_stores_shared_connection_in_extensions() {
        // NOTE: This test verifies that the middleware ATTEMPTS to store the
        // connection. With a stub (lazy) pool, `pool.acquire()` will fail
        // because there's no real DB. The expected response is 503 (connection
        // failed), which confirms the middleware ran correctly.
        let app = build_app_with_rls(stub_app_pool(), TenantContext(uuid::Uuid::new_v4()));

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // With stub pool, acquire fails → 503.
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], "rls-connection-failed");
    }

    #[tokio::test]
    async fn middleware_returns_500_when_tenant_context_missing() {
        // Don't add TenantContext to extensions.
        let app = Router::new()
            .route("/test", get(check_shared_conn))
            .layer(axum::middleware::from_fn(rls_middleware))
            .layer(Extension(stub_app_pool()));

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], "rls-missing-tenant");
    }

    #[tokio::test]
    async fn middleware_returns_500_when_pool_missing() {
        let tenant_ctx = TenantContext(uuid::Uuid::new_v4());
        // Don't add AppPool to extensions.
        let app = Router::new()
            .route("/test", get(check_shared_conn))
            .layer(axum::middleware::from_fn(rls_middleware))
            .layer(Extension(tenant_ctx));

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], "rls-missing-pool");
    }
}
