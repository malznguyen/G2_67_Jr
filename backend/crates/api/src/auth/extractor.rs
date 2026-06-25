//! Authenticated user context types.
//!
//! `AuthUser` and `AuthState` are plain data types. Resolving `AuthUser`
//! from a request is now the responsibility of
//! [`crate::auth::middleware::auth_middleware`] (a `from_fn` middleware),
//! which populates `Extension<AuthUser>` before the tenant and RLS
//! middleware run. Handlers read it via `Extension<AuthUser>`.
//!
//! Previously `AuthUser` implemented `FromRequestParts`; that extractor form
//! could not satisfy the invariant that `AuthUser` must be available to
//! `tenant_middleware` *before* handler dispatch (axum runs extractors only
//! at handler time, after all middleware).

use uuid::Uuid;

use crate::auth::jwt::{JwtClaims, JwtValidator};

/// Authenticated user context.
///
/// Populated by [`crate::auth::middleware::auth_middleware`] and read by
/// handlers / downstream middleware via `Extension<AuthUser>`.
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

/// State required by the `auth_middleware`.
///
/// Must be injected as `Extension<AuthState>` so the middleware can obtain
/// the [`JwtValidator`].
#[derive(Clone)]
pub struct AuthState {
    pub jwt_validator: JwtValidator,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::jwt::JwtValidator;
    use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, encode};

    #[allow(dead_code)]
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

    #[allow(dead_code)]
    fn make_token(claims: &super::super::jwt::JwtClaims) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(TEST_KID.to_string());
        encode(&header, claims, &EncodingKey::from_rsa_pem(TEST_PEM_PRIV).unwrap()).unwrap()
    }

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
        AuthState { jwt_validator: validator }
    }

    #[test]
    fn auth_user_new_carries_id_and_claims() {
        let claims = make_claims();
        let id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        let user = AuthUser::new(id, claims.clone());
        assert_eq!(user.user_id, id);
        assert_eq!(user.claims.sub, claims.sub);
    }

    #[tokio::test]
    async fn auth_state_is_cloneable() {
        // Compile-time + runtime check that AuthState is Clone (required for
        // Extension<AuthState> cloning into each request).
        let state = make_auth_state().await;
        let _cloned = state.clone();
    }
}
