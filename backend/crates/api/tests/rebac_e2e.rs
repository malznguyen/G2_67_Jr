//! T85 — ReBAC end-to-end + negative pentest.
//!
//! Drives the real route handlers (`routes/acl.rs` create/revoke +
//! `routes/documents.rs` preview) over an RLS-scoped `SharedConnection` to
//! prove the whole share/revoke/inheritance flow, plus adversarial cases:
//!   - share → access granted; revoke → access denied,
//!   - a non-owner (mere viewer) cannot re-share (privilege escalation),
//!   - a cross-tenant resource cannot be granted on and its grants are
//!     invisible (RLS), and
//!   - workspace members inherit access through the parent edge.
//!
//! Handlers run inside one uncommitted transaction per connection, exactly as
//! the middleware would, so a grant created via the API is immediately visible
//! to a subsequent preview on the same connection.

use axum::extract::{Extension, Path, Query};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use sqlx::PgPool;
use uuid::Uuid;

use gmrag_api::auth::extractor::AuthUser;
use gmrag_api::auth::jwt::JwtClaims;
use gmrag_api::auth::tenant::TenantContext;
use gmrag_api::error::ApiError;
use gmrag_api::middleware::rls::SharedConnection;
use gmrag_api::routes::acl::{create_grant, list_grants, revoke_grant, AclListParams, CreateGrantBody};
use gmrag_api::routes::documents::preview_document;

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

fn auth(user_id: Uuid) -> AuthUser {
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

async fn insert_workspace(pool: &PgPool, tenant_id: Uuid, created_by: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workspaces (id, tenant_id, name, slug, created_by) VALUES ($1,$2,'ws','ws',$3)",
    )
    .bind(id)
    .bind(tenant_id)
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

async fn insert_document(
    pool: &PgPool,
    tenant_id: Uuid,
    workspace_id: Option<Uuid>,
    owner_id: Uuid,
    visibility: &str,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO documents (id, tenant_id, workspace_id, owner_id, title, visibility)
         VALUES ($1,$2,$3,$4,'Doc',$5)",
    )
    .bind(id)
    .bind(tenant_id)
    .bind(workspace_id)
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

async fn status_of(result: Result<impl IntoResponse, ApiError>) -> StatusCode {
    result.into_response().status()
}

fn share(resource_id: Uuid, ptype: &str, pid: Uuid, relation: &str) -> CreateGrantBody {
    CreateGrantBody {
        resource_type: "document".to_string(),
        resource_id,
        principal_type: ptype.to_string(),
        principal_id: pid,
        relation: relation.to_string(),
    }
}

async fn preview_status(conn: &SharedConnection, tenant: Uuid, doc: Uuid, user: Uuid) -> StatusCode {
    status_of(
        preview_document(
            Path((tenant, doc)),
            Extension(TenantContext(tenant)),
            Extension(auth(user)),
            Extension(conn.clone()),
        )
        .await,
    )
    .await
}

// ─── tests ───────────────────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn e2e_share_grants_access_then_revoke_denies(pool: PgPool) {
    let owner = create_user(&pool, "owner@e85.com").await;
    let friend = create_user(&pool, "friend@e85.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let d = insert_document(&pool, tenant, None, owner, "private").await;

    let conn = rls_conn(&pool, tenant).await;

    // Before sharing: friend cannot preview.
    assert_eq!(preview_status(&conn, tenant, d, friend).await, StatusCode::NOT_FOUND);

    // Owner shares as viewer.
    let created = create_grant(
        Path(tenant),
        Extension(TenantContext(tenant)),
        Extension(auth(owner)),
        Extension(conn.clone()),
        Json(share(d, "user", friend, "viewer")),
    )
    .await
    .unwrap()
    .into_response();
    assert_eq!(created.status(), StatusCode::CREATED);

    // Now friend can preview.
    assert_eq!(preview_status(&conn, tenant, d, friend).await, StatusCode::OK);

    // Find the grant id to revoke it.
    let grant_id: Uuid = {
        let mut guard = conn.lock().await;
        sqlx::query_scalar(
            "SELECT id FROM resource_acl WHERE resource_id = $1 AND principal_id = $2",
        )
        .bind(d)
        .bind(friend)
        .fetch_one(&mut *guard)
        .await
        .unwrap()
    };

    // Owner revokes; access is removed.
    let revoked = revoke_grant(
        Path((tenant, grant_id)),
        Extension(TenantContext(tenant)),
        Extension(auth(owner)),
        Extension(conn.clone()),
    )
    .await
    .unwrap()
    .into_response();
    assert_eq!(revoked.status(), StatusCode::NO_CONTENT);
    assert_eq!(preview_status(&conn, tenant, d, friend).await, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../../migrations")]
async fn pentest_viewer_cannot_reshare(pool: PgPool) {
    let owner = create_user(&pool, "owner@e85e.com").await;
    let viewer = create_user(&pool, "viewer@e85e.com").await;
    let target = create_user(&pool, "target@e85e.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let d = insert_document(&pool, tenant, None, owner, "private").await;

    let conn = rls_conn(&pool, tenant).await;
    // Owner grants viewer to `viewer`.
    create_grant(
        Path(tenant),
        Extension(TenantContext(tenant)),
        Extension(auth(owner)),
        Extension(conn.clone()),
        Json(share(d, "user", viewer, "viewer")),
    )
    .await
    .unwrap();

    // The viewer attempts to escalate by re-sharing to a third party.
    let attempt = create_grant(
        Path(tenant),
        Extension(TenantContext(tenant)),
        Extension(auth(viewer)),
        Extension(conn.clone()),
        Json(share(d, "user", target, "editor")),
    )
    .await;
    assert!(
        matches!(attempt, Err(ApiError::Forbidden(_))),
        "a non-owner must not be able to re-share (privilege escalation)"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn pentest_cross_tenant_share_is_forbidden(pool: PgPool) {
    let attacker = create_user(&pool, "attacker@e85x.com").await;
    let victim = create_user(&pool, "victim@e85x.com").await;
    let tenant_a = insert_tenant(&pool, "A").await;
    let tenant_b = insert_tenant(&pool, "B").await;
    let doc_b = insert_document(&pool, tenant_b, None, victim, "private").await;

    // Attacker operates in tenant A and tries to grant themselves access to a
    // tenant-B document. RLS hides doc_b → owner check fails → forbidden.
    let conn_a = rls_conn(&pool, tenant_a).await;
    let attempt = create_grant(
        Path(tenant_a),
        Extension(TenantContext(tenant_a)),
        Extension(auth(attacker)),
        Extension(conn_a.clone()),
        Json(share(doc_b, "user", attacker, "viewer")),
    )
    .await;
    assert!(matches!(attempt, Err(ApiError::Forbidden(_))));

    // And even if a grant existed in B, A's preview cannot see the document.
    assert_eq!(preview_status(&conn_a, tenant_a, doc_b, attacker).await, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../../migrations")]
async fn e2e_workspace_inheritance_grants_preview(pool: PgPool) {
    let owner = create_user(&pool, "owner@e85w.com").await;
    let member = create_user(&pool, "member@e85w.com").await;
    let outsider = create_user(&pool, "outsider@e85w.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let ws = insert_workspace(&pool, tenant, owner).await;
    add_workspace_member(&pool, ws, tenant, member).await;
    let d = insert_document(&pool, tenant, Some(ws), owner, "private").await;

    let conn = rls_conn(&pool, tenant).await;
    assert_eq!(preview_status(&conn, tenant, d, member).await, StatusCode::OK);
    assert_eq!(preview_status(&conn, tenant, d, outsider).await, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../../migrations")]
async fn pentest_revoked_user_cannot_list_grants(pool: PgPool) {
    let owner = create_user(&pool, "owner@e85l.com").await;
    let friend = create_user(&pool, "friend@e85l.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    let d = insert_document(&pool, tenant, None, owner, "private").await;

    let conn = rls_conn(&pool, tenant).await;
    // Stranger cannot enumerate a private document's grants.
    let listed = list_grants(
        Path(tenant),
        Extension(TenantContext(tenant)),
        Extension(auth(friend)),
        Extension(conn.clone()),
        Query(AclListParams {
            resource_type: "document".to_string(),
            resource_id: d,
        }),
    )
    .await;
    assert!(
        matches!(listed, Err(ApiError::NotFound)),
        "a user without view access must not be able to list a resource's grants"
    );
}
