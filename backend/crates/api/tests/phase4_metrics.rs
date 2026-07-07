use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::routing::{get, post};
use axum::{Extension, Router};
use gmrag_api::auth::extractor::AuthUser;
use gmrag_api::auth::tenant::TenantContext;
use gmrag_api::authz::{
    check_or_unavailable, AuthorizationService, AuthzError, AuthzService, CheckRequest,
    CheckResult, Consistency, RelationshipTuple,
};
use gmrag_api::middleware::rate_limit::{InMemoryRateLimiter, SharedRateLimiter};
use gmrag_api::pool::AdminPool;
use gmrag_api::{metrics, metrics_endpoint};
use gmrag_core::config::RateLimitConfig;
use tower::ServiceExt;
use uuid::Uuid;

async fn ok() -> &'static str {
    "ok"
}

struct DenyAuthz;

#[async_trait]
impl AuthorizationService for DenyAuthz {
    async fn check(&self, _request: CheckRequest) -> Result<bool, AuthzError> {
        Ok(false)
    }

    async fn batch_check(
        &self,
        requests: Vec<CheckRequest>,
    ) -> Result<Vec<CheckResult>, AuthzError> {
        Ok(requests
            .into_iter()
            .map(|request| CheckResult {
                request,
                allowed: false,
            })
            .collect())
    }

    async fn list_objects(
        &self,
        _user: &str,
        _relation: &str,
        _object_type: &str,
        _consistency: Consistency,
    ) -> Result<Vec<String>, AuthzError> {
        Ok(Vec::new())
    }

    async fn read_direct_relationships(
        &self,
        _object: &str,
    ) -> Result<Vec<RelationshipTuple>, AuthzError> {
        Ok(Vec::new())
    }

    async fn read_all_direct_relationships(&self) -> Result<Vec<RelationshipTuple>, AuthzError> {
        Ok(Vec::new())
    }

    async fn write_relationships(
        &self,
        _writes: Vec<RelationshipTuple>,
        _deletes: Vec<RelationshipTuple>,
    ) -> Result<(), AuthzError> {
        Ok(())
    }

    async fn delete_all_direct_relationships_for_object(
        &self,
        _object: &str,
    ) -> Result<(), AuthzError> {
        Ok(())
    }

    async fn health(&self) -> Result<(), AuthzError> {
        Ok(())
    }
}

fn auth_user() -> AuthUser {
    AuthUser::new(
        Uuid::parse_str("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb").unwrap(),
        gmrag_api::auth::jwt::JwtClaims {
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
    )
}

fn empty_reconcile_summary() -> gmrag_api::reconcile::ReconcileSummary {
    gmrag_api::reconcile::ReconcileSummary {
        openfga: gmrag_api::reconcile::openfga::OpenFgaReport {
            missing_in_openfga: gmrag_api::reconcile::openfga::CategoryReport {
                count: 1,
                sample: vec!["tenant:sample".into()],
            },
            orphaned_in_openfga: gmrag_api::reconcile::openfga::CategoryReport {
                count: 0,
                sample: Vec::new(),
            },
            malformed: gmrag_api::reconcile::openfga::CategoryReport {
                count: 0,
                sample: Vec::new(),
            },
            auto_fix_ran: false,
            written: 0,
            deleted: 0,
        },
        qdrant: gmrag_api::reconcile::qdrant::QdrantReport {
            orphaned_chunk_points: gmrag_api::reconcile::qdrant::CategoryReport {
                count: 0,
                sample: Vec::new(),
            },
            orphaned_graph_points: gmrag_api::reconcile::qdrant::CategoryReport {
                count: 0,
                sample: Vec::new(),
            },
            missing_chunk_points: gmrag_api::reconcile::qdrant::CategoryReport {
                count: 1,
                sample: vec!["document:sample".into()],
            },
            missing_graph_points: gmrag_api::reconcile::qdrant::CategoryReport {
                count: 0,
                sample: Vec::new(),
            },
            malformed_chunk_points: 0,
            malformed_graph_points: 0,
            auto_fix_ran: false,
            deleted_chunk_docs: 0,
            deleted_graph_nodes: 0,
        },
        auto_fix: false,
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn metrics_endpoint_exposes_nonzero_samples_after_instrumented_paths(pool: sqlx::PgPool) {
    let tenant = Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap();
    let cfg = RateLimitConfig {
        enabled: true,
        auth_per_min: 1,
        job_create_per_min: 1,
        chat_create_per_min: 1,
        chat_concurrent_per_tenant: 1,
        general_per_min: 1,
        window_secs: 60,
    };
    let limiter: SharedRateLimiter = Arc::new(InMemoryRateLimiter::default());
    let app = Router::new()
        .route("/tenants/:tid/documents", post(ok))
        .route("/metrics", get(metrics_endpoint))
        .layer(axum::middleware::from_fn(metrics::http_metrics_middleware))
        .layer(axum::middleware::from_fn(
            gmrag_api::middleware::rate_limit::rate_limit_middleware,
        ))
        .layer(Extension(AdminPool(pool.clone())))
        .layer(Extension(TenantContext(tenant)))
        .layer(Extension(auth_user()))
        .layer(Extension(limiter))
        .layer(Extension(cfg));

    for expected in [StatusCode::OK, StatusCode::TOO_MANY_REQUESTS] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/tenants/{tenant}/documents"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }

    let authz: AuthzService = Arc::new(DenyAuthz);
    let allowed = check_or_unavailable(
        &authz,
        CheckRequest::new("user:test".into(), "viewer", "document:test".into()),
    )
    .await
    .expect("authz check should return a deny, not an infrastructure error");
    assert!(!allowed);

    metrics::metrics().inc_job_outcome("ingest", "success", 0.01);
    metrics::metrics().inc_sse_outcome("done");
    metrics::metrics().record_reconcile_success(&empty_reconcile_summary());

    let response = app
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();

    assert!(text.contains("gmrag_http_requests_total"));
    assert!(text.contains(r#"gmrag_rate_limit_rejections_total{category="job_create"} 1"#));
    assert!(text.contains(r#"gmrag_authz_checks_total{outcome="denied"} 1"#));
    assert!(text.contains(r#"gmrag_job_processing_total{job_type="ingest",outcome="success"} 1"#));
    assert!(text.contains(r#"gmrag_chat_sse_streams_total{outcome="done"} 1"#));
    assert!(text.contains(r#"gmrag_reconcile_runs_total{outcome="success"} 1"#));
    assert!(
        text.contains(r#"gmrag_reconcile_drift_items{category="missing",subsystem="openfga"} 1"#)
    );
}
