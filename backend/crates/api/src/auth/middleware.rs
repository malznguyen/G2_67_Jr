//! Axum middleware that resolves the authenticated user.
//!
//! `auth_middleware` is a [`axum::middleware::from_fn`] layer that runs
//! **before** [`crate::auth::tenant::tenant_middleware`] and
//! [`crate::middleware::rls::rls_middleware`]. It:
//!
//! 1. Reads `Authorization: Bearer <token>`.
//! 2. Validates the JWT via [`JwtValidator`] (from `Extension<AuthState>`).
//! 3. Parses the user UUID from the `sub` claim.
//! 4. Auto-provisions the user row via [`provision_user`] using
//!    `Extension<AdminPool>` (platform-level, bypasses RLS — justified because
//!    provisioning happens before any tenant context exists).
//! 5. Inserts [`AuthUser`] into request extensions so downstream middleware
//!    and handlers can read it via `Extension<AuthUser>`.
//!
//! Replacing the previous `AuthUser` `FromRequestParts` extractor with a
//! middleware is required by the new invariant: `TenantContext` must be
//! populated into extensions *before* `rls_middleware` runs, but axum runs
//! extractors only at handler dispatch (i.e. after all middleware). Moving
//! auth resolution into middleware guarantees `AuthUser` is available to
//! `tenant_middleware`.

use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use uuid::Uuid;

use crate::auth::extractor::{AuthState, AuthUser};
use crate::auth::jwt::JwtValidator;
use crate::auth::provision::provision_user;
use crate::error::AuthError;
use crate::pool::AdminPool;

/// Middleware that resolves `AuthUser` and stores it in request extensions.
///
/// On failure it short-circuits with the same JSON envelope as `AuthError`.
pub async fn auth_middleware(mut request: Request<Body>, next: Next) -> Response {
    // 1. Authorization header.
    let auth_header = match request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
    {
        Some(h) => h,
        None => return auth_error_response(AuthError::MissingHeader),
    };

    let token = match auth_header.strip_prefix("Bearer ") {
        Some(t) => t,
        None => {
            return auth_error_response(AuthError::InvalidToken(
                "expected 'Bearer <token>' format".to_string(),
            ))
        }
    };

    // 2. JwtValidator from extensions.
    let validator: JwtValidator = match request.extensions().get::<AuthState>() {
        Some(state) => state.jwt_validator.clone(),
        None => {
            return auth_error_response(AuthError::JwksFetchFailed(
                "auth state not configured".to_string(),
            ))
        }
    };

    // 3. Validate token.
    let claims = match validator.validate(token).await {
        Ok(c) => c,
        Err(e) => return auth_error_response(e),
    };

    // 4. Parse user UUID.
    let user_id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(e) => {
            return auth_error_response(AuthError::InvalidToken(format!(
                "invalid 'sub' UUID: {e}"
            )))
        }
    };

    let auth_user = AuthUser::new(user_id, claims);

    // 5. Auto-provision user in DB if AdminPool is available (platform-level,
    //    bypasses RLS — runs before any tenant context is set).
    if let Some(AdminPool(pool)) = request.extensions().get::<AdminPool>().cloned() {
        if let Err(e) = provision_user(&pool, &auth_user.claims).await {
            return auth_error_response(e);
        }
    }

    // 6. Store AuthUser in extensions for downstream middleware + handlers.
    request.extensions_mut().insert(auth_user);

    next.run(request).await
}

fn auth_error_response(err: AuthError) -> Response {
    let status = err_status(&err);
    let body = json!({ "error": { "code": err.code(), "message": err.to_string() } });
    (status, axum::Json(body)).into_response()
}

fn err_status(err: &AuthError) -> StatusCode {
    match err {
        AuthError::MissingHeader => StatusCode::UNAUTHORIZED,
        AuthError::InvalidToken(_) => StatusCode::UNAUTHORIZED,
        AuthError::JwksFetchFailed(_) => StatusCode::SERVICE_UNAVAILABLE,
        AuthError::UserNotFound(_) => StatusCode::UNAUTHORIZED,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::extractor::AuthState;
    use crate::auth::jwt::JwtClaims;
    use crate::pool::AdminPool;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::get;
    use axum::{Extension, Json, Router};
    use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, encode};
    use serde_json::json;
    use tower::ServiceExt;

    const TEST_PEM_PRIV: &[u8] = include_bytes!("test_keys/test_rsa_private.pem");
    const TEST_PEM_PUB: &[u8] = include_bytes!("test_keys/test_rsa_public.pem");
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

    fn make_token(claims: &JwtClaims) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(TEST_KID.to_string());
        encode(&header, claims, &EncodingKey::from_rsa_pem(TEST_PEM_PRIV).unwrap()).unwrap()
    }

    async fn make_auth_state() -> AuthState {
        let validator = JwtValidator::new(
            "http://localhost:8080/realms/gmrag".to_string(),
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

    /// Handler that reads Extension<AuthUser> (populated by auth_middleware).
    async fn protected_route(Extension(user): Extension<AuthUser>) -> impl IntoResponse {
        Json(json!({ "user_id": user.user_id.to_string() }))
    }

    fn build_app(auth_state: AuthState, pool: AdminPool) -> Router {
        Router::new()
            .route("/protected", get(protected_route))
            .layer(axum::middleware::from_fn(auth_middleware))
            // Extensions are applied outer-to-inner; auth_middleware reads
            // AuthState + AdminPool, so they must be inside (added after) the
            // from_fn layer.
            .layer(Extension(pool))
            .layer(Extension(auth_state))
    }

    async fn body_json(resp: axum::response::Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn missing_auth_header_returns_401() {
        let app = build_app(make_auth_state().await, stub_admin_pool());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body = body_json(resp).await;
        assert_eq!(body["error"]["code"], "missing-header");
    }

    #[tokio::test]
    async fn malformed_bearer_returns_401() {
        let app = build_app(make_auth_state().await, stub_admin_pool());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .header("authorization", "Basic dXNlcjpwYXNz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body = body_json(resp).await;
        assert_eq!(body["error"]["code"], "invalid-token");
    }

    #[tokio::test]
    async fn invalid_token_returns_401() {
        let app = build_app(make_auth_state().await, stub_admin_pool());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .header("authorization", "Bearer not-a-jwt")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body = body_json(resp).await;
        assert_eq!(body["error"]["code"], "invalid-token");
    }

    #[tokio::test]
    async fn expired_token_returns_401() {
        let claims = JwtClaims {
            exp: (chrono::Utc::now() - chrono::Duration::hours(1)).timestamp() as u64,
            ..make_claims()
        };
        let token = make_token(&claims);
        let app = build_app(make_auth_state().await, stub_admin_pool());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// With a stub (lazy) AdminPool, provisioning fails → 503 (JwksFetchFailed
    /// mapping is not used here; provision_user maps DB errors to UserNotFound
    /// which is 401). Valid token but no live DB → provisioning error.
    #[tokio::test]
    async fn valid_token_but_stub_pool_returns_provisioning_error() {
        let token = make_token(&make_claims());
        let app = build_app(make_auth_state().await, stub_admin_pool());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // provision_user fails on stub pool → AuthError::UserNotFound (401).
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body = body_json(resp).await;
        assert_eq!(body["error"]["code"], "user-not-found");
    }

    #[tokio::test]
    async fn missing_auth_state_returns_503() {
        // No AuthState extension → middleware cannot get JwtValidator.
        let app = Router::new()
            .route("/protected", get(protected_route))
            .layer(axum::middleware::from_fn(auth_middleware))
            .layer(Extension(stub_admin_pool()));

        let token = make_token(&make_claims());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = body_json(resp).await;
        assert_eq!(body["error"]["code"], "jwks-fetch-failed");
    }
}
