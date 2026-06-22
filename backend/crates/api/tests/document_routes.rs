//! Integration tests for document list route (T57).
//!
//! Require a running PostgreSQL instance. `#[sqlx::test]` provisions an
//! isolated database and runs migrations automatically. Tenant-scoped handlers
//! are exercised through a [`SharedConnection`] built like `rls_middleware`:
//! `BEGIN; SET LOCAL ROLE gmrag_app; SET LOCAL app.tenant_id = '<uuid>'`.
//!
//! Visibility + ACL: a document is returned iff
//!   visibility = 'shared'
//!   OR owner_id = current_user
//!   OR (workspace_id IS NOT NULL AND current_user ∈ workspace_members).

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{Extension, Path, Query};
use axum::http::{Request, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::Router;
use serde_json::Value;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use gmrag_api::auth::extractor::AuthUser;
use gmrag_api::auth::jwt::JwtClaims;
use gmrag_api::auth::tenant::TenantContext;
use gmrag_api::error::ApiError;
use gmrag_api::middleware::rls::SharedConnection;
use gmrag_api::queue::{IngestJobPayload, JobEnqueuer};
use gmrag_api::routes::documents::{
    delete_document, list_documents, preview_document, DocListParams,
};
use gmrag_api::storage::ObjectStore;
use gmrag_api::vector::VectorCleaner;

// ─── helpers ─────────────────────────────────────────────────────────────────

fn claims_for(user_id: Uuid) -> JwtClaims {
    JwtClaims {
        sub: user_id.to_string(),
        exp: (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp() as u64,
        iat: chrono::Utc::now().timestamp() as u64,
        iss: "http://localhost:8080/realms/gmrag".to_string(),
        aud: None,
        azp: None,
        scope: None,
        preferred_username: None,
        email: None,
        realm_access: None,
    }
}

fn auth_user(user_id: Uuid) -> AuthUser {
    AuthUser::new(user_id, claims_for(user_id))
}

async fn create_user(pool: &PgPool, email: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, email, name) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(email)
        .bind(email)
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn insert_tenant(pool: &PgPool, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, $2)")
        .bind(id)
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn insert_workspace(pool: &PgPool, tenant_id: Uuid, created_by: Uuid, slug: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workspaces (id, tenant_id, name, slug, created_by)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(tenant_id)
    .bind(slug)
    .bind(slug)
    .bind(created_by)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn add_workspace_member(pool: &PgPool, workspace_id: Uuid, tenant_id: Uuid, user_id: Uuid) {
    sqlx::query(
        "INSERT INTO workspace_members (workspace_id, tenant_id, user_id, role)
         VALUES ($1, $2, $3, 'member')",
    )
    .bind(workspace_id)
    .bind(tenant_id)
    .bind(user_id)
    .execute(pool)
    .await
    .unwrap();
}

#[allow(clippy::too_many_arguments)]
async fn insert_document(
    pool: &PgPool,
    tenant_id: Uuid,
    workspace_id: Option<Uuid>,
    owner_id: Uuid,
    title: &str,
    visibility: &str,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO documents (id, tenant_id, workspace_id, owner_id, title, visibility)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(tenant_id)
    .bind(workspace_id)
    .bind(owner_id)
    .bind(title)
    .bind(visibility)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn rls_conn(pool: &PgPool, tenant_id: Uuid) -> SharedConnection {
    let mut conn = pool.acquire().await.unwrap().detach();
    sqlx::Executor::execute(&mut conn, "BEGIN").await.unwrap();
    sqlx::Executor::execute(&mut conn, "SET LOCAL ROLE gmrag_app")
        .await
        .unwrap();
    sqlx::query(&format!("SET LOCAL app.tenant_id = '{}'", tenant_id))
        .execute(&mut conn)
        .await
        .unwrap();
    SharedConnection::new(conn)
}

async fn parts(result: Result<impl IntoResponse, ApiError>) -> (StatusCode, Value) {
    let resp = result.into_response();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, body)
}

fn doc_ids(body: &Value) -> Vec<String> {
    body["documents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["id"].as_str().unwrap().to_string())
        .collect()
}

// ─── T57: documents list with visibility + ACL ───────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn shared_document_visible_to_everyone(pool: PgPool) {
    let owner = create_user(&pool, "owner@t57s.com").await;
    let other = create_user(&pool, "other@t57s.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let shared = insert_document(&pool, tenant, None, owner, "Public Doc", "shared").await;

    let conn = rls_conn(&pool, tenant).await;
    let (status, body) = parts(
        list_documents(
            Path(tenant),
            Extension(TenantContext(tenant)),
            Extension(auth_user(other)),
            Extension(conn),
            Query(DocListParams { workspace_id: None }),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(doc_ids(&body), vec![shared.to_string()]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn private_document_hidden_from_non_owner(pool: PgPool) {
    let owner = create_user(&pool, "owner@t57p.com").await;
    let other = create_user(&pool, "other@t57p.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let _private = insert_document(&pool, tenant, None, owner, "Secret", "private").await;

    let conn = rls_conn(&pool, tenant).await;
    let (status, body) = parts(
        list_documents(
            Path(tenant),
            Extension(TenantContext(tenant)),
            Extension(auth_user(other)),
            Extension(conn),
            Query(DocListParams { workspace_id: None }),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(doc_ids(&body).len(), 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn owner_sees_own_private_document(pool: PgPool) {
    let owner = create_user(&pool, "owner@t57o.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let mine = insert_document(&pool, tenant, None, owner, "My Private", "private").await;

    let conn = rls_conn(&pool, tenant).await;
    let (status, body) = parts(
        list_documents(
            Path(tenant),
            Extension(TenantContext(tenant)),
            Extension(auth_user(owner)),
            Extension(conn),
            Query(DocListParams { workspace_id: None }),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(doc_ids(&body), vec![mine.to_string()]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn workspace_member_sees_workspace_document(pool: PgPool) {
    let owner = create_user(&pool, "owner@t57w.com").await;
    let member = create_user(&pool, "member@t57w.com").await;
    let outsider = create_user(&pool, "outsider@t57w.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let ws = insert_workspace(&pool, tenant, owner, "eng").await;
    add_workspace_member(&pool, ws, tenant, member).await;
    let doc = insert_document(&pool, tenant, Some(ws), owner, "WS Doc", "private").await;

    // Member of the workspace sees the private workspace document.
    let conn = rls_conn(&pool, tenant).await;
    let (status, body) = parts(
        list_documents(
            Path(tenant),
            Extension(TenantContext(tenant)),
            Extension(auth_user(member)),
            Extension(conn),
            Query(DocListParams { workspace_id: None }),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(doc_ids(&body), vec![doc.to_string()]);

    // A non-member of the workspace does not.
    let conn = rls_conn(&pool, tenant).await;
    let (status, body) = parts(
        list_documents(
            Path(tenant),
            Extension(TenantContext(tenant)),
            Extension(auth_user(outsider)),
            Extension(conn),
            Query(DocListParams { workspace_id: None }),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(doc_ids(&body).len(), 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn filter_by_workspace_id(pool: PgPool) {
    let owner = create_user(&pool, "owner@t57f.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let ws_a = insert_workspace(&pool, tenant, owner, "wsa").await;
    let ws_b = insert_workspace(&pool, tenant, owner, "wsb").await;
    let doc_a = insert_document(&pool, tenant, Some(ws_a), owner, "A", "shared").await;
    let _doc_b = insert_document(&pool, tenant, Some(ws_b), owner, "B", "shared").await;
    let _doc_none = insert_document(&pool, tenant, None, owner, "Loose", "shared").await;

    let conn = rls_conn(&pool, tenant).await;
    let (status, body) = parts(
        list_documents(
            Path(tenant),
            Extension(TenantContext(tenant)),
            Extension(auth_user(owner)),
            Extension(conn),
            Query(DocListParams {
                workspace_id: Some(ws_a),
            }),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(doc_ids(&body), vec![doc_a.to_string()]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn documents_are_tenant_isolated(pool: PgPool) {
    let user_a = create_user(&pool, "a@t57iso.com").await;
    let user_b = create_user(&pool, "b@t57iso.com").await;
    let tenant_a = insert_tenant(&pool, "A").await;
    let tenant_b = insert_tenant(&pool, "B").await;
    // Shared doc in tenant B must never leak into tenant A's listing.
    let _doc_b = insert_document(&pool, tenant_b, None, user_b, "B Shared", "shared").await;

    let conn = rls_conn(&pool, tenant_a).await;
    let (status, body) = parts(
        list_documents(
            Path(tenant_a),
            Extension(TenantContext(tenant_a)),
            Extension(auth_user(user_a)),
            Extension(conn),
            Query(DocListParams { workspace_id: None }),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(doc_ids(&body).len(), 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn list_documents_rejects_path_mismatch(pool: PgPool) {
    let owner = create_user(&pool, "owner@t57pm.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;

    let conn = rls_conn(&pool, tenant).await;
    let result = list_documents(
        Path(Uuid::new_v4()),
        Extension(TenantContext(tenant)),
        Extension(auth_user(owner)),
        Extension(conn),
        Query(DocListParams { workspace_id: None }),
    )
    .await;
    assert!(matches!(result, Err(ApiError::BadRequest(_))));
}

// ─── T58: upload — S3 + quota + Redis enqueue (mocks) ─────────────────────────

/// Records `put`/`delete` calls and can force a `put` failure.
#[derive(Default)]
struct MockObjectStore {
    put_keys: Mutex<Vec<String>>,
    deleted_keys: Mutex<Vec<String>>,
    fail_put: bool,
}

#[async_trait::async_trait]
impl ObjectStore for MockObjectStore {
    async fn put(&self, key: &str, _data: Vec<u8>, _content_type: &str) -> Result<(), String> {
        if self.fail_put {
            return Err("forced put failure".into());
        }
        self.put_keys.lock().unwrap().push(key.to_string());
        Ok(())
    }
    async fn delete(&self, key: &str) -> Result<(), String> {
        self.deleted_keys.lock().unwrap().push(key.to_string());
        Ok(())
    }
}

/// Records the enqueued payload and can force a failure (rollback path).
#[derive(Default)]
struct MockEnqueuer {
    jobs: Mutex<Vec<IngestJobPayload>>,
    fail: bool,
}

#[async_trait::async_trait]
impl JobEnqueuer for MockEnqueuer {
    async fn enqueue(&self, job: &IngestJobPayload) -> Result<(), String> {
        if self.fail {
            return Err("forced redis failure".into());
        }
        self.jobs.lock().unwrap().push(job.clone());
        Ok(())
    }
}

/// Records each `(tenant, document)` cleanup request. Used by the T59 tests.
#[derive(Default)]
struct MockVectorCleaner {
    cleaned: Mutex<Vec<(Uuid, Uuid)>>,
}

#[async_trait::async_trait]
impl VectorCleaner for MockVectorCleaner {
    async fn delete_document_chunks(
        &self,
        tenant_id: Uuid,
        document_id: Uuid,
    ) -> Result<(), String> {
        self.cleaned.lock().unwrap().push((tenant_id, document_id));
        Ok(())
    }
}

const BOUNDARY: &str = "X-GMRAG-TEST-BOUNDARY";

/// Build a `multipart/form-data` body with text fields and an optional file part.
fn build_multipart(
    text_fields: &[(&str, &str)],
    file: Option<(&str, &str, &[u8])>,
) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, value) in text_fields {
        body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(value.as_bytes());
        body.extend_from_slice(b"\r\n");
    }
    if let Some((filename, content_type, bytes)) = file {
        body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        body.extend_from_slice(
            format!(
                "Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    body
}

#[allow(clippy::too_many_arguments)]
fn upload_app(
    conn: SharedConnection,
    tenant: Uuid,
    user: Uuid,
    store: Arc<dyn ObjectStore>,
    enqueuer: Arc<dyn JobEnqueuer>,
) -> Router {
    Router::new()
        .route(
            "/tenants/:tid/documents",
            post(gmrag_api::routes::documents::upload_document),
        )
        .layer(Extension(conn))
        .layer(Extension(TenantContext(tenant)))
        .layer(Extension(auth_user(user)))
        .layer(Extension(store))
        .layer(Extension(enqueuer))
}

fn upload_request(tenant: Uuid, body: Vec<u8>) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!("/tenants/{tenant}/documents"))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(body))
        .unwrap()
}

async fn insert_quota(pool: &PgPool, tenant_id: Uuid, max_storage_bytes: i64, max_documents: i32) {
    sqlx::query(
        "INSERT INTO tenant_quotas (tenant_id, max_storage_bytes, max_documents)
         VALUES ($1, $2, $3)",
    )
    .bind(tenant_id)
    .bind(max_storage_bytes)
    .bind(max_documents)
    .execute(pool)
    .await
    .unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn upload_succeeds_writes_db_s3_and_enqueues(pool: PgPool) {
    let owner = create_user(&pool, "owner@t58ok.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let ws = insert_workspace(&pool, tenant, owner, "eng").await;

    let store = Arc::new(MockObjectStore::default());
    let enqueuer = Arc::new(MockEnqueuer::default());
    let conn = rls_conn(&pool, tenant).await;
    let app = upload_app(
        conn.clone(),
        tenant,
        owner,
        store.clone(),
        enqueuer.clone(),
    );

    let body = build_multipart(
        &[
            ("visibility", "private"),
            ("workspace_id", &ws.to_string()),
            ("title", "My Report"),
        ],
        Some(("report.pdf", "application/pdf", b"%PDF-1.7 fake bytes")),
    );
    let resp = app.oneshot(upload_request(tenant, body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    let doc_id = Uuid::parse_str(v["id"].as_str().unwrap()).unwrap();

    // S3 put called exactly once with the tenant/ws/doc.pdf key.
    let expected_key = format!("{tenant}/{ws}/{doc_id}.pdf");
    assert_eq!(*store.put_keys.lock().unwrap(), vec![expected_key.clone()]);
    assert!(store.deleted_keys.lock().unwrap().is_empty());

    // Document + ingest_jobs rows exist on the shared (RLS) connection.
    let mut guard = conn.lock().await;
    let doc: (String, String, Uuid, Option<Uuid>, i64, Option<String>) = sqlx::query_as(
        "SELECT visibility, status, owner_id, workspace_id, byte_size, s3_key
         FROM documents WHERE id = $1",
    )
    .bind(doc_id)
    .fetch_one(&mut *guard)
    .await
    .unwrap();
    assert_eq!(doc.0, "private");
    assert_eq!(doc.1, "uploaded");
    assert_eq!(doc.2, owner);
    assert_eq!(doc.3, Some(ws));
    assert_eq!(doc.4, "%PDF-1.7 fake bytes".len() as i64);
    assert_eq!(doc.5, Some(expected_key));

    let job: (Uuid, String) =
        sqlx::query_as("SELECT id, status FROM ingest_jobs WHERE document_id = $1")
            .bind(doc_id)
            .fetch_one(&mut *guard)
            .await
            .unwrap();
    drop(guard);

    // Enqueued payload mirrors the row and carries owner_id + visibility.
    let jobs = enqueuer.jobs.lock().unwrap();
    assert_eq!(jobs.len(), 1);
    let payload = &jobs[0];
    assert_eq!(payload.id, job.0, "redis job id must equal ingest_jobs.id");
    assert_eq!(payload.tenant_id, tenant);
    assert_eq!(payload.workspace_id, ws);
    assert_eq!(payload.document_id, doc_id);
    assert_eq!(payload.owner_id, owner);
    assert_eq!(payload.visibility, "private");
    assert_eq!(payload.filename, "report.pdf");
    assert_eq!(payload.attempts, 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn upload_over_quota_returns_429(pool: PgPool) {
    let owner = create_user(&pool, "owner@t58q.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let ws = insert_workspace(&pool, tenant, owner, "eng").await;
    // 5-byte storage limit; the upload is far bigger.
    insert_quota(&pool, tenant, 5, 100).await;

    let store = Arc::new(MockObjectStore::default());
    let enqueuer = Arc::new(MockEnqueuer::default());
    let conn = rls_conn(&pool, tenant).await;
    let app = upload_app(
        conn.clone(),
        tenant,
        owner,
        store.clone(),
        enqueuer.clone(),
    );

    let body = build_multipart(
        &[("visibility", "private"), ("workspace_id", &ws.to_string())],
        Some(("big.pdf", "application/pdf", b"this is way more than five bytes")),
    );
    let resp = app.oneshot(upload_request(tenant, body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);

    // Quota rejected before any S3 write; no rows created.
    assert!(store.put_keys.lock().unwrap().is_empty());
    assert!(enqueuer.jobs.lock().unwrap().is_empty());
    let mut guard = conn.lock().await;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM documents")
        .fetch_one(&mut *guard)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn upload_invalid_visibility_returns_400(pool: PgPool) {
    let owner = create_user(&pool, "owner@t58v.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let ws = insert_workspace(&pool, tenant, owner, "eng").await;

    let store = Arc::new(MockObjectStore::default());
    let enqueuer = Arc::new(MockEnqueuer::default());
    let conn = rls_conn(&pool, tenant).await;
    let app = upload_app(
        conn.clone(),
        tenant,
        owner,
        store.clone(),
        enqueuer.clone(),
    );

    let body = build_multipart(
        &[("visibility", "public"), ("workspace_id", &ws.to_string())],
        Some(("x.pdf", "application/pdf", b"bytes")),
    );
    let resp = app.oneshot(upload_request(tenant, body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(store.put_keys.lock().unwrap().is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn upload_rolls_back_s3_and_db_when_enqueue_fails(pool: PgPool) {
    let owner = create_user(&pool, "owner@t58rb.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let ws = insert_workspace(&pool, tenant, owner, "eng").await;

    let store = Arc::new(MockObjectStore::default());
    let enqueuer = Arc::new(MockEnqueuer {
        fail: true,
        ..Default::default()
    });
    let conn = rls_conn(&pool, tenant).await;
    let app = upload_app(
        conn.clone(),
        tenant,
        owner,
        store.clone(),
        enqueuer.clone(),
    );

    let body = build_multipart(
        &[("visibility", "shared"), ("workspace_id", &ws.to_string())],
        Some(("doc.pdf", "application/pdf", b"%PDF bytes here")),
    );
    let resp = app.oneshot(upload_request(tenant, body)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

    // S3 object uploaded then deleted (rollback); same key both times.
    let put = store.put_keys.lock().unwrap().clone();
    let deleted = store.deleted_keys.lock().unwrap().clone();
    assert_eq!(put.len(), 1);
    assert_eq!(deleted, put, "the uploaded key must be deleted on rollback");

    // No document/ingest_jobs rows persisted (ROLLBACK TO SAVEPOINT).
    let mut guard = conn.lock().await;
    let docs: i64 = sqlx::query_scalar("SELECT count(*) FROM documents")
        .fetch_one(&mut *guard)
        .await
        .unwrap();
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM ingest_jobs")
        .fetch_one(&mut *guard)
        .await
        .unwrap();
    assert_eq!(docs, 0, "document insert must be rolled back");
    assert_eq!(jobs, 0, "ingest_jobs insert must be rolled back");
}

// ─── T59: delete — cascade + S3/Qdrant orphan cleanup ─────────────────────────

async fn insert_ingest_job(pool: &PgPool, tenant_id: Uuid, document_id: Uuid) {
    sqlx::query(
        "INSERT INTO ingest_jobs (tenant_id, document_id, status) VALUES ($1, $2, 'pending')",
    )
    .bind(tenant_id)
    .bind(document_id)
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_chunk(pool: &PgPool, tenant_id: Uuid, document_id: Uuid, idx: i32) {
    sqlx::query(
        "INSERT INTO document_chunks (tenant_id, document_id, chunk_index, content, qdrant_point_id)
         VALUES ($1, $2, $3, $4, gen_random_uuid())",
    )
    .bind(tenant_id)
    .bind(document_id)
    .bind(idx)
    .bind(format!("chunk {idx}"))
    .execute(pool)
    .await
    .unwrap();
}

async fn set_s3_key(pool: &PgPool, document_id: Uuid, key: &str) {
    sqlx::query("UPDATE documents SET s3_key = $2 WHERE id = $1")
        .bind(document_id)
        .bind(key)
        .execute(pool)
        .await
        .unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn delete_document_owner_cascades_and_cleans_up(pool: PgPool) {
    let owner = create_user(&pool, "owner@t59ok.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let ws = insert_workspace(&pool, tenant, owner, "eng").await;
    let doc = insert_document(&pool, tenant, Some(ws), owner, "Doc", "private").await;
    let s3_key = format!("{tenant}/{ws}/{doc}.pdf");
    set_s3_key(&pool, doc, &s3_key).await;
    insert_ingest_job(&pool, tenant, doc).await;
    insert_chunk(&pool, tenant, doc, 0).await;
    insert_chunk(&pool, tenant, doc, 1).await;

    let store = Arc::new(MockObjectStore::default());
    let cleaner = Arc::new(MockVectorCleaner::default());
    let conn = rls_conn(&pool, tenant).await;
    let (status, _) = parts(
        delete_document(
            Path((tenant, doc)),
            Extension(TenantContext(tenant)),
            Extension(auth_user(owner)),
            Extension(conn.clone()),
            Extension(store.clone() as Arc<dyn ObjectStore>),
            Extension(cleaner.clone() as Arc<dyn VectorCleaner>),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // S3 object + Qdrant chunks cleanup invoked.
    assert_eq!(*store.deleted_keys.lock().unwrap(), vec![s3_key]);
    assert_eq!(*cleaner.cleaned.lock().unwrap(), vec![(tenant, doc)]);

    // Postgres cascade removed the document + chunks + ingest job.
    let mut guard = conn.lock().await;
    let docs: i64 = sqlx::query_scalar("SELECT count(*) FROM documents WHERE id = $1")
        .bind(doc)
        .fetch_one(&mut *guard)
        .await
        .unwrap();
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM ingest_jobs WHERE document_id = $1")
        .bind(doc)
        .fetch_one(&mut *guard)
        .await
        .unwrap();
    let chunks: i64 =
        sqlx::query_scalar("SELECT count(*) FROM document_chunks WHERE document_id = $1")
            .bind(doc)
            .fetch_one(&mut *guard)
            .await
            .unwrap();
    assert_eq!(docs, 0);
    assert_eq!(jobs, 0, "ingest_jobs must cascade-delete");
    assert_eq!(chunks, 0, "document_chunks must cascade-delete");
}

#[sqlx::test(migrations = "../../migrations")]
async fn delete_document_non_owner_returns_403(pool: PgPool) {
    let owner = create_user(&pool, "owner@t59f.com").await;
    let other = create_user(&pool, "other@t59f.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let doc = insert_document(&pool, tenant, None, owner, "Doc", "shared").await;

    let store = Arc::new(MockObjectStore::default());
    let cleaner = Arc::new(MockVectorCleaner::default());
    let conn = rls_conn(&pool, tenant).await;
    let result = delete_document(
        Path((tenant, doc)),
        Extension(TenantContext(tenant)),
        Extension(auth_user(other)),
        Extension(conn.clone()),
        Extension(store.clone() as Arc<dyn ObjectStore>),
        Extension(cleaner.clone() as Arc<dyn VectorCleaner>),
    )
    .await;
    assert!(matches!(result, Err(ApiError::Forbidden(_))));

    // Nothing cleaned, document still present.
    assert!(store.deleted_keys.lock().unwrap().is_empty());
    assert!(cleaner.cleaned.lock().unwrap().is_empty());
    let mut guard = conn.lock().await;
    let docs: i64 = sqlx::query_scalar("SELECT count(*) FROM documents WHERE id = $1")
        .bind(doc)
        .fetch_one(&mut *guard)
        .await
        .unwrap();
    assert_eq!(docs, 1, "non-owner delete must not remove the document");
}

#[sqlx::test(migrations = "../../migrations")]
async fn delete_document_cross_tenant_returns_404(pool: PgPool) {
    let user_a = create_user(&pool, "a@t59x.com").await;
    let user_b = create_user(&pool, "b@t59x.com").await;
    let tenant_a = insert_tenant(&pool, "A").await;
    let tenant_b = insert_tenant(&pool, "B").await;
    let doc_b = insert_document(&pool, tenant_b, None, user_b, "B Doc", "shared").await;

    let store = Arc::new(MockObjectStore::default());
    let cleaner = Arc::new(MockVectorCleaner::default());
    // Caller operates in tenant A; doc_b belongs to tenant B → RLS hides it.
    let conn = rls_conn(&pool, tenant_a).await;
    let result = delete_document(
        Path((tenant_a, doc_b)),
        Extension(TenantContext(tenant_a)),
        Extension(auth_user(user_a)),
        Extension(conn.clone()),
        Extension(store.clone() as Arc<dyn ObjectStore>),
        Extension(cleaner.clone() as Arc<dyn VectorCleaner>),
    )
    .await;
    assert!(matches!(result, Err(ApiError::NotFound)));
    assert!(store.deleted_keys.lock().unwrap().is_empty());
    assert!(cleaner.cleaned.lock().unwrap().is_empty());

    // Document still exists in tenant B.
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM documents WHERE id = $1")
        .bind(doc_b)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

// ─── T60: preview — metadata + chunks (visibility/ACL) ────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn preview_owner_returns_metadata_and_ordered_chunks(pool: PgPool) {
    let owner = create_user(&pool, "owner@t60o.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let doc = insert_document(&pool, tenant, None, owner, "My Report", "private").await;
    // Insert out of order to prove ORDER BY chunk_index ASC.
    insert_chunk(&pool, tenant, doc, 2).await;
    insert_chunk(&pool, tenant, doc, 0).await;
    insert_chunk(&pool, tenant, doc, 1).await;

    let conn = rls_conn(&pool, tenant).await;
    let (status, body) = parts(
        preview_document(
            Path((tenant, doc)),
            Extension(TenantContext(tenant)),
            Extension(auth_user(owner)),
            Extension(conn),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["document"]["id"].as_str().unwrap(), doc.to_string());
    assert_eq!(body["document"]["title"].as_str().unwrap(), "My Report");
    assert_eq!(body["document"]["visibility"].as_str().unwrap(), "private");

    let chunks = body["chunks"].as_array().unwrap();
    assert_eq!(chunks.len(), 3);
    let indices: Vec<i64> = chunks
        .iter()
        .map(|c| c["chunk_index"].as_i64().unwrap())
        .collect();
    assert_eq!(indices, vec![0, 1, 2], "chunks must be ordered by chunk_index");
    assert_eq!(chunks[0]["content"].as_str().unwrap(), "chunk 0");
}

#[sqlx::test(migrations = "../../migrations")]
async fn preview_shared_document_visible_to_other(pool: PgPool) {
    let owner = create_user(&pool, "owner@t60s.com").await;
    let other = create_user(&pool, "other@t60s.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let doc = insert_document(&pool, tenant, None, owner, "Shared", "shared").await;

    let conn = rls_conn(&pool, tenant).await;
    let (status, body) = parts(
        preview_document(
            Path((tenant, doc)),
            Extension(TenantContext(tenant)),
            Extension(auth_user(other)),
            Extension(conn),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["document"]["id"].as_str().unwrap(), doc.to_string());
    assert_eq!(body["chunks"].as_array().unwrap().len(), 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn preview_workspace_member_sees_private_document(pool: PgPool) {
    let owner = create_user(&pool, "owner@t60w.com").await;
    let member = create_user(&pool, "member@t60w.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let ws = insert_workspace(&pool, tenant, owner, "eng").await;
    add_workspace_member(&pool, ws, tenant, member).await;
    let doc = insert_document(&pool, tenant, Some(ws), owner, "WS Doc", "private").await;

    let conn = rls_conn(&pool, tenant).await;
    let (status, body) = parts(
        preview_document(
            Path((tenant, doc)),
            Extension(TenantContext(tenant)),
            Extension(auth_user(member)),
            Extension(conn),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["document"]["id"].as_str().unwrap(), doc.to_string());
}

#[sqlx::test(migrations = "../../migrations")]
async fn preview_private_hidden_from_non_owner_returns_404(pool: PgPool) {
    let owner = create_user(&pool, "owner@t60p.com").await;
    let other = create_user(&pool, "other@t60p.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let doc = insert_document(&pool, tenant, None, owner, "Secret", "private").await;

    let conn = rls_conn(&pool, tenant).await;
    let result = preview_document(
        Path((tenant, doc)),
        Extension(TenantContext(tenant)),
        Extension(auth_user(other)),
        Extension(conn),
    )
    .await;
    assert!(matches!(result, Err(ApiError::NotFound)));
}

#[sqlx::test(migrations = "../../migrations")]
async fn preview_cross_tenant_returns_404(pool: PgPool) {
    let user_a = create_user(&pool, "a@t60x.com").await;
    let user_b = create_user(&pool, "b@t60x.com").await;
    let tenant_a = insert_tenant(&pool, "A").await;
    let tenant_b = insert_tenant(&pool, "B").await;
    let doc_b = insert_document(&pool, tenant_b, None, user_b, "B Shared", "shared").await;

    let conn = rls_conn(&pool, tenant_a).await;
    let result = preview_document(
        Path((tenant_a, doc_b)),
        Extension(TenantContext(tenant_a)),
        Extension(auth_user(user_a)),
        Extension(conn),
    )
    .await;
    assert!(matches!(result, Err(ApiError::NotFound)));
}
