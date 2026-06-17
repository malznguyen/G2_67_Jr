//! Axum extractor for authenticated user context.
//!
//! `AuthUser` is extracted from the `Authorization: Bearer <token>` header.
//! It validates the JWT using `JwtValidator` and returns the user's UUID
//! from the `sub` claim.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use uuid::Uuid;

use crate::auth::jwt::{JwtClaims, JwtValidator};
use crate::error::AuthError;

/// Authenticated user context extracted from the request.
#[derive(Debug, Clone)]
pub struct AuthUser {
    /// The user's UUID (from the JWT `sub` claim).
    pub user_id: Uuid,
    /// The raw JWT claims.
    pub claims: JwtClaims,
}

impl AuthUser {
    pub fn new(user_id: Uuid, claims: JwtClaims) -> Self {
        Self { user_id, claims }
    }
}

/// State required by the `AuthUser` extractor.
///
/// Must be available in the axum `State` or as a request extension.
#[derive(Clone)]
pub struct AuthState {
    pub jwt_validator: JwtValidator,
}

#[axum::async_trait]
impl<S> FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
{
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        // Extract the Authorization header.
        let auth_header = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .ok_or(AuthError::MissingHeader)?;

        // Parse "Bearer <token>".
        let token = auth_header
            .strip_prefix("Bearer ")
            .ok_or_else(|| AuthError::InvalidToken("expected 'Bearer <token>' format".to_string()))?;

        // Get the JwtValidator from request extensions (injected by middleware).
        let validator = parts
            .extensions
            .get::<AuthState>()
            .ok_or_else(|| AuthError::JwksFetchFailed("auth state not configured".to_string()))?
            .jwt_validator
            .clone();

        // Validate the token.
        let claims = validator.validate(token).await?;

        // Parse user UUID from the `sub` claim.
        let user_id = Uuid::parse_str(&claims.sub)
            .map_err(|e| AuthError::InvalidToken(format!("invalid 'sub' UUID: {e}")))?;

        let auth_user = AuthUser::new(user_id, claims);

        // Store in extensions so downstream extractors (e.g. TenantContext) can access it.
        parts.extensions.insert(auth_user.clone());

        Ok(auth_user)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::jwt::JwtValidator;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::get;
    use axum::{Json, Router};
    use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, encode};
    use serde_json::json;
    use tower::ServiceExt;

    const TEST_PEM_PRIV: &[u8] = include_bytes!("test_keys/test_rsa_private.pem");
    const TEST_PEM_PUB: &[u8] = include_bytes!("test_keys/test_rsa_public.pem");
    const TEST_KID: &str = "test-kid-1";

    fn make_claims() -> super::super::jwt::JwtClaims {
        super::super::jwt::JwtClaims {
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

    fn make_token(claims: &super::super::jwt::JwtClaims) -> String {
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
        AuthState { jwt_validator: validator }
    }

    /// A protected route that extracts `AuthUser` and returns the user_id.
    async fn protected_route(user: AuthUser) -> impl IntoResponse {
        Json(json!({ "user_id": user.user_id.to_string() }))
    }

    fn build_app(auth_state: AuthState) -> Router {
        Router::new()
            .route("/protected", get(protected_route))
            .layer(axum::Extension(auth_state))
    }

    #[tokio::test]
    async fn missing_auth_header_returns_401() {
        let auth_state = make_auth_state().await;
        let app = build_app(auth_state);

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
    }

    #[tokio::test]
    async fn malformed_bearer_returns_401() {
        let auth_state = make_auth_state().await;
        let app = build_app(auth_state);

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
    }

    #[tokio::test]
    async fn invalid_token_returns_401() {
        let auth_state = make_auth_state().await;
        let app = build_app(auth_state);

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
    }

    #[tokio::test]
    async fn expired_token_returns_401() {
        let claims = super::super::jwt::JwtClaims {
            exp: (chrono::Utc::now() - chrono::Duration::hours(1)).timestamp() as u64,
            ..make_claims()
        };
        let token = make_token(&claims);

        let auth_state = make_auth_state().await;
        let app = build_app(auth_state);

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

    #[tokio::test]
    async fn valid_token_extracts_user() {
        let claims = make_claims();
        let token = make_token(&claims);

        let auth_state = make_auth_state().await;
        let app = build_app(auth_state);

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

        assert_eq!(resp.status(), StatusCode::OK);

        let body: serde_json::Value =
            serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(body["user_id"], "550e8400-e29b-41d4-a716-446655440000");
    }

    #[tokio::test]
    async fn missing_auth_state_returns_503() {
        // Build app WITHOUT auth state in extensions.
        let app = Router::new()
            .route("/protected", get(protected_route));

        let claims = make_claims();
        let token = make_token(&claims);

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
    }
}
