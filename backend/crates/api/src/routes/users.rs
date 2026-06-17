//! User-related API routes.

use axum::extract::Extension;
use axum::response::IntoResponse;
use axum::Json;
use serde::Serialize;
use serde_json::json;
use uuid::Uuid;

use crate::auth::extractor::AuthUser;
use crate::error::ApiError;
use crate::pool::AdminPool;

#[derive(Serialize)]
struct UserRow {
    id: Uuid,
    email: String,
    name: String,
    created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize)]
struct TenantRow {
    id: Uuid,
    name: String,
    role: String,
}

/// `GET /users/me` — Return the authenticated user's profile and tenant memberships.
///
/// Uses [`AdminPool`] (superuser, bypasses RLS) because this endpoint is
/// cross-tenant: it lists ALL tenants the user is a member of, not just the
/// active tenant. `AuthUser` is populated by `auth_middleware`.
pub async fn get_me(
    Extension(auth_user): Extension<AuthUser>,
    Extension(AdminPool(pool)): Extension<AdminPool>,
) -> Result<impl IntoResponse, ApiError> {
    let user = sqlx::query_as!(
        UserRow,
        "SELECT id, email, name, created_at FROM users WHERE id = $1",
        auth_user.user_id
    )
    .fetch_optional(&pool)
    .await
    .map_err(|e| ApiError::Internal(format!("db error: {e}")))?
    .ok_or(ApiError::NotFound)?;

    let tenants = sqlx::query_as!(
        TenantRow,
        "SELECT t.id, t.name, tm.role
         FROM tenant_members tm
         JOIN tenants t ON t.id = tm.tenant_id
         WHERE tm.user_id = $1",
        auth_user.user_id
    )
    .fetch_all(&pool)
    .await
    .map_err(|e| ApiError::Internal(format!("db error: {e}")))?;

    Ok(Json(json!({
        "user": {
            "id": user.id,
            "email": user.email,
            "name": user.name,
            "created_at": user.created_at,
        },
        "tenants": tenants.iter().map(|t| json!({
            "id": t.id,
            "name": t.name,
            "role": t.role,
        })).collect::<Vec<_>>(),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::extractor::AuthState;
    use crate::auth::jwt::{JwtClaims, JwtValidator};
    use crate::auth::middleware::auth_middleware;
    use crate::pool::AdminPool;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use axum::{Extension, Router};
    use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, encode};
    use tower::ServiceExt;

    const TEST_PEM_PRIV: &[u8] = include_bytes!("../auth/test_keys/test_rsa_private.pem");
    const TEST_PEM_PUB: &[u8] = include_bytes!("../auth/test_keys/test_rsa_public.pem");
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

    fn build_app(auth_state: AuthState, pool: AdminPool) -> Router {
        Router::new()
            .route("/users/me", get(get_me))
            .layer(axum::middleware::from_fn(auth_middleware))
            .layer(Extension(pool))
            .layer(Extension(auth_state))
    }

    #[tokio::test]
    async fn get_me_without_auth_returns_401() {
        let app = build_app(make_auth_state().await, stub_admin_pool());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/users/me")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn get_me_with_valid_auth_but_stub_pool_returns_provisioning_error() {
        // Valid token, but stub AdminPool → provision_user fails → 401.
        let app = build_app(make_auth_state().await, stub_admin_pool());

        let token = make_token(&make_claims());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/users/me")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // auth_middleware provisioning fails on stub pool → 401 user-not-found.
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
