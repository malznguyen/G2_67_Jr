//! Tenant context extractor.
//!
//! `TenantContext` is extracted from the `X-Tenant-Id` header **after**
//! `AuthUser` has been resolved.  It verifies that the authenticated user is a
//! member of the requested tenant by querying the `tenant_members` table.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::extractor::AuthUser;
use crate::error::ApiError;

/// The resolved tenant for the current request.
///
/// Carries the tenant UUID validated against the `tenant_members` table.
#[derive(Debug, Clone)]
pub struct TenantContext(pub Uuid);

const TENANT_HEADER: &str = "x-tenant-id";

#[axum::async_trait]
impl<S> FromRequestParts<S> for TenantContext
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        // AuthUser must be resolved first (declared before TenantContext in handler args).
        let auth_user = parts
            .extensions
            .get::<AuthUser>()
            .cloned()
            .ok_or(ApiError::BadRequest(
                "auth must be resolved before tenant context".into(),
            ))?;

        // Read X-Tenant-Id header.
        let header_value = parts
            .headers
            .get(TENANT_HEADER)
            .ok_or(ApiError::BadRequest("missing X-Tenant-Id header".into()))?;

        let header_str = header_value
            .to_str()
            .map_err(|_| ApiError::BadRequest("X-Tenant-Id contains invalid characters".into()))?;

        let tenant_id = Uuid::parse_str(header_str)
            .map_err(|_| ApiError::BadRequest("X-Tenant-Id is not a valid UUID".into()))?;

        // Get DbPool from extensions (injected by AppState).
        let pool = parts
            .extensions
            .get::<PgPool>()
            .cloned()
            .ok_or(ApiError::Internal("database pool not configured".into()))?;

        // Verify membership.
        let is_member = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM tenant_members WHERE tenant_id = $1 AND user_id = $2)",
        )
        .bind(tenant_id)
        .bind(auth_user.user_id)
        .fetch_one(&pool)
        .await
        .map_err(|e| ApiError::Internal(format!("membership check failed: {e}")))?;

        if !is_member {
            return Err(ApiError::Forbidden(format!(
                "user {} is not a member of tenant {tenant_id}",
                auth_user.user_id
            )));
        }

        Ok(TenantContext(tenant_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::extractor::AuthState;
    use crate::auth::jwt::{JwtClaims, JwtValidator};
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

    /// A protected route that extracts both `AuthUser` and `TenantContext`.
    async fn protected_route(
        _user: AuthUser,
        tenant: TenantContext,
    ) -> impl IntoResponse {
        Json(json!({ "tenant_id": tenant.0.to_string() }))
    }

    fn build_app(auth_state: AuthState, _pool: PgPool) -> Router {
        Router::new()
            .route("/protected", get(protected_route))
            .layer(Extension(auth_state))
    }

    #[tokio::test]
    async fn missing_tenant_header_returns_400() {
        let auth_state = make_auth_state().await;
        let pool = stub_pool().await;
        let app = build_app(auth_state, pool);

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

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = body_json(resp).await;
        assert_eq!(body["error"]["code"], "bad-request");
        let msg = body["error"]["message"].as_str().unwrap().to_string();
        assert!(msg.contains("X-Tenant-Id"), "unexpected message: {msg}");
    }

    #[tokio::test]
    async fn invalid_tenant_uuid_returns_400() {
        let auth_state = make_auth_state().await;
        let pool = stub_pool().await;
        let app = build_app(auth_state, pool);

        let token = make_token(&make_claims());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .header("authorization", format!("Bearer {token}"))
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
        let auth_state = make_auth_state().await;
        let pool = stub_pool().await;
        let app = build_app(auth_state, pool);

        let token = make_token(&make_claims());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .header("authorization", format!("Bearer {token}"))
                    .header("x-tenant-id", "")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    async fn body_json(resp: axum::response::Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn stub_pool() -> PgPool {
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy("postgres://stub:stub@127.0.0.1:1/stub")
            .expect("lazy pool")
    }
}
