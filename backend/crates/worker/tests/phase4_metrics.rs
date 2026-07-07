use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use gmrag_api::authz::{
    AuthorizationService, AuthzError, AuthzService, CheckRequest, CheckResult, Consistency,
    RelationshipTuple,
};
use gmrag_core::config::QdrantConfig;
use gmrag_core::QdrantStore;
use gmrag_worker::reconcile_loop::{RealReconcileRunner, ReconcileRunner};
use gmrag_worker::{process_job_with_retry, worker_metrics_app, IngestJob, JobRunner};
use sqlx::PgPool;
use uuid::Uuid;

struct SuccessRunner {
    calls: Arc<AtomicU32>,
}

#[async_trait]
impl JobRunner for SuccessRunner {
    async fn run(&self, _job: &IngestJob) -> Result<(), String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct EmptyAuthz;

#[async_trait]
impl AuthorizationService for EmptyAuthz {
    async fn check(&self, _request: CheckRequest) -> Result<bool, AuthzError> {
        Ok(true)
    }

    async fn batch_check(
        &self,
        requests: Vec<CheckRequest>,
    ) -> Result<Vec<CheckResult>, AuthzError> {
        Ok(requests
            .into_iter()
            .map(|request| CheckResult {
                request,
                allowed: true,
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

async fn seed_job(pool: &PgPool, status: &str) -> IngestJob {
    let tenant = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, 'phase4 metrics tenant')")
        .bind(tenant)
        .execute(pool)
        .await
        .unwrap();
    let owner = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, email, name) VALUES ($1, $2, 'owner')")
        .bind(owner)
        .bind(format!("{owner}@phase4.test"))
        .execute(pool)
        .await
        .unwrap();
    let workspace = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workspaces (id, tenant_id, name, slug, created_by)
         VALUES ($1, $2, 'phase4 ws', $3, $4)",
    )
    .bind(workspace)
    .bind(tenant)
    .bind(format!("phase4-{workspace}"))
    .bind(owner)
    .execute(pool)
    .await
    .unwrap();
    let document = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO documents (id, tenant_id, workspace_id, owner_id, title, status, visibility, s3_key)
         VALUES ($1, $2, $3, $4, 'phase4 doc', 'uploaded', 'private', 'k')",
    )
    .bind(document)
    .bind(tenant)
    .bind(workspace)
    .bind(owner)
    .execute(pool)
    .await
    .unwrap();
    let job_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO ingest_jobs (id, tenant_id, document_id, status, attempts)
         VALUES ($1, $2, $3, $4, 0)",
    )
    .bind(job_id)
    .bind(tenant)
    .bind(document)
    .bind(status)
    .execute(pool)
    .await
    .unwrap();

    IngestJob {
        id: job_id,
        tenant_id: tenant,
        workspace_id: workspace,
        document_id: document,
        s3_key: "k".into(),
        filename: "phase4.pdf".into(),
        owner_id: owner,
        visibility: "private".into(),
        attempts: 0,
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn worker_metrics_port_exposes_queue_job_and_reconcile_samples(pool: PgPool) {
    let completed_job = seed_job(&pool, "pending").await;
    let _pending_job = seed_job(&pool, "pending").await;
    let calls = Arc::new(AtomicU32::new(0));
    let runner = SuccessRunner {
        calls: calls.clone(),
    };
    process_job_with_retry(&runner, &pool, &completed_job)
        .await
        .expect("job should complete");
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let qdrant = QdrantStore::new(&QdrantConfig {
        url: "http://localhost:6334".into(),
        api_key: None,
        collection_default: "gmrag_chunks".into(),
    })
    .await
    .expect("qdrant");
    let authz: AuthzService = Arc::new(EmptyAuthz);
    let reconcile = RealReconcileRunner {
        pool: pool.clone(),
        qdrant,
        authz,
    };
    reconcile.run_once(false).await.expect("reconcile dry-run");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind metrics listener");
    let addr = listener.local_addr().unwrap();
    let app = worker_metrics_app(pool.clone());
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let text = reqwest::get(format!("http://{addr}/metrics"))
        .await
        .expect("GET /metrics")
        .text()
        .await
        .expect("metrics body");
    server.abort();

    assert!(text.contains("gmrag_ingest_job_queue_depth 1"));
    assert!(text.contains(r#"gmrag_job_processing_total{job_type="ingest",outcome="success"} 1"#));
    assert!(text.contains(r#"gmrag_reconcile_runs_total{outcome="success"} 1"#));
    assert!(text.contains("gmrag_reconcile_drift_items"));
}
