# PHASE4_REPORT

## Summary

Phase 4 adds request-intake rate limiting and Prometheus observability without changing LLM/worker throughput controls or Phase 1/2/3 retry, sweeper, dispatcher, or reconciler behavior.

Implementation choices:

- Rate limiting: Axum middleware using Redis-backed token buckets in production and an in-memory implementation for focused tests.
- SSE concurrency: in-process per-tenant slot counter, acquired before stream start and released by guard drop on normal completion or abnormal disconnect.
- Metrics: `prometheus` crate (`0.13.x`) with API `/metrics` on the existing API port and worker `/metrics` on `GMRAG_WORKER_METRICS_BIND` (default `0.0.0.0:9091`).
- Metric cardinality: no tenant IDs in labels. Labels are bounded category/status/outcome/job_type/subsystem plus Axum route templates.

## Rate Limit Defaults

| Category | Scope | Env var | Default |
| --- | --- | --- | --- |
| Auth | IP | `GMRAG_RATELIMIT_AUTH_PER_MIN` | `10/min` |
| Document/job creation | Tenant | `GMRAG_RATELIMIT_JOB_CREATE_PER_MIN` | `20/min` |
| Chat creation | Tenant + user | `GMRAG_RATELIMIT_CHAT_CREATE_PER_MIN` | `30/min` |
| Chat SSE concurrent streams | Tenant | `GMRAG_RATELIMIT_CHAT_CONCURRENT_PER_TENANT` | `50` |
| General routes | Tenant, or user for non-tenant authed routes | `GMRAG_RATELIMIT_GENERAL_PER_MIN` | `300/min` |

Master switch: `GMRAG_RATELIMIT_ENABLED=true`.

Always exempt: `/health`, `/healthz`, `/metrics`.

Sample 429:

```http
HTTP/1.1 429 Too Many Requests
retry-after: 3
content-type: application/json

{"error":{"code":"rate-limit-exceeded","message":"rate limit exceeded","category":"job_create"}}
```

## Metrics

Metric families:

- `gmrag_http_requests_total`
- `gmrag_http_request_duration_seconds`
- `gmrag_rate_limit_rejections_total`
- `gmrag_authz_checks_total`
- `gmrag_ingest_job_queue_depth`
- `gmrag_ingest_jobs_by_status`
- `gmrag_job_processing_total`
- `gmrag_job_processing_duration_seconds`
- `gmrag_chat_sse_active_connections`
- `gmrag_chat_sse_streams_total`
- `gmrag_reconcile_last_run_timestamp_seconds`
- `gmrag_reconcile_runs_total`
- `gmrag_reconcile_drift_items`

Sample `/metrics` output:

```text
# HELP gmrag_http_requests_total HTTP requests by route and status.
# TYPE gmrag_http_requests_total counter
gmrag_http_requests_total{route="/tenants/:tid/documents",status="200"} 1
# HELP gmrag_rate_limit_rejections_total Rate-limit rejections by bounded category label.
# TYPE gmrag_rate_limit_rejections_total counter
gmrag_rate_limit_rejections_total{category="job_create"} 1
# HELP gmrag_ingest_job_queue_depth Pending ingest jobs.
# TYPE gmrag_ingest_job_queue_depth gauge
gmrag_ingest_job_queue_depth 1
```

## Checklist Evidence

All listed tests passed in the final `cargo test --workspace` run.

| # | Requirement | Test evidence |
| --- | --- | --- |
| 1 | Per-category N+1 request returns `429` + `Retry-After` for auth, job creation, chat creation, and general routes | `backend/crates/api/src/middleware/rate_limit.rs` -> `middleware::rate_limit::tests::per_category_n_plus_one_requests_return_429_with_retry_after` PASS |
| 2 | Cross-tenant isolation | `backend/crates/api/src/middleware/rate_limit.rs` -> `middleware::rate_limit::tests::exhausted_tenant_does_not_affect_different_tenant` PASS |
| 3 | SSE cap rejects over-cap; normal close and simulated abnormal disconnect both free slots | `backend/crates/api/src/middleware/rate_limit.rs` -> `middleware::rate_limit::tests::sse_slot_releases_after_normal_close_and_abnormal_disconnect` PASS |
| 4 | `/health`, `/healthz`, `/metrics` are never limited after all categories are exhausted | `backend/crates/api/src/middleware/rate_limit.rs` -> `middleware::rate_limit::tests::health_and_metrics_are_exempt_under_burst` PASS |
| 5 | `GMRAG_RATELIMIT_ENABLED=false` disables all categories | `backend/crates/api/src/middleware/rate_limit.rs` -> `middleware::rate_limit::tests::disabled_config_passes_through` PASS |
| 6 | API `/metrics` exposes nonzero samples after HTTP request, rate-limit rejection, authz check, job outcome, SSE done, and reconcile metrics are exercised | `backend/crates/api/tests/phase4_metrics.rs` -> `metrics_endpoint_exposes_nonzero_samples_after_instrumented_paths` PASS |
| 7 | HTTP route label uses route template, not raw path | `backend/crates/api/src/metrics.rs` -> `metrics::tests::http_metrics_route_label_uses_route_template_not_raw_path` PASS |
| 8 | Worker `/metrics` port exposes queue-depth, job-outcome, and reconciler metrics after a job runs | `backend/crates/worker/tests/phase4_metrics.rs` -> `worker_metrics_port_exposes_queue_job_and_reconcile_samples` PASS |

Route-label cardinality result: no raw-path bug was found. The `route` label is the Axum route template.

Example verified by test:

```text
gmrag_http_requests_total{route="/tenants/:tid/documents",status="200"} 1
```

The raw path form below is explicitly rejected by the route-label test:

```text
gmrag_http_requests_total{route="/tenants/11111111-1111-1111-1111-111111111111/documents",status="200"} 1
```

## Files Changed

New files:

- `PHASE4_REPORT.md`
- `backend/crates/api/src/metrics.rs`
- `backend/crates/api/src/middleware/rate_limit.rs`
- `backend/crates/api/tests/phase4_metrics.rs`
- `backend/crates/worker/tests/phase4_metrics.rs`

Edited files:

- `.env.example`
- `backend/Cargo.toml`
- `backend/Cargo.lock`
- `backend/crates/api/Cargo.toml`
- `backend/crates/api/src/authz.rs`
- `backend/crates/api/src/lib.rs`
- `backend/crates/api/src/middleware/mod.rs`
- `backend/crates/api/src/routes/chat.rs`
- `backend/crates/core/src/config.rs`
- `backend/crates/worker/Cargo.toml`
- `backend/crates/worker/src/job.rs`
- `backend/crates/worker/src/lib.rs`
- `backend/crates/worker/src/reconcile_loop.rs`
- `infra/docker-compose.yml`

## Final Verification

Final command, run from `backend/`:

```powershell
$env:DATABASE_URL='postgres://gmrag:7d52bde8138028a77dde2eb1574c33b6@localhost:5432/gmrag'
cargo test --workspace
```

This was a normal workspace test run: not `--no-run`, not `SQLX_OFFLINE`.

Final result: 409 passed, 0 failed, 0 ignored.
