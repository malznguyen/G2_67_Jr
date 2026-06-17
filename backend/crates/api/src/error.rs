//! Error types for the gmrag-api crate.

/// Authentication / authorization errors.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("invalid token: {0}")]
    InvalidToken(String),

    #[error("JWKS fetch failed: {0}")]
    JwksFetchFailed(String),

    #[error("missing authorization header")]
    MissingHeader,

    #[error("user not found: {0}")]
    UserNotFound(String),
}
