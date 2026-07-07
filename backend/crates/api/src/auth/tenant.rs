//! Tenant context middleware.
//!
//! `tenant_middleware` is a [`axum::middleware::from_fn`] layer that runs
//! **after** [`crate::auth::middleware::auth_middleware`] (which populates
//! `Extension<AuthUser>`) and **before**
//! [`crate::middleware::rls::rls_middleware`] (which consumes
//! `Extension<TenantContext>`).
//!
//! It reads the configured tenant header, parses it as a UUID, and verifies that
//! the authenticated user is a member of that tenant by querying
//! `tenant_members` via the [`AdminPool`] (platform-level, bypasses RLS —
//! this lookup must succeed *before* the RLS tenant context is established,
//! otherwise the policy on `tenant_members` would hide the very row we need
//! to authorise).
//!
//! On success it inserts [`TenantContext`] into request extensions.
//!
//! This replaces the previous `TenantContext` `FromRequestParts` extractor.
//! The extractor form could not satisfy the invariant that
//! `TenantContext` must be present in extensions *before* `rls_middleware`
//! runs, because axum only dispatches extractors at handler time (after all
//! middleware).

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderName, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use uuid::Uuid;

use crate::auth::extractor::AuthUser;
use crate::authz::{
    check_or_unavailable, tenant_obj, user_obj, AuthzService, CheckRequest, REL_MEMBER,
};
use crate::pool::AdminPool;

/// The resolved tenant for the current request.
///
/// Carries the tenant UUID validated against OpenFGA tenant membership.
/// Populated by [`tenant_middleware`] and consumed by
/// [`crate::middleware::rls::rls_middleware`] and tenant-scoped handlers.
#[derive(Debug, Clone)]
pub struct TenantContext(pub Uuid);

/// Configured tenant header name.
///
/// Parsed from `GMRAG_TENANT_HEADER` at startup and injected as an extension
/// so tenant resolution cannot drift from config.
#[derive(Debug, Clone)]
pub struct TenantHeaderName(pub HeaderName);

impl TenantHeaderName {
    pub fn from_config(value: &str) -> Result<Self, axum::http::header::InvalidHeaderName> {
        HeaderName::from_bytes(value.as_bytes()).map(Self)
    }

    fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

/// Middleware that resolves [`TenantContext`] and stores it in extensions.
///
/// Requires `Extension<AuthUser>` (populated by `auth_middleware`) and
/// `Extension<AdminPool>` to be present in request extensions.
pub async fn tenant_middleware(mut request: Request<Body>, next: Next) -> Response {
    // 1. AuthUser must already be in extensions (auth_middleware ran first).
    let auth_user = match request.extensions().get::<AuthUser>().cloned() {
        Some(u) => u,
        None => {
            return tenant_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "tenant-missing-auth",
                "tenant_middleware requires AuthUser in extensions",
            )
        }
    };

    // 2. Read configured tenant header.
    let tenant_header = match request.extensions().get::<TenantHeaderName>().cloned() {
        Some(header) => header,
        None => {
            return tenant_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "tenant-missing-header-config",
                "tenant_middleware requires TenantHeaderName in extensions",
            )
        }
    };

    let header_value = match request.headers().get(&tenant_header.0) {
        Some(v) => v,
        None => {
            return tenant_error_response(
                StatusCode::BAD_REQUEST,
                "bad-request",
                &format!("missing {} header", tenant_header.as_str()),
            )
        }
    };

    let header_str = match header_value.to_str() {
        Ok(s) => s,
        Err(_) => {
            return tenant_error_response(
                StatusCode::BAD_REQUEST,
                "bad-request",
                &format!("{} contains invalid characters", tenant_header.as_str()),
            )
        }
    };

    let tenant_id = match Uuid::parse_str(header_str) {
        Ok(id) => id,
        Err(_) => {
            return tenant_error_response(
                StatusCode::BAD_REQUEST,
                "bad-request",
                &format!("{} is not a valid UUID", tenant_header.as_str()),
            )
        }
    };

    // 3. AdminPool for the tenant existence check (platform-level, bypasses RLS).
    let AdminPool(pool) = match request.extensions().get::<AdminPool>().cloned() {
        Some(p) => p,
        None => {
            return tenant_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "tenant-missing-pool",
                "tenant_middleware requires AdminPool in extensions",
            )
        }
    };

    // 4. Confirm the tenant row exists before authorizing it.
    let tenant_exists: bool =
        match sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tenants WHERE id = $1)")
            .bind(tenant_id)
            .fetch_one(&pool)
            .await
        {
            Ok(v) => v,
            Err(e) => {
                return tenant_error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal-error",
                    &format!("tenant existence check failed: {e}"),
                )
            }
        };

    if !tenant_exists {
        return tenant_error_response(
            StatusCode::FORBIDDEN,
            "forbidden",
            &format!("tenant {tenant_id} is not available to this user"),
        );
    }

    let authz = match request.extensions().get::<AuthzService>().cloned() {
        Some(authz) => authz,
        None => {
            return tenant_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "tenant-missing-authz",
                "tenant_middleware requires AuthorizationService in extensions",
            )
        }
    };

    // 5. Verify membership through OpenFGA.
    let is_member = match check_or_unavailable(
        &authz,
        CheckRequest::new(
            user_obj(auth_user.user_id),
            REL_MEMBER,
            tenant_obj(tenant_id),
        ),
    )
    .await
    {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };

    if !is_member {
        return tenant_error_response(
            StatusCode::FORBIDDEN,
            "forbidden",
            &format!(
                "user {} is not a member of tenant {tenant_id}",
                auth_user.user_id
            ),
        );
    }

    // 6. Store TenantContext in extensions for rls_middleware + handlers.
    request.extensions_mut().insert(TenantContext(tenant_id));

    next.run(request).await
}

fn tenant_error_response(status: StatusCode, code: &str, message: &str) -> Response {
    let body = json!({ "error": { "code": code, "message": message } });
    (status, axum::Json(body)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::extractor::{AuthState, AuthUser};
    use crate::auth::jwt::{JwtClaims, JwtValidator};
    use crate::pool::AdminPool;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::get;
    use axum::{Extension, Router};
    use jsonwebtoken::{encode, Algorithm, DecodingKey, EncodingKey, Header};
    use serde_json::json;
    use tower::ServiceExt;

    #[allow(dead_code)]
    const TEST_PEM_PRIV: &[u8] = include_bytes!("test_keys/test_rsa_private.pem");
    #[allow(dead_code)]
    const TEST_PEM_PUB: &[u8] = include_bytes!("test_keys/test_rsa_public.pem");
    #[allow(dead_code)]
    const TEST_KID: &str = "test-kid-1";

    fn make_claims() -> JwtClaims {
        JwtClaims {
            sub: "550e8400-e29b-41d4-a716-446655440000".to_string(),
            exp: (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp() as u64,
            iat: chrono::Utc::now().timestamp() as u64,
            iss: "http://localhost:8080/realms/gmrag".to_string(),
            aud: Some(serde_json::Value::String("gmrag-backend".to_string())),
            azp: Some("gmrag-backend".to_string()),
            scope: Some("openid".to_string()),
            preferred_username: Some("testuser".to_string()),
            email: Some("test@example.com".to_string()),
            realm_access: None,
        }
    }

    #[allow(dead_code)]
    fn make_token(claims: &JwtClaims) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(TEST_KID.to_string());
        encode(
            &header,
            claims,
            &EncodingKey::from_rsa_pem(TEST_PEM_PRIV).unwrap(),
        )
        .unwrap()
    }

    #[allow(dead_code)]
    async fn make_auth_state() -> AuthState {
        let validator = JwtValidator::new(
            "http://localhost:8080/realms/gmrag".to_string(),
            "http://localhost:8080/realms/gmrag".to_string(),
            vec!["gmrag-backend".to_string()],
            "gmrag-backend".to_string(),
        );
        validator
            .inject_key(
                TEST_KID.to_string(),
                DecodingKey::from_rsa_pem(TEST_PEM_PUB).unwrap(),
                Algorithm::RS256,
            )
            .await;
        AuthState {
            jwt_validator: validator,
        }
    }

    fn stub_admin_pool() -> AdminPool {
        AdminPool(
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(1)
                .connect_lazy("postgres://stub:stub@127.0.0.1:1/stub")
                .expect("lazy pool"),
        )
    }

    fn make_auth_user() -> AuthUser {
        AuthUser::new(
            Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap(),
            make_claims(),
        )
    }

    /// A handler that reads Extension<TenantContext>.
    async fn protected_route(Extension(tenant): Extension<TenantContext>) -> impl IntoResponse {
        Json(json!({ "tenant_id": tenant.0.to_string() }))
    }

    use axum::Json;

    fn default_tenant_header() -> TenantHeaderName {
        TenantHeaderName::from_config("X-Tenant-ID").unwrap()
    }

    /// Build a test app that wires tenant_middleware and pre-seeds
    /// Extension<AuthUser> + Extension<AdminPool> (skipping auth_middleware).
    fn build_app(auth_user: AuthUser, pool: AdminPool) -> Router {
        Router::new()
            .route("/protected", get(protected_route))
            .layer(axum::middleware::from_fn(tenant_middleware))
            .layer(Extension(auth_user))
            .layer(Extension(pool))
            .layer(Extension(default_tenant_header()))
    }

    #[tokio::test]
    async fn missing_tenant_header_returns_400() {
        let app = build_app(make_auth_user(), stub_admin_pool());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = body_json(resp).await;
        assert_eq!(body["error"]["code"], "bad-request");
        let msg = body["error"]["message"].as_str().unwrap().to_string();
        assert!(msg.contains("x-tenant-id"), "unexpected message: {msg}");
    }

    #[tokio::test]
    async fn invalid_tenant_uuid_returns_400() {
        let app = build_app(make_auth_user(), stub_admin_pool());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .header("x-tenant-id", "not-a-uuid")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = body_json(resp).await;
        assert_eq!(body["error"]["code"], "bad-request");
        let msg = body["error"]["message"].as_str().unwrap().to_string();
        assert!(msg.contains("UUID"), "unexpected message: {msg}");
    }

    #[tokio::test]
    async fn empty_tenant_header_returns_400() {
        let app = build_app(make_auth_user(), stub_admin_pool());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .header("x-tenant-id", "")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn missing_auth_user_returns_500() {
        // Don't seed AuthUser — tenant_middleware must fail cleanly.
        let app = Router::new()
            .route("/protected", get(protected_route))
            .layer(axum::middleware::from_fn(tenant_middleware))
            .layer(Extension(stub_admin_pool()))
            .layer(Extension(default_tenant_header()));

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .header("x-tenant-id", Uuid::new_v4().to_string())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = body_json(resp).await;
        assert_eq!(body["error"]["code"], "tenant-missing-auth");
    }

    #[tokio::test]
    async fn missing_admin_pool_returns_500() {
        let app = Router::new()
            .route("/protected", get(protected_route))
            .layer(axum::middleware::from_fn(tenant_middleware))
            .layer(Extension(make_auth_user()))
            .layer(Extension(default_tenant_header()));

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .header("x-tenant-id", Uuid::new_v4().to_string())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = body_json(resp).await;
        assert_eq!(body["error"]["code"], "tenant-missing-pool");
    }

    /// With a stub (lazy) pool, the membership query fails to connect → 500.
    /// Confirms the middleware reached the DB step (header + uuid parsed ok).
    #[tokio::test]
    async fn stub_pool_membership_check_returns_500() {
        let app = build_app(make_auth_user(), stub_admin_pool());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .header("x-tenant-id", Uuid::new_v4().to_string())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = body_json(resp).await;
        assert_eq!(body["error"]["code"], "internal-error");
    }

    async fn body_json(resp: axum::response::Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }
}
