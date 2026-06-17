//! Error types for the gmrag-api crate.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

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

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            AuthError::MissingHeader => (StatusCode::UNAUTHORIZED, self.to_string()),
            AuthError::InvalidToken(_) => (StatusCode::UNAUTHORIZED, self.to_string()),
            AuthError::JwksFetchFailed(_) => (StatusCode::SERVICE_UNAVAILABLE, self.to_string()),
            AuthError::UserNotFound(_) => (StatusCode::UNAUTHORIZED, self.to_string()),
        };

        let body = axum::Json(json!({
            "error": {
                "code": status.as_u16(),
                "message": message,
            }
        }));

        (status, body).into_response()
    }
}
