//! Request-intake rate limiting.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::{MatchedPath, Request};
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use gmrag_core::config::RateLimitConfig;
use serde_json::json;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::auth::extractor::AuthUser;
use crate::auth::tenant::TenantContext;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitCategory {
    Auth,
    JobCreate,
    ChatCreate,
    ChatSse,
    General,
}

impl RateLimitCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auth => "auth",
            Self::JobCreate => "job_create",
            Self::ChatCreate => "chat_create",
            Self::ChatSse => "chat_sse",
            Self::General => "general",
        }
    }

    fn limit_per_window(self, cfg: &RateLimitConfig) -> u32 {
        match self {
            Self::Auth => cfg.auth_per_min,
            Self::JobCreate => cfg.job_create_per_min,
            Self::ChatCreate | Self::ChatSse => cfg.chat_create_per_min,
            Self::General => cfg.general_per_min,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LimitDecision {
    pub allowed: bool,
    pub retry_after_secs: u64,
}

#[async_trait::async_trait]
pub trait RateLimiter: Send + Sync {
    async fn check(&self, key: &str, limit: u32, window: Duration)
        -> Result<LimitDecision, String>;
}

pub type SharedRateLimiter = Arc<dyn RateLimiter>;

#[derive(Clone)]
pub struct RedisRateLimiter {
    conn: redis::aio::MultiplexedConnection,
}

impl RedisRateLimiter {
    pub async fn connect(url: &str) -> anyhow::Result<Self> {
        let client = redis::Client::open(url)?;
        let conn = client.get_multiplexed_async_connection().await?;
        Ok(Self { conn })
    }
}

#[async_trait::async_trait]
impl RateLimiter for RedisRateLimiter {
    async fn check(
        &self,
        key: &str,
        limit: u32,
        window: Duration,
    ) -> Result<LimitDecision, String> {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_millis() as u64;
        let mut conn = self.conn.clone();
        let result: (i64, i64) = redis::Script::new(TOKEN_BUCKET_LUA)
            .key(key)
            .arg(i64::from(limit))
            .arg(window.as_secs().max(1) as i64)
            .arg(now_ms as i64)
            .invoke_async(&mut conn)
            .await
            .map_err(|e| format!("redis rate limit script: {e}"))?;
        Ok(LimitDecision {
            allowed: result.0 == 1,
            retry_after_secs: result.1.max(1) as u64,
        })
    }
}

const TOKEN_BUCKET_LUA: &str = r#"
local key = KEYS[1]
local limit = tonumber(ARGV[1])
local window_secs = tonumber(ARGV[2])
local now_ms = tonumber(ARGV[3])
local refill_per_ms = limit / (window_secs * 1000)
local bucket = redis.call('HMGET', key, 'tokens', 'ts')
local tokens = tonumber(bucket[1])
local ts = tonumber(bucket[2])
if tokens == nil then
  tokens = limit
  ts = now_ms
end
local elapsed = math.max(0, now_ms - ts)
tokens = math.min(limit, tokens + (elapsed * refill_per_ms))
local allowed = 0
local retry_after = 1
if tokens >= 1 then
  tokens = tokens - 1
  allowed = 1
else
  retry_after = math.ceil((1 - tokens) / refill_per_ms / 1000)
  if retry_after < 1 then retry_after = 1 end
end
redis.call('HSET', key, 'tokens', tokens, 'ts', now_ms)
redis.call('EXPIRE', key, math.ceil(window_secs * 2))
return { allowed, retry_after }
"#;

#[derive(Default)]
pub struct InMemoryRateLimiter {
    buckets: Mutex<HashMap<String, MemoryBucket>>,
}

#[derive(Clone)]
struct MemoryBucket {
    tokens: f64,
    updated_at: Instant,
}

#[async_trait::async_trait]
impl RateLimiter for InMemoryRateLimiter {
    async fn check(
        &self,
        key: &str,
        limit: u32,
        window: Duration,
    ) -> Result<LimitDecision, String> {
        let limit = limit.max(1);
        let now = Instant::now();
        let refill_per_sec = f64::from(limit) / window.as_secs_f64().max(1.0);
        let mut buckets = self.buckets.lock().await;
        let bucket = buckets.entry(key.to_string()).or_insert(MemoryBucket {
            tokens: f64::from(limit),
            updated_at: now,
        });
        let elapsed = now.duration_since(bucket.updated_at).as_secs_f64();
        bucket.tokens = f64::from(limit).min(bucket.tokens + elapsed * refill_per_sec);
        bucket.updated_at = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            Ok(LimitDecision {
                allowed: true,
                retry_after_secs: 1,
            })
        } else {
            let retry = ((1.0 - bucket.tokens) / refill_per_sec).ceil().max(1.0) as u64;
            Ok(LimitDecision {
                allowed: false,
                retry_after_secs: retry,
            })
        }
    }
}

#[derive(Clone, Default)]
pub struct SseConnectionLimiter {
    counts: Arc<Mutex<HashMap<Uuid, u32>>>,
}

impl SseConnectionLimiter {
    pub async fn try_acquire(&self, tenant_id: Uuid, limit: u32) -> Result<SseSlotGuard, u64> {
        let mut counts = self.counts.lock().await;
        let count = counts.entry(tenant_id).or_insert(0);
        if *count >= limit {
            return Err(1);
        }
        *count += 1;
        crate::metrics::metrics().inc_sse_active();
        Ok(SseSlotGuard {
            limiter: self.clone(),
            tenant_id,
            active: true,
        })
    }

    async fn release(&self, tenant_id: Uuid) {
        let mut counts = self.counts.lock().await;
        if let Some(count) = counts.get_mut(&tenant_id) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                counts.remove(&tenant_id);
            }
        }
        crate::metrics::metrics().dec_sse_active();
    }

    pub async fn active_for(&self, tenant_id: Uuid) -> u32 {
        *self.counts.lock().await.get(&tenant_id).unwrap_or(&0)
    }
}

pub struct SseSlotGuard {
    limiter: SseConnectionLimiter,
    tenant_id: Uuid,
    active: bool,
}

impl Drop for SseSlotGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        let limiter = self.limiter.clone();
        let tenant_id = self.tenant_id;
        tokio::spawn(async move {
            limiter.release(tenant_id).await;
        });
    }
}

pub async fn rate_limit_middleware(request: Request<Body>, next: Next) -> Response {
    let Some(cfg) = request.extensions().get::<RateLimitConfig>().cloned() else {
        return next.run(request).await;
    };
    if !cfg.enabled || is_exempt(request.uri().path()) {
        return next.run(request).await;
    }

    let method = request.method().clone();
    let route = matched_route(&request);
    let Some(category) = route_category(&method, &route) else {
        return next.run(request).await;
    };
    let Some(scope) = rate_scope(&request, category) else {
        return rate_limit_error(category, 1, "missing rate-limit scope");
    };
    let Some(limiter) = request.extensions().get::<SharedRateLimiter>().cloned() else {
        return rate_limit_error(category, 1, "rate limiter unavailable");
    };

    let key = format!("gmrag:ratelimit:{}:{scope}", category.as_str());
    let decision = match limiter
        .check(
            &key,
            category.limit_per_window(&cfg),
            Duration::from_secs(cfg.window_secs.max(1)),
        )
        .await
    {
        Ok(decision) => decision,
        Err(e) => {
            tracing::warn!(error = %e, category = category.as_str(), "rate limiter failed open");
            return next.run(request).await;
        }
    };

    if decision.allowed {
        next.run(request).await
    } else {
        crate::metrics::metrics().inc_rate_limit_rejection(category.as_str());
        rate_limit_error(category, decision.retry_after_secs, "rate limit exceeded")
    }
}

fn matched_route(request: &Request<Body>) -> String {
    request
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| request.uri().path().to_string())
}

fn is_exempt(path: &str) -> bool {
    matches!(path, "/health" | "/healthz" | "/metrics")
}

fn route_category(method: &Method, route: &str) -> Option<RateLimitCategory> {
    if route.starts_with("/auth") || route.contains("/token") {
        return Some(RateLimitCategory::Auth);
    }
    match (method, route) {
        (&Method::POST, "/tenants/:tid/documents") => Some(RateLimitCategory::JobCreate),
        (&Method::POST, "/tenants/:tid/chat_sessions") => Some(RateLimitCategory::ChatCreate),
        (&Method::POST, "/tenants/:tid/chat_sessions/:sid/chat") => {
            Some(RateLimitCategory::ChatSse)
        }
        (&Method::POST, route)
            if route.starts_with("/tenants/") && route.ends_with("/documents") =>
        {
            Some(RateLimitCategory::JobCreate)
        }
        (&Method::POST, route)
            if route.starts_with("/tenants/") && route.ends_with("/chat_sessions") =>
        {
            Some(RateLimitCategory::ChatCreate)
        }
        (&Method::POST, route) if route.starts_with("/tenants/") && route.ends_with("/chat") => {
            Some(RateLimitCategory::ChatSse)
        }
        (_, route) if route.starts_with("/tenants") || route == "/users/me" => {
            Some(RateLimitCategory::General)
        }
        _ => None,
    }
}

fn rate_scope(request: &Request<Body>, category: RateLimitCategory) -> Option<String> {
    if let RateLimitCategory::Auth = category {
        return request
            .headers()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .map(|v| format!("ip:{}", v.trim()))
            .or_else(|| Some("ip:unknown".into()));
    }
    if matches!(
        category,
        RateLimitCategory::ChatCreate | RateLimitCategory::ChatSse
    ) {
        let tenant = request.extensions().get::<TenantContext>()?.0;
        let user = request.extensions().get::<AuthUser>()?.user_id;
        return Some(format!("tenant:{tenant}:user:{user}"));
    }
    if let Some(tenant) = request.extensions().get::<TenantContext>() {
        return Some(format!("tenant:{}", tenant.0));
    }
    request
        .extensions()
        .get::<AuthUser>()
        .map(|user| format!("user:{}", user.user_id))
}

fn rate_limit_error(category: RateLimitCategory, retry_after_secs: u64, message: &str) -> Response {
    let mut response = (
        StatusCode::TOO_MANY_REQUESTS,
        axum::Json(json!({
            "error": {
                "code": "rate-limit-exceeded",
                "message": message,
                "category": category.as_str(),
            }
        })),
    )
        .into_response();
    let retry = HeaderValue::from_str(&retry_after_secs.max(1).to_string())
        .unwrap_or_else(|_| HeaderValue::from_static("1"));
    response.headers_mut().insert(header::RETRY_AFTER, retry);
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;
    use axum::{Extension, Router};
    use tower::ServiceExt;

    async fn ok() -> &'static str {
        "ok"
    }

    fn app_for_tenant(limit: u32, limiter: SharedRateLimiter, tenant_id: Uuid) -> Router {
        let cfg = RateLimitConfig {
            enabled: true,
            auth_per_min: limit,
            job_create_per_min: limit,
            chat_create_per_min: limit,
            chat_concurrent_per_tenant: limit,
            general_per_min: limit,
            window_secs: 60,
        };
        Router::new()
            .route("/auth/login", get(ok).post(ok))
            .route("/health", get(ok))
            .route("/healthz", get(ok))
            .route("/metrics", get(ok))
            .route("/tenants/:tid/documents", get(ok).post(ok))
            .route("/tenants/:tid/chat_sessions", get(ok).post(ok))
            .route("/tenants/:tid/settings/llm", get(ok))
            .layer(axum::middleware::from_fn(rate_limit_middleware))
            .layer(Extension(TenantContext(tenant_id)))
            .layer(Extension(AuthUser::new(
                Uuid::parse_str("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb").unwrap(),
                crate::auth::jwt::JwtClaims {
                    sub: "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb".into(),
                    exp: 1,
                    iat: 1,
                    iss: "test".into(),
                    aud: None,
                    azp: None,
                    scope: None,
                    preferred_username: None,
                    email: None,
                    realm_access: None,
                },
            )))
            .layer(Extension(limiter))
            .layer(Extension(cfg))
    }

    fn app(limit: u32, limiter: SharedRateLimiter) -> Router {
        app_for_tenant(
            limit,
            limiter,
            Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap(),
        )
    }

    #[tokio::test]
    async fn per_category_n_plus_one_requests_return_429_with_retry_after() {
        let cases = [
            ("auth", Method::POST, "/auth/login"),
            (
                "job_create",
                Method::POST,
                "/tenants/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa/documents",
            ),
            (
                "chat_create",
                Method::POST,
                "/tenants/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa/chat_sessions",
            ),
            (
                "general",
                Method::GET,
                "/tenants/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa/settings/llm",
            ),
        ];

        for (category, method, path) in cases {
            let app = app(1, Arc::new(InMemoryRateLimiter::default()));
            let first = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method.clone())
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(first.status(), StatusCode::OK, "{category} first request");

            let second = app
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                second.status(),
                StatusCode::TOO_MANY_REQUESTS,
                "{category} N+1 request"
            );
            assert!(
                second.headers().contains_key(header::RETRY_AFTER),
                "{category} 429 must include Retry-After"
            );
        }
    }

    #[tokio::test]
    async fn disabled_config_passes_through() {
        let limiter = Arc::new(InMemoryRateLimiter::default());
        let mut cfg = RateLimitConfig {
            enabled: false,
            auth_per_min: 1,
            job_create_per_min: 1,
            chat_create_per_min: 1,
            chat_concurrent_per_tenant: 1,
            general_per_min: 1,
            window_secs: 60,
        };
        cfg.enabled = false;
        let app = Router::new()
            .route("/auth/login", get(ok).post(ok))
            .route("/tenants/:tid/documents", get(ok).post(ok))
            .route("/tenants/:tid/chat_sessions", get(ok).post(ok))
            .route("/tenants/:tid/settings/llm", get(ok))
            .layer(axum::middleware::from_fn(rate_limit_middleware))
            .layer(Extension(TenantContext(Uuid::new_v4())))
            .layer(Extension(AuthUser::new(
                Uuid::parse_str("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb").unwrap(),
                crate::auth::jwt::JwtClaims {
                    sub: "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb".into(),
                    exp: 1,
                    iat: 1,
                    iss: "test".into(),
                    aud: None,
                    azp: None,
                    scope: None,
                    preferred_username: None,
                    email: None,
                    realm_access: None,
                },
            )))
            .layer(Extension(limiter as SharedRateLimiter))
            .layer(Extension(cfg));

        for (method, path) in [
            (Method::POST, "/auth/login"),
            (Method::POST, "/tenants/x/documents"),
            (Method::POST, "/tenants/x/chat_sessions"),
            (Method::GET, "/tenants/x/settings/llm"),
        ] {
            for _ in 0..3 {
                let resp = app
                    .clone()
                    .oneshot(
                        Request::builder()
                            .method(method.clone())
                            .uri(path)
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(resp.status(), StatusCode::OK, "{path} should bypass");
            }
        }
    }

    #[tokio::test]
    async fn exhausted_tenant_does_not_affect_different_tenant() {
        let limiter: SharedRateLimiter = Arc::new(InMemoryRateLimiter::default());
        let tenant_a = Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap();
        let tenant_b = Uuid::parse_str("cccccccc-cccc-cccc-cccc-cccccccccccc").unwrap();
        let app_a = app_for_tenant(1, limiter.clone(), tenant_a);
        let app_b = app_for_tenant(1, limiter, tenant_b);

        for expected in [StatusCode::OK, StatusCode::TOO_MANY_REQUESTS] {
            let resp = app_a
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(format!("/tenants/{tenant_a}/documents"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), expected);
        }

        let resp = app_b
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/tenants/{tenant_b}/documents"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn health_and_metrics_are_exempt_under_burst() {
        let app = app(1, Arc::new(InMemoryRateLimiter::default()));
        let tenant = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";

        for (method, path) in [
            (Method::POST, "/auth/login".to_string()),
            (Method::POST, format!("/tenants/{tenant}/documents")),
            (Method::POST, format!("/tenants/{tenant}/chat_sessions")),
            (Method::GET, format!("/tenants/{tenant}/settings/llm")),
        ] {
            for _ in 0..2 {
                let _ = app
                    .clone()
                    .oneshot(
                        Request::builder()
                            .method(method.clone())
                            .uri(path.clone())
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
            }
        }

        for path in ["/health", "/healthz", "/metrics"] {
            for _ in 0..5 {
                let resp = app
                    .clone()
                    .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                    .await
                    .unwrap();
                assert_eq!(resp.status(), StatusCode::OK, "{path} must be exempt");
            }
        }
    }

    #[tokio::test]
    async fn sse_slot_releases_after_normal_close_and_abnormal_disconnect() {
        let limiter = SseConnectionLimiter::default();
        let normal_tenant = Uuid::new_v4();
        let guard = limiter
            .try_acquire(normal_tenant, 1)
            .await
            .expect("first slot");
        assert_eq!(limiter.active_for(normal_tenant).await, 1);
        assert!(limiter.try_acquire(normal_tenant, 1).await.is_err());
        drop(guard);
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(limiter.active_for(normal_tenant).await, 0);
        assert!(limiter.try_acquire(normal_tenant, 1).await.is_ok());

        let abnormal_tenant = Uuid::new_v4();
        let limiter_for_task = limiter.clone();
        let handle = tokio::spawn(async move {
            let _guard = limiter_for_task
                .try_acquire(abnormal_tenant, 1)
                .await
                .expect("abnormal slot");
            std::future::pending::<()>().await;
        });
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(limiter.active_for(abnormal_tenant).await, 1);
        assert!(limiter.try_acquire(abnormal_tenant, 1).await.is_err());
        handle.abort();
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(limiter.active_for(abnormal_tenant).await, 0);
        assert!(limiter.try_acquire(abnormal_tenant, 1).await.is_ok());
    }
}
