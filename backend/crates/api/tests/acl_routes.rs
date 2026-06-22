//! Integration tests for the ACL grant routes (T67).
//!
//! `routes/acl.rs` exposes list/create/revoke of ReBAC relation tuples on top
//! of `resource_acl`, guarded so that only a resource's **owner** may share or
//! revoke. Every grant/revoke writes an `audit_log` row. Tests run handlers
//! directly on an RLS-scoped `SharedConnection` (as the middleware would) and
//! verify the effect through the Check engine on the same transaction.

use axum::extract::{Extension, Path, Query};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use gmrag_api::auth::extractor::AuthUser;
use gmrag_api::auth::jwt::JwtClaims;
use gmrag_api::auth::tenant::TenantContext;
use gmrag_api::error::ApiError;
use gmrag_api::middleware::rls::SharedConnection;
use gmrag_api::rbac::check::check_relation;
use gmrag_api::rbac::model::{ObjectRef, Principal, Relation, NS_DOCUMENT};
use gmrag_api::routes::acl::{
    create_grant, list_grants, revoke_grant, AclListParams, CreateGrantBody,
};

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

async fn insert_document(pool: &PgPool, tenant_id: Uuid, owner_id: Uuid, visibility: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO documents (id, tenant_id, owner_id, title, visibility)
         VALUES ($1, $2, $3, 'Doc', $4)",
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

fn body_create(resource_id: Uuid, principal_type: &str, principal_id: Uuid, relation: &str) -> CreateGrantBody {
    CreateGrantBody {
        resource_type: NS_DOCUMENT.to_string(),
        resource_id,
        principal_type: principal_type.to_string(),
        principal_id,
        relation: relation.to_string(),
    }
}

// ─── tests ───────────────────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn owner_can_grant_viewer_and_it_takes_effect(pool: PgPool) {
    let owner = create_user(&pool, "owner@a67g.com").await;
    let friend = create_user(&pool, "friend@a67g.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let d = insert_document(&pool, tenant, owner, "private").await;

    let conn = rls_conn(&pool, tenant).await;
    let (status, body) = parts(
        create_grant(
            Path(tenant),
            Extension(TenantContext(tenant)),
            Extension(auth_user(owner)),
            Extension(conn.clone()),
            Json(body_create(d, "user", friend, "viewer")),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["relation"].as_str().unwrap(), "viewer");

    // The grant takes effect on the same RLS transaction.
    let mut guard = conn.lock().await;
    assert!(
        check_relation(&mut guard, &ObjectRef::new(NS_DOCUMENT, d), Relation::Viewer, Principal::User(friend))
            .await
            .unwrap()
    );

    // Audit row written.
    let action: String =
        sqlx::query_scalar("SELECT action FROM audit_log WHERE resource_id = $1 ORDER BY created_at DESC LIMIT 1")
            .bind(d)
            .fetch_one(&mut *guard)
            .await
            .unwrap();
    assert_eq!(action, "acl.grant");
}

#[sqlx::test(migrations = "../../migrations")]
async fn non_owner_cannot_grant(pool: PgPool) {
    let owner = create_user(&pool, "owner@a67n.com").await;
    let friend = create_user(&pool, "friend@a67n.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    // friend has only viewer (shared doc), not owner.
    let d = insert_document(&pool, tenant, owner, "shared").await;

    let conn = rls_conn(&pool, tenant).await;
    let result = create_grant(
        Path(tenant),
        Extension(TenantContext(tenant)),
        Extension(auth_user(friend)),
        Extension(conn.clone()),
        Json(body_create(d, "user", create_user(&pool, "x@a67n.com").await, "viewer")),
    )
    .await;
    assert!(matches!(result, Err(ApiError::Forbidden(_))));
}

#[sqlx::test(migrations = "../../migrations")]
async fn grant_member_relation_is_rejected(pool: PgPool) {
    let owner = create_user(&pool, "owner@a67m.com").await;
    let friend = create_user(&pool, "friend@a67m.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let d = insert_document(&pool, tenant, owner, "private").await;

    let conn = rls_conn(&pool, tenant).await;
    let result = create_grant(
        Path(tenant),
        Extension(TenantContext(tenant)),
        Extension(auth_user(owner)),
        Extension(conn.clone()),
        Json(body_create(d, "user", friend, "member")),
    )
    .await;
    assert!(matches!(result, Err(ApiError::BadRequest(_))));
}

#[sqlx::test(migrations = "../../migrations")]
async fn list_grants_returns_created_grant(pool: PgPool) {
    let owner = create_user(&pool, "owner@a67l.com").await;
    let friend = create_user(&pool, "friend@a67l.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let d = insert_document(&pool, tenant, owner, "private").await;

    let conn = rls_conn(&pool, tenant).await;
    let _ = create_grant(
        Path(tenant),
        Extension(TenantContext(tenant)),
        Extension(auth_user(owner)),
        Extension(conn.clone()),
        Json(body_create(d, "user", friend, "editor")),
    )
    .await
    .unwrap();

    let (status, body) = parts(
        list_grants(
            Path(tenant),
            Extension(TenantContext(tenant)),
            Extension(auth_user(owner)),
            Extension(conn.clone()),
            Query(AclListParams {
                resource_type: NS_DOCUMENT.to_string(),
                resource_id: d,
            }),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let grants = body["grants"].as_array().unwrap();
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0]["principal_id"].as_str().unwrap(), friend.to_string());
    assert_eq!(grants[0]["relation"].as_str().unwrap(), "editor");
}

#[sqlx::test(migrations = "../../migrations")]
async fn owner_can_revoke_and_access_is_removed(pool: PgPool) {
    let owner = create_user(&pool, "owner@a67r.com").await;
    let friend = create_user(&pool, "friend@a67r.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let d = insert_document(&pool, tenant, owner, "private").await;

    let conn = rls_conn(&pool, tenant).await;
    let (_, created) = parts(
        create_grant(
            Path(tenant),
            Extension(TenantContext(tenant)),
            Extension(auth_user(owner)),
            Extension(conn.clone()),
            Json(body_create(d, "user", friend, "viewer")),
        )
        .await,
    )
    .await;
    let grant_id = Uuid::parse_str(created["id"].as_str().unwrap()).unwrap();

    // friend can view now.
    {
        let mut guard = conn.lock().await;
        assert!(check_relation(&mut guard, &ObjectRef::new(NS_DOCUMENT, d), Relation::Viewer, Principal::User(friend))
            .await
            .unwrap());
    }

    let (status, _) = parts(
        revoke_grant(
            Path((tenant, grant_id)),
            Extension(TenantContext(tenant)),
            Extension(auth_user(owner)),
            Extension(conn.clone()),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let mut guard = conn.lock().await;
    assert!(
        !check_relation(&mut guard, &ObjectRef::new(NS_DOCUMENT, d), Relation::Viewer, Principal::User(friend))
            .await
            .unwrap(),
        "access must be removed after revoke"
    );
    // A revoke audit row exists (within one tx now() ties, so match by action).
    let revokes: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_log WHERE resource_id = $1 AND action = 'acl.revoke'",
    )
    .bind(d)
    .fetch_one(&mut *guard)
    .await
    .unwrap();
    assert_eq!(revokes, 1, "a revoke must write exactly one acl.revoke audit row");
}

#[sqlx::test(migrations = "../../migrations")]
async fn grant_to_workspace_group_reaches_members(pool: PgPool) {
    let owner = create_user(&pool, "owner@a67w.com").await;
    let member = create_user(&pool, "member@a67w.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let ws = insert_workspace(&pool, tenant, owner, "eng").await;
    add_workspace_member(&pool, ws, tenant, member).await;
    let d = insert_document(&pool, tenant, owner, "private").await;

    let conn = rls_conn(&pool, tenant).await;
    let (status, _) = parts(
        create_grant(
            Path(tenant),
            Extension(TenantContext(tenant)),
            Extension(auth_user(owner)),
            Extension(conn.clone()),
            Json(body_create(d, "workspace", ws, "viewer")),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let mut guard = conn.lock().await;
    assert!(check_relation(&mut guard, &ObjectRef::new(NS_DOCUMENT, d), Relation::Viewer, Principal::User(member))
        .await
        .unwrap());
}

#[sqlx::test(migrations = "../../migrations")]
async fn revoke_missing_grant_returns_404(pool: PgPool) {
    let owner = create_user(&pool, "owner@a67x.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;

    let conn = rls_conn(&pool, tenant).await;
    let result = revoke_grant(
        Path((tenant, Uuid::new_v4())),
        Extension(TenantContext(tenant)),
        Extension(auth_user(owner)),
        Extension(conn.clone()),
    )
    .await;
    assert!(matches!(result, Err(ApiError::NotFound)));
}
