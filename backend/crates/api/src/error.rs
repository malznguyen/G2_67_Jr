//! Error types for the gmrag-api crate.
//!
//! All HTTP error responses follow a standardised JSON envelope:
//! ```json
//! { "error": { "code": "<kebab-case>", "message": "<human-readable>" } }
//! ```
//!
//! `AuthError` handles authentication/authorization failures.
//! `ApiError` is the catch-all for every other error path in the API.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

// ─── AuthError ───────────────────────────────────────────────────────────────

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

impl AuthError {
    /// Stable, machine-readable error code (kebab-case).
    pub fn code(&self) -> &'static str {
        match self {
            AuthError::InvalidToken(_) => "invalid-token",
            AuthError::JwksFetchFailed(_) => "jwks-fetch-failed",
            AuthError::MissingHeader => "missing-header",
            AuthError::UserNotFound(_) => "user-not-found",
        }
    }

    fn status(&self) -> StatusCode {
        match self {
            AuthError::MissingHeader => StatusCode::UNAUTHORIZED,
            AuthError::InvalidToken(_) => StatusCode::UNAUTHORIZED,
            AuthError::JwksFetchFailed(_) => StatusCode::SERVICE_UNAVAILABLE,
            AuthError::UserNotFound(_) => StatusCode::UNAUTHORIZED,
        }
    }
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let status = self.status();
        let body = axum::Json(json!({
            "error": {
                "code": self.code(),
                "message": self.to_string(),
            }
        }));
        (status, body).into_response()
    }
}

// ─── ApiError ────────────────────────────────────────────────────────────────

/// General-purpose API error.
///
/// Covers infrastructure failures (`Core`), HTTP-level client errors
/// (`NotFound`, `BadRequest`, `Forbidden`), and catch-all `Internal`.
/// Every variant maps to a stable kebab-case code in the JSON envelope.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error(transparent)]
    Auth(#[from] AuthError),

    #[error(transparent)]
    Core(#[from] gmrag_core::Error),

    #[error("not found")]
    NotFound,

    #[error("forbidden: {0}")]
    Forbidden(String),

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl ApiError {
    /// Stable, machine-readable error code (kebab-case).
    pub fn code(&self) -> String {
        match self {
            ApiError::Auth(e) => e.code().to_string(),
            ApiError::Core(e) => e.code().to_string(),
            ApiError::NotFound => "not-found".into(),
            ApiError::Forbidden(_) => "forbidden".into(),
            ApiError::BadRequest(_) => "bad-request".into(),
            ApiError::Internal(_) => "internal-error".into(),
        }
    }

    fn status(&self) -> StatusCode {
        match self {
            ApiError::Auth(e) => e.status(),
            ApiError::Core(_) => StatusCode::INTERNAL_SERVER_ERROR,
            ApiError::NotFound => StatusCode::NOT_FOUND,
            ApiError::Forbidden(_) => StatusCode::FORBIDDEN,
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        let body = axum::Json(json!({
            "error": {
                "code": self.code(),
                "message": self.to_string(),
            }
        }));
        (status, body).into_response()
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;

    // Helper: extract JSON body from a response.
    async fn body_json(resp: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    // ── AuthError envelope tests ─────────────────────────────────────────

    #[tokio::test]
    async fn auth_error_missing_header_has_string_code() {
        let resp = AuthError::MissingHeader.into_response();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body = body_json(resp).await;
        assert_eq!(body["error"]["code"], "missing-header");
        assert!(body["error"]["message"].as_str().is_some());
    }

    #[tokio::test]
    async fn auth_error_invalid_token_has_string_code() {
        let resp = AuthError::InvalidToken("bad".into()).into_response();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body = body_json(resp).await;
        assert_eq!(body["error"]["code"], "invalid-token");
        assert!(body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("bad"));
    }

    #[tokio::test]
    async fn auth_error_jwks_fetch_has_string_code() {
        let resp = AuthError::JwksFetchFailed("timeout".into()).into_response();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = body_json(resp).await;
        assert_eq!(body["error"]["code"], "jwks-fetch-failed");
    }

    #[tokio::test]
    async fn auth_error_user_not_found_has_string_code() {
        let resp = AuthError::UserNotFound("abc".into()).into_response();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body = body_json(resp).await;
        assert_eq!(body["error"]["code"], "user-not-found");
    }

    // ── ApiError envelope tests ──────────────────────────────────────────

    #[tokio::test]
    async fn api_error_auth_delegates_to_auth_error() {
        let err: ApiError = AuthError::MissingHeader.into();
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body = body_json(resp).await;
        assert_eq!(body["error"]["code"], "missing-header");
    }

    #[tokio::test]
    async fn api_error_not_found() {
        let resp = ApiError::NotFound.into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = body_json(resp).await;
        assert_eq!(body["error"]["code"], "not-found");
    }

    #[tokio::test]
    async fn api_error_forbidden() {
        let resp = ApiError::Forbidden("no access".into()).into_response();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let body = body_json(resp).await;
        assert_eq!(body["error"]["code"], "forbidden");
        assert!(body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("no access"));
    }

    #[tokio::test]
    async fn api_error_bad_request() {
        let resp = ApiError::BadRequest("bad uuid".into()).into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = body_json(resp).await;
        assert_eq!(body["error"]["code"], "bad-request");
        assert!(body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("bad uuid"));
    }

    #[tokio::test]
    async fn api_error_internal() {
        let resp = ApiError::Internal("boom".into()).into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = body_json(resp).await;
        assert_eq!(body["error"]["code"], "internal-error");
    }

    #[tokio::test]
    async fn api_error_core_maps_to_database_error_code() {
        let core_err = gmrag_core::Error::Database(sqlx::Error::PoolClosed);
        let resp: Response = ApiError::from(core_err).into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = body_json(resp).await;
        assert_eq!(body["error"]["code"], "database-error");
    }

    #[tokio::test]
    async fn api_error_core_config_maps_correctly() {
        let core_err = gmrag_core::Error::Config("missing X".into());
        let resp: Response = ApiError::from(core_err).into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = body_json(resp).await;
        assert_eq!(body["error"]["code"], "config-error");
        assert!(body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("missing X"));
    }

    // ── Envelope structure invariant ─────────────────────────────────────
    // Every error response MUST have the shape { "error": { "code": string, "message": string } }

    #[tokio::test]
    async fn envelope_always_has_error_object_with_code_and_message() {
        let errors: Vec<Response> = vec![
            AuthError::MissingHeader.into_response(),
            AuthError::InvalidToken("x".into()).into_response(),
            AuthError::JwksFetchFailed("x".into()).into_response(),
            AuthError::UserNotFound("x".into()).into_response(),
            ApiError::NotFound.into_response(),
            ApiError::Forbidden("x".into()).into_response(),
            ApiError::BadRequest("x".into()).into_response(),
            ApiError::Internal("x".into()).into_response(),
            ApiError::from(gmrag_core::Error::Config("x".into())).into_response(),
        ];

        for resp in errors {
            let body = body_json(resp).await;
            assert!(
                body.get("error").is_some(),
                "missing 'error' key in {body}"
            );
            assert!(
                body["error"]["code"].is_string(),
                "'code' must be a string, got: {}",
                body["error"]["code"]
            );
            assert!(
                body["error"]["message"].is_string(),
                "'message' must be a string, got: {}",
                body["error"]["message"]
            );
        }
    }
}
