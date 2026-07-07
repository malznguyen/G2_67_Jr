//! CORS layer configured from `CORS_*` environment variables (see `.env.example`).

use axum::http::{HeaderName, HeaderValue, Method};
use tower_http::cors::{AllowCredentials, AllowHeaders, AllowMethods, AllowOrigin, CorsLayer};
use tracing::warn;

const DEFAULT_ORIGINS: &str = "http://localhost:3000";
const DEFAULT_METHODS: &str = "GET,POST,PUT,PATCH,DELETE,OPTIONS";

/// Build a [`CorsLayer`] from process environment.
///
/// Reads `CORS_ALLOWED_ORIGINS`, `CORS_ALLOWED_METHODS`, `CORS_ALLOWED_HEADERS`,
/// and `CORS_ALLOW_CREDENTIALS`. Invalid entries are skipped with a warning.
pub fn layer_from_env(tenant_header: &HeaderName) -> CorsLayer {
    let origins_raw =
        std::env::var("CORS_ALLOWED_ORIGINS").unwrap_or_else(|_| DEFAULT_ORIGINS.into());
    let methods_raw =
        std::env::var("CORS_ALLOWED_METHODS").unwrap_or_else(|_| DEFAULT_METHODS.into());
    let headers_raw = std::env::var("CORS_ALLOWED_HEADERS").unwrap_or_else(|_| {
        format!(
            "Authorization,Content-Type,{},X-Request-ID",
            tenant_header.as_str()
        )
    });
    let allow_credentials = std::env::var("CORS_ALLOW_CREDENTIALS")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(true);

    let origins: Vec<HeaderValue> = origins_raw
        .split(',')
        .filter_map(|s| {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return None;
            }
            trimmed.parse().ok().or_else(|| {
                warn!(
                    origin = trimmed,
                    "skipping invalid CORS_ALLOWED_ORIGINS entry"
                );
                None
            })
        })
        .collect();

    let methods: Vec<Method> = methods_raw
        .split(',')
        .filter_map(|s| {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return None;
            }
            trimmed.parse().ok().or_else(|| {
                warn!(
                    method = trimmed,
                    "skipping invalid CORS_ALLOWED_METHODS entry"
                );
                None
            })
        })
        .collect();

    let headers: Vec<HeaderName> = headers_raw
        .split(',')
        .filter_map(|s| {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return None;
            }
            trimmed.parse().ok().or_else(|| {
                warn!(
                    header = trimmed,
                    "skipping invalid CORS_ALLOWED_HEADERS entry"
                );
                None
            })
        })
        .collect();

    let mut layer = CorsLayer::new();

    if origins.is_empty() {
        warn!("no valid CORS origins configured; browser cross-origin requests may fail");
    } else {
        layer = layer.allow_origin(AllowOrigin::list(origins));
    }

    if methods.is_empty() {
        layer = layer.allow_methods(AllowMethods::any());
    } else {
        layer = layer.allow_methods(AllowMethods::list(methods));
    }

    if headers.is_empty() {
        layer = layer.allow_headers(AllowHeaders::any());
    } else {
        layer = layer.allow_headers(AllowHeaders::list(headers));
    }

    if allow_credentials {
        layer = layer.allow_credentials(AllowCredentials::yes());
    }

    layer
}
