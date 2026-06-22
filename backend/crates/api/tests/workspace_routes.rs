//! Integration tests for workspace routes (T55).
//!
//! Require a running PostgreSQL instance. `#[sqlx::test]` provisions an
//! isolated database and runs migrations automatically. The `DATABASE_URL`
//! user is a superuser (bypasses RLS); tenant-scoped handlers are exercised
//! through a [`SharedConnection`] built like `rls_middleware` does:
//! `BEGIN; SET LOCAL ROLE gmrag_app; SET LOCAL app.tenant_id = '<uuid>'`.

use axum::extract::{Extension, Path};
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
use gmrag_api::routes::workspaces::{
    create_workspace, delete_workspace, list_workspaces, update_workspace, CreateWorkspaceBody,
    UpdateWorkspaceBody,
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

async fn add_tenant_member(pool: &PgPool, tenant_id: Uuid, user_id: Uuid, role: &str) {
    sqlx::query("INSERT INTO tenant_members (tenant_id, user_id, role) VALUES ($1, $2, $3)")
        .bind(tenant_id)
        .bind(user_id)
        .bind(role)
        .execute(pool)
        .await
        .unwrap();
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

/// Build a `SharedConnection` whose transaction has RLS active for `tenant_id`,
/// mirroring `rls_middleware`.
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

// ─── T55: workspaces CRUD ────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn create_workspace_succeeds_and_lists(pool: PgPool) {
    let owner = create_user(&pool, "owner@t55.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    add_tenant_member(&pool, tenant, owner, "owner").await;

    let conn = rls_conn(&pool, tenant).await;
    let (status, body) = parts(
        create_workspace(
            Path(tenant),
            Extension(TenantContext(tenant)),
            Extension(auth_user(owner)),
            Extension(conn.clone()),
            Json(CreateWorkspaceBody {
                name: "Engineering".into(),
                slug: "engineering".into(),
            }),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["name"], "Engineering");
    assert_eq!(body["slug"], "engineering");
    assert_eq!(body["created_by"].as_str().unwrap(), owner.to_string());

    let (status, body) = parts(
        list_workspaces(
            Path(tenant),
            Extension(TenantContext(tenant)),
            Extension(conn),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let workspaces = body["workspaces"].as_array().unwrap();
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0]["slug"], "engineering");
}

#[sqlx::test(migrations = "../../migrations")]
async fn create_workspace_rejects_empty_name(pool: PgPool) {
    let owner = create_user(&pool, "x@t55.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    add_tenant_member(&pool, tenant, owner, "owner").await;

    let conn = rls_conn(&pool, tenant).await;
    let result = create_workspace(
        Path(tenant),
        Extension(TenantContext(tenant)),
        Extension(auth_user(owner)),
        Extension(conn),
        Json(CreateWorkspaceBody {
            name: "   ".into(),
            slug: "ok".into(),
        }),
    )
    .await;
    assert!(matches!(result, Err(ApiError::BadRequest(_))));
}

#[sqlx::test(migrations = "../../migrations")]
async fn update_workspace_renames(pool: PgPool) {
    let owner = create_user(&pool, "owner@t55u.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    add_tenant_member(&pool, tenant, owner, "owner").await;
    let ws = insert_workspace(&pool, tenant, owner, "old").await;

    let conn = rls_conn(&pool, tenant).await;
    let (status, body) = parts(
        update_workspace(
            Path((tenant, ws)),
            Extension(TenantContext(tenant)),
            Extension(conn),
            Json(UpdateWorkspaceBody {
                name: "New Name".into(),
                slug: "new-name".into(),
            }),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"], "New Name");
    assert_eq!(body["slug"], "new-name");
}

#[sqlx::test(migrations = "../../migrations")]
async fn delete_workspace_removes(pool: PgPool) {
    let owner = create_user(&pool, "owner@t55d.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    add_tenant_member(&pool, tenant, owner, "owner").await;
    let ws = insert_workspace(&pool, tenant, owner, "doomed").await;

    let conn = rls_conn(&pool, tenant).await;
    let (status, _) = parts(
        delete_workspace(
            Path((tenant, ws)),
            Extension(TenantContext(tenant)),
            Extension(conn.clone()),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let mut guard = conn.lock().await;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workspaces WHERE id = $1")
        .bind(ws)
        .fetch_one(&mut *guard)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn cross_tenant_workspace_update_is_not_found(pool: PgPool) {
    let user_a = create_user(&pool, "a@t55x.com").await;
    let user_b = create_user(&pool, "b@t55x.com").await;
    let tenant_a = insert_tenant(&pool, "A").await;
    let tenant_b = insert_tenant(&pool, "B").await;
    add_tenant_member(&pool, tenant_a, user_a, "owner").await;
    add_tenant_member(&pool, tenant_b, user_b, "owner").await;
    let ws_b = insert_workspace(&pool, tenant_b, user_b, "secret").await;

    // Tenant A's RLS context tries to touch tenant B's workspace, supplying
    // tenant B's id only in the path. The path guard checks against tenant A's
    // context, so it is rejected before any query runs.
    let conn = rls_conn(&pool, tenant_a).await;
    let result = update_workspace(
        Path((tenant_b, ws_b)),
        Extension(TenantContext(tenant_a)),
        Extension(conn),
        Json(UpdateWorkspaceBody {
            name: "hax".into(),
            slug: "hax".into(),
        }),
    )
    .await;
    assert!(matches!(result, Err(ApiError::BadRequest(_))));
}

#[sqlx::test(migrations = "../../migrations")]
async fn workspace_update_unknown_id_is_not_found(pool: PgPool) {
    let owner = create_user(&pool, "owner@t55n.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    add_tenant_member(&pool, tenant, owner, "owner").await;

    let conn = rls_conn(&pool, tenant).await;
    let result = update_workspace(
        Path((tenant, Uuid::new_v4())),
        Extension(TenantContext(tenant)),
        Extension(conn),
        Json(UpdateWorkspaceBody {
            name: "Nope".into(),
            slug: "nope".into(),
        }),
    )
    .await;
    assert!(matches!(result, Err(ApiError::NotFound)));
}
