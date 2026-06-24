//! Integration tests for tenant routes (T52 + T53 + T54).
//!
//! Require a running PostgreSQL instance. `#[sqlx::test]` provisions an
//! isolated database and runs migrations automatically. The `DATABASE_URL`
//! user is a superuser (bypasses RLS); tenant-scoped handlers are exercised
//! through a [`SharedConnection`] built like `rls_middleware` does:
//! `BEGIN; SET LOCAL ROLE gmrag_app; SET LOCAL app.tenant_id = '<uuid>'`.

use std::sync::Arc;

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
use gmrag_api::pool::AdminPool;
use gmrag_api::routes::tenant_members::{
    invite_member, list_members, remove_member, InviteBody,
};
use gmrag_api::routes::tenants::{
    create_tenant, delete_tenant, list_tenants, update_tenant, CreateTenantBody, UpdateTenantBody,
};
use gmrag_api::storage::ObjectStore;
use gmrag_core::config::QdrantConfig;
use gmrag_core::QdrantStore;

/// T84D Phase 2.2: a no-op ObjectStore so delete_tenant tests don't need
/// live MinIO. The handler best-effort-logs `delete_prefix` failures and
/// continues, so returning `Ok(())` here keeps the cascade delete intact.
struct NoopObjectStore;
#[async_trait::async_trait]
impl ObjectStore for NoopObjectStore {
    async fn put(&self, _key: &str, _data: Vec<u8>, _content_type: &str) -> Result<(), String> {
        Ok(())
    }
    async fn delete(&self, _key: &str) -> Result<(), String> {
        Ok(())
    }
    async fn delete_prefix(&self, _prefix: &str) -> Result<(), String> {
        Ok(())
    }
}

async fn maybe_qdrant() -> Option<QdrantStore> {
    QdrantStore::new(&QdrantConfig {
        url: "http://localhost:6334".into(),
        api_key: None,
        collection_default: "gmrag_chunks".into(),
    })
    .await
    .ok()
}

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

async fn add_member(pool: &PgPool, tenant_id: Uuid, user_id: Uuid, role: &str) {
    sqlx::query("INSERT INTO tenant_members (tenant_id, user_id, role) VALUES ($1, $2, $3)")
        .bind(tenant_id)
        .bind(user_id)
        .bind(role)
        .execute(pool)
        .await
        .unwrap();
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

// ─── T52: GET/POST /tenants ──────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn create_tenant_makes_owner_and_lists(pool: PgPool) {
    let user = create_user(&pool, "owner@t52.com").await;

    let (status, body) = parts(
        create_tenant(
            Extension(auth_user(user)),
            Extension(AdminPool(pool.clone())),
            Json(CreateTenantBody {
                name: "Acme".into(),
            }),
        )
        .await,
    )
    .await;

    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["role"], "owner");
    let tenant_id = Uuid::parse_str(body["id"].as_str().unwrap()).unwrap();

    // Membership row created with owner role.
    let role: String = sqlx::query_scalar(
        "SELECT role FROM tenant_members WHERE tenant_id = $1 AND user_id = $2",
    )
    .bind(tenant_id)
    .bind(user)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(role, "owner");

    // GET /tenants lists it.
    let (status, body) = parts(
        list_tenants(Extension(auth_user(user)), Extension(AdminPool(pool.clone()))).await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let tenants = body["tenants"].as_array().unwrap();
    assert_eq!(tenants.len(), 1);
    assert_eq!(tenants[0]["id"].as_str().unwrap(), tenant_id.to_string());
    assert_eq!(tenants[0]["role"], "owner");
}

#[sqlx::test(migrations = "../../migrations")]
async fn create_tenant_rejects_empty_name(pool: PgPool) {
    let user = create_user(&pool, "x@t52.com").await;
    let result = create_tenant(
        Extension(auth_user(user)),
        Extension(AdminPool(pool.clone())),
        Json(CreateTenantBody { name: "   ".into() }),
    )
    .await;
    assert!(matches!(result, Err(ApiError::BadRequest(_))));
}

#[sqlx::test(migrations = "../../migrations")]
async fn list_tenants_only_returns_membership(pool: PgPool) {
    let user = create_user(&pool, "u@t52.com").await;
    let other = create_user(&pool, "o@t52.com").await;
    let t_mine = insert_tenant(&pool, "Mine").await;
    let t_other = insert_tenant(&pool, "Other").await;
    add_member(&pool, t_mine, user, "owner").await;
    add_member(&pool, t_other, other, "owner").await;

    let (status, body) =
        parts(list_tenants(Extension(auth_user(user)), Extension(AdminPool(pool.clone()))).await)
            .await;
    assert_eq!(status, StatusCode::OK);
    let tenants = body["tenants"].as_array().unwrap();
    assert_eq!(tenants.len(), 1);
    assert_eq!(tenants[0]["id"].as_str().unwrap(), t_mine.to_string());
}

// ─── T53: PATCH/DELETE /tenants/{tid} ────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn owner_can_rename_tenant(pool: PgPool) {
    let owner = create_user(&pool, "owner@t53.com").await;
    let tenant = insert_tenant(&pool, "Old").await;
    add_member(&pool, tenant, owner, "owner").await;

    let conn = rls_conn(&pool, tenant).await;
    let (status, body) = parts(
        update_tenant(
            Path(tenant),
            Extension(TenantContext(tenant)),
            Extension(auth_user(owner)),
            Extension(conn.clone()),
            Json(UpdateTenantBody { name: "New".into() }),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"], "New");

    // Verify within the same RLS transaction.
    let mut guard = conn.lock().await;
    let name: String = sqlx::query_scalar("SELECT name FROM tenants WHERE id = $1")
        .bind(tenant)
        .fetch_one(&mut *guard)
        .await
        .unwrap();
    assert_eq!(name, "New");
}

#[sqlx::test(migrations = "../../migrations")]
async fn non_owner_cannot_rename_tenant(pool: PgPool) {
    let member = create_user(&pool, "member@t53.com").await;
    let tenant = insert_tenant(&pool, "Old").await;
    add_member(&pool, tenant, member, "member").await;

    let conn = rls_conn(&pool, tenant).await;
    let result = update_tenant(
        Path(tenant),
        Extension(TenantContext(tenant)),
        Extension(auth_user(member)),
        Extension(conn),
        Json(UpdateTenantBody { name: "Nope".into() }),
    )
    .await;
    assert!(matches!(result, Err(ApiError::Forbidden(_))));
}

#[sqlx::test(migrations = "../../migrations")]
async fn rename_with_mismatched_path_is_bad_request(pool: PgPool) {
    let owner = create_user(&pool, "owner@t53b.com").await;
    let tenant = insert_tenant(&pool, "Old").await;
    add_member(&pool, tenant, owner, "owner").await;

    let conn = rls_conn(&pool, tenant).await;
    let result = update_tenant(
        Path(Uuid::new_v4()), // path tid != context
        Extension(TenantContext(tenant)),
        Extension(auth_user(owner)),
        Extension(conn),
        Json(UpdateTenantBody { name: "New".into() }),
    )
    .await;
    assert!(matches!(result, Err(ApiError::BadRequest(_))));
}

#[sqlx::test(migrations = "../../migrations")]
async fn owner_can_delete_tenant_with_cascade(pool: PgPool) {
    let owner = create_user(&pool, "owner@t53c.com").await;
    let tenant = insert_tenant(&pool, "Doomed").await;
    add_member(&pool, tenant, owner, "owner").await;

    let conn = rls_conn(&pool, tenant).await;
    // T84D Phase 2.2: handler now requires QdrantStore + Arc<dyn ObjectStore>.
    // Skip when live Qdrant isn't available; otherwise inject a no-op store.
    let qdrant = match maybe_qdrant().await {
        Some(s) => s,
        None => return,
    };
    let object_store: Arc<dyn ObjectStore> = Arc::new(NoopObjectStore);
    let (status, _) = parts(
        delete_tenant(
            Path(tenant),
            Extension(TenantContext(tenant)),
            Extension(auth_user(owner)),
            Extension(conn.clone()),
            Extension(qdrant),
            Extension(object_store),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Commit so the deletion is observable via the superuser pool.
    {
        let mut guard = conn.lock().await;
        sqlx::Executor::execute(&mut *guard, "COMMIT").await.unwrap();
    }

    let tenant_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tenants WHERE id = $1")
        .bind(tenant)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(tenant_count, 0);

    let member_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM tenant_members WHERE tenant_id = $1")
            .bind(tenant)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(member_count, 0, "cascade must remove memberships");
}

#[sqlx::test(migrations = "../../migrations")]
async fn non_owner_cannot_delete_tenant(pool: PgPool) {
    let member = create_user(&pool, "member@t53d.com").await;
    let tenant = insert_tenant(&pool, "Safe").await;
    add_member(&pool, tenant, member, "member").await;

    let conn = rls_conn(&pool, tenant).await;
    // Non-owner path bails on the require_owner guard BEFORE touching Qdrant
    // or S3, so live Qdrant availability doesn't matter. Still pass stubs
    // to satisfy the new handler signature.
    let qdrant = match maybe_qdrant().await {
        Some(s) => s,
        None => return,
    };
    let object_store: Arc<dyn ObjectStore> = Arc::new(NoopObjectStore);
    let result = delete_tenant(
        Path(tenant),
        Extension(TenantContext(tenant)),
        Extension(auth_user(member)),
        Extension(conn),
        Extension(qdrant),
        Extension(object_store),
    )
    .await;
    assert!(matches!(result, Err(ApiError::Forbidden(_))));
}

// ─── T54: members list / invite / remove ─────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn list_members_is_tenant_isolated(pool: PgPool) {
    let user_a = create_user(&pool, "a@t54.com").await;
    let user_b = create_user(&pool, "b@t54.com").await;
    let tenant_a = insert_tenant(&pool, "A").await;
    let tenant_b = insert_tenant(&pool, "B").await;
    add_member(&pool, tenant_a, user_a, "owner").await;
    add_member(&pool, tenant_b, user_b, "owner").await;

    let conn = rls_conn(&pool, tenant_a).await;
    let (status, body) = parts(
        list_members(
            Path(tenant_a),
            Extension(TenantContext(tenant_a)),
            Extension(conn),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let members = body["members"].as_array().unwrap();
    assert_eq!(members.len(), 1, "RLS must hide tenant B members");
    assert_eq!(members[0]["user_id"].as_str().unwrap(), user_a.to_string());
}

#[sqlx::test(migrations = "../../migrations")]
async fn owner_can_invite_member(pool: PgPool) {
    let owner = create_user(&pool, "owner@t54i.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    add_member(&pool, tenant, owner, "owner").await;

    let conn = rls_conn(&pool, tenant).await;
    let (status, body) = parts(
        invite_member(
            Path(tenant),
            Extension(TenantContext(tenant)),
            Extension(auth_user(owner)),
            Extension(conn.clone()),
            Json(InviteBody {
                email: "invitee@t54i.com".into(),
                role: Some("admin".into()),
            }),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["email"], "invitee@t54i.com");
    assert_eq!(body["role"], "admin");
    assert_eq!(body["status"], "pending");

    // Pending invitation row exists in the same RLS transaction.
    let mut guard = conn.lock().await;
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM invitations WHERE email = $1 AND status = 'pending'",
    )
    .bind("invitee@t54i.com")
    .fetch_one(&mut *guard)
    .await
    .unwrap();
    assert_eq!(count, 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn non_owner_cannot_invite(pool: PgPool) {
    let member = create_user(&pool, "member@t54i.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    add_member(&pool, tenant, member, "member").await;

    let conn = rls_conn(&pool, tenant).await;
    let result = invite_member(
        Path(tenant),
        Extension(TenantContext(tenant)),
        Extension(auth_user(member)),
        Extension(conn),
        Json(InviteBody {
            email: "x@t54i.com".into(),
            role: None,
        }),
    )
    .await;
    assert!(matches!(result, Err(ApiError::Forbidden(_))));
}

#[sqlx::test(migrations = "../../migrations")]
async fn owner_can_remove_member(pool: PgPool) {
    let owner = create_user(&pool, "owner@t54r.com").await;
    let other = create_user(&pool, "other@t54r.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    add_member(&pool, tenant, owner, "owner").await;
    add_member(&pool, tenant, other, "member").await;

    let conn = rls_conn(&pool, tenant).await;
    let (status, _) = parts(
        remove_member(
            Path((tenant, other)),
            Extension(TenantContext(tenant)),
            Extension(auth_user(owner)),
            Extension(conn.clone()),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let mut guard = conn.lock().await;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tenant_members WHERE user_id = $1")
        .bind(other)
        .fetch_one(&mut *guard)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn cannot_remove_last_owner(pool: PgPool) {
    let owner = create_user(&pool, "soleowner@t54r.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    add_member(&pool, tenant, owner, "owner").await;

    let conn = rls_conn(&pool, tenant).await;
    let result = remove_member(
        Path((tenant, owner)),
        Extension(TenantContext(tenant)),
        Extension(auth_user(owner)),
        Extension(conn),
    )
    .await;
    assert!(matches!(result, Err(ApiError::BadRequest(_))));
}
