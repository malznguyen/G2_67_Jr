//! Prometheus metrics registry and helpers.

use std::sync::OnceLock;
use std::time::Instant;

use axum::body::Body;
use axum::extract::{MatchedPath, Request};
use axum::middleware::Next;
use axum::response::Response;
use prometheus::{
    Encoder, HistogramOpts, HistogramVec, IntCounterVec, IntGauge, IntGaugeVec, Opts, Registry,
    TextEncoder,
};
use sqlx::PgPool;

pub struct Metrics {
    registry: Registry,
    http_requests_total: IntCounterVec,
    http_request_duration_seconds: HistogramVec,
    rate_limit_rejections_total: IntCounterVec,
    authz_checks_total: IntCounterVec,
    ingest_job_queue_depth: IntGauge,
    ingest_jobs_by_status: IntGaugeVec,
    job_processing_total: IntCounterVec,
    job_processing_duration_seconds: HistogramVec,
    chat_sse_active_connections: IntGauge,
    chat_sse_streams_total: IntCounterVec,
    reconcile_last_run_timestamp_seconds: IntGauge,
    reconcile_runs_total: IntCounterVec,
    reconcile_drift_items: IntGaugeVec,
}

static METRICS: OnceLock<Metrics> = OnceLock::new();

pub fn metrics() -> &'static Metrics {
    METRICS.get_or_init(Metrics::new)
}

impl Metrics {
    fn new() -> Self {
        let registry = Registry::new_custom(Some("gmrag".into()), None)
            .expect("prometheus registry options are valid");

        let http_requests_total = IntCounterVec::new(
            Opts::new("http_requests_total", "HTTP requests by route and status."),
            &["route", "status"],
        )
        .expect("http counter is valid");
        registry
            .register(Box::new(http_requests_total.clone()))
            .expect("register http counter");

        let http_request_duration_seconds = HistogramVec::new(
            HistogramOpts::new(
                "http_request_duration_seconds",
                "HTTP request latency seconds.",
            ),
            &["route", "status"],
        )
        .expect("http histogram is valid");
        registry
            .register(Box::new(http_request_duration_seconds.clone()))
            .expect("register http histogram");

        let rate_limit_rejections_total = IntCounterVec::new(
            Opts::new(
                "rate_limit_rejections_total",
                "Rate-limit rejections by bounded category label.",
            ),
            &["category"],
        )
        .expect("rate limit counter is valid");
        registry
            .register(Box::new(rate_limit_rejections_total.clone()))
            .expect("register rate limit counter");

        let authz_checks_total = IntCounterVec::new(
            Opts::new("authz_checks_total", "Authorization check outcomes."),
            &["outcome"],
        )
        .expect("authz counter is valid");
        registry
            .register(Box::new(authz_checks_total.clone()))
            .expect("register authz counter");

        let ingest_job_queue_depth =
            IntGauge::new("ingest_job_queue_depth", "Pending ingest jobs.").expect("queue gauge");
        registry
            .register(Box::new(ingest_job_queue_depth.clone()))
            .expect("register queue gauge");

        let ingest_jobs_by_status = IntGaugeVec::new(
            Opts::new("ingest_jobs_by_status", "Ingest jobs by status."),
            &["status"],
        )
        .expect("status gauge is valid");
        registry
            .register(Box::new(ingest_jobs_by_status.clone()))
            .expect("register status gauge");

        let job_processing_total = IntCounterVec::new(
            Opts::new("job_processing_total", "Worker job processing outcomes."),
            &["job_type", "outcome"],
        )
        .expect("job counter is valid");
        registry
            .register(Box::new(job_processing_total.clone()))
            .expect("register job counter");

        let job_processing_duration_seconds = HistogramVec::new(
            HistogramOpts::new(
                "job_processing_duration_seconds",
                "Worker job processing duration seconds.",
            ),
            &["job_type", "outcome"],
        )
        .expect("job histogram is valid");
        registry
            .register(Box::new(job_processing_duration_seconds.clone()))
            .expect("register job histogram");

        let chat_sse_active_connections = IntGauge::new(
            "chat_sse_active_connections",
            "Active chat SSE connections.",
        )
        .expect("sse gauge");
        registry
            .register(Box::new(chat_sse_active_connections.clone()))
            .expect("register sse gauge");

        let chat_sse_streams_total = IntCounterVec::new(
            Opts::new("chat_sse_streams_total", "Chat SSE terminal outcomes."),
            &["outcome"],
        )
        .expect("sse counter is valid");
        registry
            .register(Box::new(chat_sse_streams_total.clone()))
            .expect("register sse counter");

        let reconcile_last_run_timestamp_seconds = IntGauge::new(
            "reconcile_last_run_timestamp_seconds",
            "Unix timestamp of the last reconcile run.",
        )
        .expect("reconcile timestamp gauge");
        registry
            .register(Box::new(reconcile_last_run_timestamp_seconds.clone()))
            .expect("register reconcile timestamp gauge");

        let reconcile_runs_total = IntCounterVec::new(
            Opts::new("reconcile_runs_total", "Reconcile run outcomes."),
            &["outcome"],
        )
        .expect("reconcile counter is valid");
        registry
            .register(Box::new(reconcile_runs_total.clone()))
            .expect("register reconcile counter");

        let reconcile_drift_items = IntGaugeVec::new(
            Opts::new("reconcile_drift_items", "Last reconcile drift counts."),
            &["subsystem", "category"],
        )
        .expect("reconcile drift gauge is valid");
        registry
            .register(Box::new(reconcile_drift_items.clone()))
            .expect("register reconcile drift gauge");

        Self {
            registry,
            http_requests_total,
            http_request_duration_seconds,
            rate_limit_rejections_total,
            authz_checks_total,
            ingest_job_queue_depth,
            ingest_jobs_by_status,
            job_processing_total,
            job_processing_duration_seconds,
            chat_sse_active_connections,
            chat_sse_streams_total,
            reconcile_last_run_timestamp_seconds,
            reconcile_runs_total,
            reconcile_drift_items,
        }
    }

    pub fn observe_http(&self, route: &str, status: u16, seconds: f64) {
        let status = status.to_string();
        self.http_requests_total
            .with_label_values(&[route, &status])
            .inc();
        self.http_request_duration_seconds
            .with_label_values(&[route, &status])
            .observe(seconds);
    }

    pub fn inc_rate_limit_rejection(&self, category: &str) {
        self.rate_limit_rejections_total
            .with_label_values(&[category])
            .inc();
    }

    pub fn inc_authz(&self, outcome: &str) {
        self.authz_checks_total.with_label_values(&[outcome]).inc();
    }

    pub fn inc_job_outcome(&self, job_type: &str, outcome: &str, seconds: f64) {
        self.job_processing_total
            .with_label_values(&[job_type, outcome])
            .inc();
        self.job_processing_duration_seconds
            .with_label_values(&[job_type, outcome])
            .observe(seconds);
    }

    pub fn inc_sse_active(&self) {
        self.chat_sse_active_connections.inc();
    }

    pub fn dec_sse_active(&self) {
        self.chat_sse_active_connections.dec();
    }

    pub fn inc_sse_outcome(&self, outcome: &str) {
        self.chat_sse_streams_total
            .with_label_values(&[outcome])
            .inc();
    }

    pub fn record_reconcile_success(&self, summary: &crate::reconcile::ReconcileSummary) {
        self.reconcile_runs_total
            .with_label_values(&["success"])
            .inc();
        self.reconcile_last_run_timestamp_seconds
            .set(chrono::Utc::now().timestamp());
        self.reconcile_drift_items
            .with_label_values(&["openfga", "missing"])
            .set(summary.openfga.missing_in_openfga.count as i64);
        self.reconcile_drift_items
            .with_label_values(&["openfga", "orphaned"])
            .set(summary.openfga.orphaned_in_openfga.count as i64);
        self.reconcile_drift_items
            .with_label_values(&["openfga", "malformed"])
            .set(summary.openfga.malformed.count as i64);
        self.reconcile_drift_items
            .with_label_values(&["qdrant", "orphaned_chunk_points"])
            .set(summary.qdrant.orphaned_chunk_points.count as i64);
        self.reconcile_drift_items
            .with_label_values(&["qdrant", "orphaned_graph_points"])
            .set(summary.qdrant.orphaned_graph_points.count as i64);
        self.reconcile_drift_items
            .with_label_values(&["qdrant", "missing_chunk_points"])
            .set(summary.qdrant.missing_chunk_points.count as i64);
        self.reconcile_drift_items
            .with_label_values(&["qdrant", "missing_graph_points"])
            .set(summary.qdrant.missing_graph_points.count as i64);
    }

    pub fn record_reconcile_failure(&self) {
        self.reconcile_runs_total
            .with_label_values(&["failure"])
            .inc();
        self.reconcile_last_run_timestamp_seconds
            .set(chrono::Utc::now().timestamp());
    }

    fn render(&self) -> String {
        let families = self.registry.gather();
        let mut buffer = Vec::new();
        TextEncoder::new()
            .encode(&families, &mut buffer)
            .expect("encode prometheus metrics");
        String::from_utf8(buffer).expect("prometheus text is utf8")
    }
}

pub async fn refresh_ingest_job_metrics(pool: &PgPool) -> Result<(), sqlx::Error> {
    let metrics = metrics();
    for status in ["pending", "processing", "completed", "failed"] {
        metrics
            .ingest_jobs_by_status
            .with_label_values(&[status])
            .set(0);
    }

    let rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT status, COUNT(*)::bigint FROM ingest_jobs GROUP BY status")
            .fetch_all(pool)
            .await?;
    let mut pending = 0_i64;
    for (status, count) in rows {
        if status == "pending" {
            pending = count;
        }
        metrics
            .ingest_jobs_by_status
            .with_label_values(&[&status])
            .set(count);
    }
    metrics.ingest_job_queue_depth.set(pending);
    Ok(())
}

pub fn render_prometheus() -> String {
    metrics().render()
}

pub async fn http_metrics_middleware(request: Request<Body>, next: Next) -> Response {
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| request.uri().path().to_string());
    let started = Instant::now();
    let response = next.run(request).await;
    metrics().observe_http(
        &route,
        response.status().as_u16(),
        started.elapsed().as_secs_f64(),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    #[test]
    fn render_prometheus_contains_core_metric_families() {
        metrics().observe_http("/health", 200, 0.001);
        metrics().inc_rate_limit_rejection("general");
        metrics().inc_authz("denied");
        metrics().inc_job_outcome("ingest", "success", 0.01);
        metrics().inc_sse_outcome("done");

        let text = render_prometheus();
        assert!(text.contains("# HELP gmrag_http_requests_total"));
        assert!(text.contains("gmrag_rate_limit_rejections_total"));
        assert!(text.contains("gmrag_authz_checks_total"));
        assert!(text.contains("gmrag_job_processing_total"));
        assert!(text.contains("gmrag_chat_sse_streams_total"));
    }

    #[tokio::test]
    async fn http_metrics_route_label_uses_route_template_not_raw_path() {
        async fn ok() -> &'static str {
            "ok"
        }

        let raw_id = "11111111-1111-1111-1111-111111111111";
        let app = Router::new()
            .route("/tenants/:tid/documents", get(ok))
            .layer(axum::middleware::from_fn(http_metrics_middleware));

        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/tenants/{raw_id}/documents"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);

        let text = render_prometheus();
        assert!(
            text.contains(
                r#"gmrag_http_requests_total{route="/tenants/:tid/documents",status="200"}"#
            ),
            "http request metric must use the route template label, got:\n{text}"
        );
        assert!(
            !text.contains(&format!(r#"route="/tenants/{raw_id}/documents""#)),
            "raw request path must not appear as a route label"
        );
    }
}
