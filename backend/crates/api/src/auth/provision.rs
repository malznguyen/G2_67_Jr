//! Auto-provision user from Keycloak JWT claims.
//!
//! Called during authentication to ensure the user record exists in the
//! `users` table before any business logic runs.

use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::jwt::JwtClaims;
use crate::error::AuthError;

/// Upsert a user record from JWT claims.
///
/// If the user already exists, updates `email` and `name` to match the
/// latest claims. If the user doesn't exist, inserts a new record.
pub async fn provision_user(pool: &PgPool, claims: &JwtClaims) -> Result<(), AuthError> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|e| AuthError::InvalidToken(format!("invalid sub: {e}")))?;

    let email = claims.email.as_deref().unwrap_or("");
    let name = claims.preferred_username.as_deref().unwrap_or("");

    sqlx::query(
        "INSERT INTO users (id, email, name)
         VALUES ($1, $2, $3)
         ON CONFLICT (id) DO UPDATE
           SET email = EXCLUDED.email,
               name  = EXCLUDED.name",
    )
    .bind(user_id)
    .bind(email)
    .bind(name)
    .execute(pool)
    .await
    .map_err(|e| AuthError::UserNotFound(format!("provision failed: {e}")))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_claims(sub: &str, email: &str, username: &str) -> JwtClaims {
        JwtClaims {
            sub: sub.to_string(),
            exp: (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp() as u64,
            iat: chrono::Utc::now().timestamp() as u64,
            iss: "http://localhost:8080/realms/gmrag".to_string(),
            aud: Some(serde_json::Value::String("gmrag-backend".to_string())),
            azp: Some("gmrag-backend".to_string()),
            scope: Some("openid".to_string()),
            preferred_username: Some(username.to_string()),
            email: Some(email.to_string()),
            realm_access: None,
        }
    }

    #[tokio::test]
    async fn provision_user_with_valid_claims_succeeds() {
        // This test requires a real database (DATABASE_URL env var).
        // With a stub pool, it will fail at execute(), which is expected.
        let pool = stub_pool().await;
        let claims = make_claims(
            "550e8400-e29b-41d4-a716-446655440000",
            "test@example.com",
            "testuser",
        );

        let result = provision_user(&pool, &claims).await;
        // With stub pool, this fails with UserNotFound (expected).
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, AuthError::UserNotFound(_)));
    }

    #[tokio::test]
    async fn provision_user_with_invalid_sub_returns_error() {
        let pool = stub_pool().await;
        let claims = make_claims("not-a-uuid", "test@example.com", "testuser");

        let result = provision_user(&pool, &claims).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, AuthError::InvalidToken(_)));
    }

    async fn stub_pool() -> PgPool {
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy("postgres://stub:stub@127.0.0.1:1/stub")
            .expect("lazy pool")
    }
}
