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

use axum::extract::{Extension, Path, Query};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use gmrag_api::auth::extractor::AuthUser;
use gmrag_api::auth::jwt::JwtClaims;
use gmrag_api::auth::tenant::TenantContext;
use gmrag_api::error::ApiError;
use gmrag_api::middleware::rls::SharedConnection;
use gmrag_api::routes::documents::{list_documents, DocListParams};

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
