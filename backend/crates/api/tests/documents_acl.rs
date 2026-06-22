//! T83 regression tests: document read paths honour ReBAC `resource_acl`
//! grants (direct user shares + workspace-group shares) in addition to the
//! legacy `shared`/owner/workspace-member visibility, via the Check engine.

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
use gmrag_api::routes::documents::{list_documents, preview_document, DocListParams};

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
        "INSERT INTO workspaces (id, tenant_id, name, slug, created_by) VALUES ($1,$2,$3,$4,$5)",
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
        "INSERT INTO workspace_members (workspace_id, tenant_id, user_id, role) VALUES ($1,$2,$3,'member')",
    )
    .bind(workspace_id)
    .bind(tenant_id)
    .bind(user_id)
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_document(pool: &PgPool, tenant_id: Uuid, owner_id: Uuid, visibility: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO documents (id, tenant_id, owner_id, title, visibility) VALUES ($1,$2,$3,'Doc',$4)",
    )
    .bind(id)
    .bind(tenant_id)
    .bind(owner_id)
    .bind(visibility)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn insert_grant(
    pool: &PgPool,
    tenant_id: Uuid,
    resource_id: Uuid,
    principal_type: &str,
    principal_id: Uuid,
    relation: &str,
) {
    sqlx::query(
        "INSERT INTO resource_acl
           (tenant_id, resource_type, resource_id, principal_type, principal_id, permission)
         VALUES ($1, 'document', $2, $3, $4, $5)",
    )
    .bind(tenant_id)
    .bind(resource_id)
    .bind(principal_type)
    .bind(principal_id)
    .bind(relation)
    .execute(pool)
    .await
    .unwrap();
}

async fn rls_conn(pool: &PgPool, tenant_id: Uuid) -> SharedConnection {
    let mut conn = pool.acquire().await.unwrap().detach();
    sqlx::Executor::execute(&mut conn, "BEGIN").await.unwrap();
    sqlx::Executor::execute(&mut conn, "SET LOCAL ROLE gmrag_app")
        .await
        .unwrap();
    sqlx::query(&format!("SET LOCAL app.tenant_id = '{tenant_id}'"))
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

#[sqlx::test(migrations = "../../migrations")]
async fn list_includes_document_shared_via_acl(pool: PgPool) {
    let owner = create_user(&pool, "owner@t83l.com").await;
    let friend = create_user(&pool, "friend@t83l.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let d = insert_document(&pool, tenant, owner, "private").await;
    insert_grant(&pool, tenant, d, "user", friend, "viewer").await;

    let conn = rls_conn(&pool, tenant).await;
    let (status, body) = parts(
        list_documents(
            Path(tenant),
            Extension(TenantContext(tenant)),
            Extension(auth_user(friend)),
            Extension(conn),
            Query(DocListParams { workspace_id: None }),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        doc_ids(&body),
        vec![d.to_string()],
        "a document shared via resource_acl must appear in the recipient's list"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn list_includes_document_shared_with_workspace_group(pool: PgPool) {
    let owner = create_user(&pool, "owner@t83w.com").await;
    let member = create_user(&pool, "member@t83w.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let ws = insert_workspace(&pool, tenant, owner, "eng").await;
    add_workspace_member(&pool, ws, tenant, member).await;
    let d = insert_document(&pool, tenant, owner, "private").await;
    insert_grant(&pool, tenant, d, "workspace", ws, "viewer").await;

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
    assert_eq!(doc_ids(&body), vec![d.to_string()]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn preview_visible_to_acl_recipient(pool: PgPool) {
    let owner = create_user(&pool, "owner@t83p.com").await;
    let friend = create_user(&pool, "friend@t83p.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let d = insert_document(&pool, tenant, owner, "private").await;
    insert_grant(&pool, tenant, d, "user", friend, "viewer").await;

    let conn = rls_conn(&pool, tenant).await;
    let (status, body) = parts(
        preview_document(
            Path((tenant, d)),
            Extension(TenantContext(tenant)),
            Extension(auth_user(friend)),
            Extension(conn),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["document"]["id"].as_str().unwrap(), d.to_string());
}

#[sqlx::test(migrations = "../../migrations")]
async fn preview_denied_without_grant(pool: PgPool) {
    let owner = create_user(&pool, "owner@t83d.com").await;
    let stranger = create_user(&pool, "stranger@t83d.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let d = insert_document(&pool, tenant, owner, "private").await;

    let conn = rls_conn(&pool, tenant).await;
    let result = preview_document(
        Path((tenant, d)),
        Extension(TenantContext(tenant)),
        Extension(auth_user(stranger)),
        Extension(conn),
    )
    .await;
    assert!(matches!(result, Err(ApiError::NotFound)));
}
