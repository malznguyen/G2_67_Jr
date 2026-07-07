//! Integration tests for workspace routes (T55 + T56).
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
use gmrag_api::authz::{
    document_obj, user_obj, AuthzService, CheckRequest, PgTestAuthorizationService, REL_VIEWER,
};
use gmrag_api::error::ApiError;
use gmrag_api::middleware::rls::SharedConnection;
use gmrag_api::routes::workspaces::{
    create_workspace, delete_workspace, list_workspaces, update_workspace, CreateWorkspaceBody,
    UpdateWorkspaceBody,
};
use gmrag_api::routes::ws_members::{
    add_member as ws_add_member, list_members as ws_list_members,
    remove_member as ws_remove_member, AddMemberBody,
};
use gmrag_api::storage::ObjectStore;
use gmrag_api::vector::VectorCleaner;

// ─── Phase 0 cleanup mocks ───────────────────────────────────────────────────
// A recording VectorCleaner + noop ObjectStore so delete_workspace tests do
// not need live Qdrant/MinIO. The handler best-effort-logs cleanup failures
// and continues, so returning Ok(()) keeps the cascade delete intact.

use std::sync::Mutex;

#[derive(Default)]
struct RecordingCleaner {
    workspace_deletes: Mutex<Vec<(Uuid, Uuid)>>,
}

#[async_trait::async_trait]
impl VectorCleaner for RecordingCleaner {
    async fn delete_document_chunks(
        &self,
        _tenant_id: Uuid,
        _document_id: Uuid,
    ) -> Result<(), String> {
        Ok(())
    }
    async fn delete_workspace_chunks(
        &self,
        tenant_id: Uuid,
        workspace_id: Uuid,
    ) -> Result<(), String> {
        self.workspace_deletes
            .lock()
            .unwrap()
            .push((tenant_id, workspace_id));
        Ok(())
    }
}

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

#[derive(Default)]
struct RecordingObjectStore {
    prefix_deletes: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl ObjectStore for RecordingObjectStore {
    async fn put(&self, _key: &str, _data: Vec<u8>, _content_type: &str) -> Result<(), String> {
        Ok(())
    }
    async fn delete(&self, _key: &str) -> Result<(), String> {
        Ok(())
    }
    async fn delete_prefix(&self, prefix: &str) -> Result<(), String> {
        self.prefix_deletes.lock().unwrap().push(prefix.to_string());
        Ok(())
    }
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

fn test_authz(pool: &PgPool) -> AuthzService {
    Arc::new(PgTestAuthorizationService::new(pool.clone()))
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

async fn add_workspace_member(
    pool: &PgPool,
    tenant_id: Uuid,
    workspace_id: Uuid,
    user_id: Uuid,
    role: &str,
) {
    sqlx::query(
        "INSERT INTO workspace_members (workspace_id, tenant_id, user_id, role)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(workspace_id)
    .bind(tenant_id)
    .bind(user_id)
    .bind(role)
    .execute(pool)
    .await
    .unwrap();
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

async fn remove_member_and_commit(
    conn: SharedConnection,
    authz: AuthzService,
    tenant: Uuid,
    workspace: Uuid,
    target: Uuid,
    caller: Uuid,
) -> StatusCode {
    let (status, _) = parts(
        ws_remove_member(
            Path((tenant, workspace, target)),
            Extension(TenantContext(tenant)),
            Extension(auth_user(caller)),
            Extension(conn.clone()),
            Extension(authz),
        )
        .await,
    )
    .await;
    let mut guard = conn.lock().await;
    sqlx::Executor::execute(&mut *guard, "COMMIT")
        .await
        .unwrap();
    status
}

// ─── T55: workspaces CRUD ────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn create_workspace_succeeds_and_lists(pool: PgPool) {
    let owner = create_user(&pool, "owner@t55.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    add_tenant_member(&pool, tenant, owner, "owner").await;

    let conn = rls_conn(&pool, tenant).await;
    let authz = test_authz(&pool);
    let (status, body) = parts(
        create_workspace(
            Path(tenant),
            Extension(TenantContext(tenant)),
            Extension(auth_user(owner)),
            Extension(conn.clone()),
            Extension(authz.clone()),
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

    {
        let mut guard = conn.lock().await;
        sqlx::Executor::execute(&mut *guard, "COMMIT")
            .await
            .unwrap();
    }
    let conn = rls_conn(&pool, tenant).await;
    let (status, body) = parts(
        list_workspaces(
            Path(tenant),
            Extension(TenantContext(tenant)),
            Extension(auth_user(owner)),
            Extension(conn),
            Extension(authz),
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
async fn create_workspace_bootstraps_creator_as_owner_and_document_viewer(pool: PgPool) {
    let creator = create_user(&pool, "creator@p1create.com").await;
    let other = create_user(&pool, "other@p1create.com").await;
    let tenant = insert_tenant(&pool, "P1 Create").await;
    add_tenant_member(&pool, tenant, creator, "member").await;
    add_tenant_member(&pool, tenant, other, "member").await;

    let conn = rls_conn(&pool, tenant).await;
    let authz = test_authz(&pool);
    let (status, body) = parts(
        create_workspace(
            Path(tenant),
            Extension(TenantContext(tenant)),
            Extension(auth_user(creator)),
            Extension(conn.clone()),
            Extension(authz.clone()),
            Json(CreateWorkspaceBody {
                name: "Bootstrap".into(),
                slug: "bootstrap".into(),
            }),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let ws = Uuid::parse_str(body["id"].as_str().unwrap()).unwrap();

    let mut guard = conn.lock().await;
    let role: String = sqlx::query_scalar(
        "SELECT role FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(ws)
    .bind(creator)
    .fetch_one(&mut *guard)
    .await
    .unwrap();
    assert_eq!(role, "owner");

    let doc: Uuid = sqlx::query_scalar(
        "INSERT INTO documents (tenant_id, workspace_id, owner_id, title, status, visibility)
         VALUES ($1, $2, $3, 'Bootstrap Doc', 'indexed', 'private')
         RETURNING id",
    )
    .bind(tenant)
    .bind(ws)
    .bind(other)
    .fetch_one(&mut *guard)
    .await
    .unwrap();

    sqlx::Executor::execute(&mut *guard, "COMMIT")
        .await
        .unwrap();
    drop(guard);

    let can_view = authz
        .check(CheckRequest::new(
            user_obj(creator),
            REL_VIEWER,
            document_obj(doc),
        ))
        .await
        .unwrap();
    assert!(
        can_view,
        "creator should inherit document viewer access through workspace membership"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn create_workspace_rolls_back_when_owner_bootstrap_fails(pool: PgPool) {
    let creator = create_user(&pool, "creator@p15atomic.com").await;
    let tenant = insert_tenant(&pool, "P1.5 Atomic").await;
    add_tenant_member(&pool, tenant, creator, "member").await;

    sqlx::query(
        r#"
        CREATE FUNCTION test_block_workspace_member_insert()
        RETURNS trigger
        LANGUAGE plpgsql
        AS $$
        BEGIN
            RAISE EXCEPTION 'blocked workspace member insert';
        END;
        $$;
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        CREATE TRIGGER test_block_workspace_member_insert
        BEFORE INSERT ON workspace_members
        FOR EACH ROW
        EXECUTE FUNCTION test_block_workspace_member_insert();
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();

    let conn = rls_conn(&pool, tenant).await;
    let result = create_workspace(
        Path(tenant),
        Extension(TenantContext(tenant)),
        Extension(auth_user(creator)),
        Extension(conn),
        Extension(test_authz(&pool)),
        Json(CreateWorkspaceBody {
            name: "Atomic".into(),
            slug: "atomic-fail".into(),
        }),
    )
    .await;
    assert!(matches!(result, Err(ApiError::Internal(_))));

    sqlx::query("DROP TRIGGER test_block_workspace_member_insert ON workspace_members")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DROP FUNCTION test_block_workspace_member_insert()")
        .execute(&pool)
        .await
        .unwrap();

    let workspaces: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM workspaces WHERE slug = 'atomic-fail'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let memberships: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)
         FROM workspace_members wm
         JOIN workspaces w ON w.id = wm.workspace_id
         WHERE w.slug = 'atomic-fail'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(workspaces, 0);
    assert_eq!(memberships, 0);
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
        Extension(test_authz(&pool)),
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
            Extension(auth_user(owner)),
            Extension(conn),
            Extension(test_authz(&pool)),
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
async fn workspace_admin_can_patch_but_tenant_admin_alone_cannot(pool: PgPool) {
    let creator = create_user(&pool, "creator@p1patch.com").await;
    let ws_admin = create_user(&pool, "wsadmin@p1patch.com").await;
    let tenant_admin = create_user(&pool, "tenantadmin@p1patch.com").await;
    let tenant = insert_tenant(&pool, "P1 Patch").await;
    add_tenant_member(&pool, tenant, creator, "member").await;
    add_tenant_member(&pool, tenant, ws_admin, "member").await;
    add_tenant_member(&pool, tenant, tenant_admin, "admin").await;
    let ws = insert_workspace(&pool, tenant, creator, "patch").await;
    add_workspace_member(&pool, tenant, ws, ws_admin, "admin").await;

    let conn = rls_conn(&pool, tenant).await;
    let (status, _) = parts(
        update_workspace(
            Path((tenant, ws)),
            Extension(TenantContext(tenant)),
            Extension(auth_user(ws_admin)),
            Extension(conn.clone()),
            Extension(test_authz(&pool)),
            Json(UpdateWorkspaceBody {
                name: "Patched".into(),
                slug: "patched".into(),
            }),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let result = update_workspace(
        Path((tenant, ws)),
        Extension(TenantContext(tenant)),
        Extension(auth_user(tenant_admin)),
        Extension(conn),
        Extension(test_authz(&pool)),
        Json(UpdateWorkspaceBody {
            name: "Denied".into(),
            slug: "denied".into(),
        }),
    )
    .await;
    assert!(matches!(result, Err(ApiError::Forbidden(_))));
}

#[sqlx::test(migrations = "../../migrations")]
async fn delete_workspace_removes(pool: PgPool) {
    let owner = create_user(&pool, "owner@t55d.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    add_tenant_member(&pool, tenant, owner, "owner").await;
    let ws = insert_workspace(&pool, tenant, owner, "doomed").await;

    let conn = rls_conn(&pool, tenant).await;
    let cleaner = Arc::new(RecordingCleaner::default()) as Arc<dyn VectorCleaner>;
    let store = Arc::new(NoopObjectStore) as Arc<dyn ObjectStore>;
    let (status, _) = parts(
        delete_workspace(
            Path((tenant, ws)),
            Extension(TenantContext(tenant)),
            Extension(auth_user(owner)),
            Extension(conn.clone()),
            Extension(test_authz(&pool)),
            Extension(cleaner),
            Extension(store),
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
async fn unauthorized_workspace_delete_performs_no_external_cleanup(pool: PgPool) {
    let creator = create_user(&pool, "creator@p1delete.com").await;
    let member = create_user(&pool, "member@p1delete.com").await;
    let tenant = insert_tenant(&pool, "P1 Delete").await;
    add_tenant_member(&pool, tenant, creator, "member").await;
    add_tenant_member(&pool, tenant, member, "member").await;
    let ws = insert_workspace(&pool, tenant, creator, "delete-guard").await;
    add_workspace_member(&pool, tenant, ws, member, "member").await;

    let conn = rls_conn(&pool, tenant).await;
    let cleaner = Arc::new(RecordingCleaner::default());
    let store = Arc::new(RecordingObjectStore::default());
    let (status, _) = parts(
        delete_workspace(
            Path((tenant, ws)),
            Extension(TenantContext(tenant)),
            Extension(auth_user(member)),
            Extension(conn.clone()),
            Extension(test_authz(&pool)),
            Extension(cleaner.clone() as Arc<dyn VectorCleaner>),
            Extension(store.clone() as Arc<dyn ObjectStore>),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(cleaner.workspace_deletes.lock().unwrap().is_empty());
    assert!(store.prefix_deletes.lock().unwrap().is_empty());

    let mut guard = conn.lock().await;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workspaces WHERE id = $1")
        .bind(ws)
        .fetch_one(&mut *guard)
        .await
        .unwrap();
    assert_eq!(count, 1);
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
        Extension(auth_user(user_a)),
        Extension(conn),
        Extension(test_authz(&pool)),
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
        Extension(auth_user(owner)),
        Extension(conn),
        Extension(test_authz(&pool)),
        Json(UpdateWorkspaceBody {
            name: "Nope".into(),
            slug: "nope".into(),
        }),
    )
    .await;
    assert!(matches!(result, Err(ApiError::NotFound)));
}

// ─── T56: workspace members ──────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn add_and_list_workspace_members(pool: PgPool) {
    let owner = create_user(&pool, "owner@t56.com").await;
    let member = create_user(&pool, "member@t56.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    add_tenant_member(&pool, tenant, owner, "owner").await;
    add_tenant_member(&pool, tenant, member, "member").await;
    let ws = insert_workspace(&pool, tenant, owner, "eng").await;

    let conn = rls_conn(&pool, tenant).await;
    let authz = test_authz(&pool);
    let (status, body) = parts(
        ws_add_member(
            Path((tenant, ws)),
            Extension(TenantContext(tenant)),
            Extension(auth_user(owner)),
            Extension(conn.clone()),
            Extension(authz.clone()),
            Json(AddMemberBody {
                user_id: member,
                role: Some("admin".into()),
            }),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["user_id"].as_str().unwrap(), member.to_string());
    assert_eq!(body["role"], "admin");

    {
        let mut guard = conn.lock().await;
        sqlx::Executor::execute(&mut *guard, "COMMIT")
            .await
            .unwrap();
    }
    let conn = rls_conn(&pool, tenant).await;
    let (status, body) = parts(
        ws_list_members(
            Path((tenant, ws)),
            Extension(TenantContext(tenant)),
            Extension(auth_user(owner)),
            Extension(conn),
            Extension(authz),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let members = body["members"].as_array().unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0]["user_id"].as_str().unwrap(), member.to_string());
    assert_eq!(members[0]["email"], "member@t56.com");
}

#[sqlx::test(migrations = "../../migrations")]
async fn list_workspace_members_requires_workspace_access(pool: PgPool) {
    let creator = create_user(&pool, "creator@p15list.com").await;
    let ws_admin = create_user(&pool, "wsadmin@p15list.com").await;
    let ws_member = create_user(&pool, "wsmember@p15list.com").await;
    let tenant_owner = create_user(&pool, "tenantowner@p15list.com").await;
    let tenant_admin = create_user(&pool, "tenantadmin@p15list.com").await;
    let unrelated = create_user(&pool, "unrelated@p15list.com").await;
    let other_user = create_user(&pool, "other@p15list.com").await;
    let tenant = insert_tenant(&pool, "P1.5 List").await;
    let other_tenant = insert_tenant(&pool, "P1.5 List Other").await;
    for user in [
        creator,
        ws_admin,
        ws_member,
        tenant_owner,
        tenant_admin,
        unrelated,
    ] {
        let role = if user == tenant_owner {
            "owner"
        } else if user == tenant_admin {
            "admin"
        } else {
            "member"
        };
        add_tenant_member(&pool, tenant, user, role).await;
    }
    add_tenant_member(&pool, other_tenant, other_user, "owner").await;
    let ws = insert_workspace(&pool, tenant, creator, "list-guard").await;
    let other_ws = insert_workspace(&pool, other_tenant, other_user, "list-other").await;
    add_workspace_member(&pool, tenant, ws, creator, "owner").await;
    add_workspace_member(&pool, tenant, ws, ws_admin, "admin").await;
    add_workspace_member(&pool, tenant, ws, ws_member, "member").await;

    for allowed in [creator, ws_admin, ws_member, tenant_owner] {
        let conn = rls_conn(&pool, tenant).await;
        let (status, body) = parts(
            ws_list_members(
                Path((tenant, ws)),
                Extension(TenantContext(tenant)),
                Extension(auth_user(allowed)),
                Extension(conn),
                Extension(test_authz(&pool)),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["members"].as_array().unwrap().len(), 3);
    }

    for denied in [tenant_admin, unrelated] {
        let conn = rls_conn(&pool, tenant).await;
        let (status, _) = parts(
            ws_list_members(
                Path((tenant, ws)),
                Extension(TenantContext(tenant)),
                Extension(auth_user(denied)),
                Extension(conn),
                Extension(test_authz(&pool)),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    let conn = rls_conn(&pool, tenant).await;
    let (status, _) = parts(
        ws_list_members(
            Path((tenant, other_ws)),
            Extension(TenantContext(tenant)),
            Extension(auth_user(tenant_owner)),
            Extension(conn),
            Extension(test_authz(&pool)),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../../migrations")]
async fn add_member_defaults_role_to_member(pool: PgPool) {
    let owner = create_user(&pool, "owner@t56def.com").await;
    let member = create_user(&pool, "member@t56def.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    add_tenant_member(&pool, tenant, owner, "owner").await;
    add_tenant_member(&pool, tenant, member, "member").await;
    let ws = insert_workspace(&pool, tenant, owner, "eng").await;

    let conn = rls_conn(&pool, tenant).await;
    let (status, body) = parts(
        ws_add_member(
            Path((tenant, ws)),
            Extension(TenantContext(tenant)),
            Extension(auth_user(owner)),
            Extension(conn),
            Extension(test_authz(&pool)),
            Json(AddMemberBody {
                user_id: member,
                role: None,
            }),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["role"], "member");
}

#[sqlx::test(migrations = "../../migrations")]
async fn workspace_member_add_requires_manager_and_target_tenant_member(pool: PgPool) {
    let creator = create_user(&pool, "creator@p1add.com").await;
    let ws_member = create_user(&pool, "wsmember@p1add.com").await;
    let target = create_user(&pool, "target@p1add.com").await;
    let outsider = create_user(&pool, "outsider@p1add.com").await;
    let tenant = insert_tenant(&pool, "P1 Add").await;
    add_tenant_member(&pool, tenant, creator, "member").await;
    add_tenant_member(&pool, tenant, ws_member, "member").await;
    add_tenant_member(&pool, tenant, target, "member").await;
    let ws = insert_workspace(&pool, tenant, creator, "add-guard").await;
    add_workspace_member(&pool, tenant, ws, creator, "owner").await;
    add_workspace_member(&pool, tenant, ws, ws_member, "member").await;

    let conn = rls_conn(&pool, tenant).await;
    let denied = ws_add_member(
        Path((tenant, ws)),
        Extension(TenantContext(tenant)),
        Extension(auth_user(ws_member)),
        Extension(conn.clone()),
        Extension(test_authz(&pool)),
        Json(AddMemberBody {
            user_id: target,
            role: Some("member".into()),
        }),
    )
    .await;
    assert!(matches!(denied, Err(ApiError::Forbidden(_))));

    let (status, _) = parts(
        ws_add_member(
            Path((tenant, ws)),
            Extension(TenantContext(tenant)),
            Extension(auth_user(creator)),
            Extension(conn.clone()),
            Extension(test_authz(&pool)),
            Json(AddMemberBody {
                user_id: outsider,
                role: Some("member".into()),
            }),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let mut guard = conn.lock().await;
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM workspace_members WHERE user_id = $1")
            .bind(outsider)
            .fetch_one(&mut *guard)
            .await
            .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn add_member_rejects_path_mismatch(pool: PgPool) {
    let owner = create_user(&pool, "owner@t56pm.com").await;
    let member = create_user(&pool, "member@t56pm.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    add_tenant_member(&pool, tenant, owner, "owner").await;
    let ws = insert_workspace(&pool, tenant, owner, "eng").await;

    let conn = rls_conn(&pool, tenant).await;
    let result = ws_add_member(
        Path((Uuid::new_v4(), ws)),
        Extension(TenantContext(tenant)),
        Extension(auth_user(owner)),
        Extension(conn),
        Extension(test_authz(&pool)),
        Json(AddMemberBody {
            user_id: member,
            role: None,
        }),
    )
    .await;
    assert!(matches!(result, Err(ApiError::BadRequest(_))));
}

#[sqlx::test(migrations = "../../migrations")]
async fn remove_workspace_member(pool: PgPool) {
    let owner = create_user(&pool, "owner@t56r.com").await;
    let member = create_user(&pool, "member@t56r.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    add_tenant_member(&pool, tenant, owner, "owner").await;
    add_tenant_member(&pool, tenant, member, "member").await;
    let ws = insert_workspace(&pool, tenant, owner, "eng").await;

    let conn = rls_conn(&pool, tenant).await;
    sqlx::query(
        "INSERT INTO workspace_members (workspace_id, tenant_id, user_id, role)
         VALUES ($1, $2, $3, 'member')",
    )
    .bind(ws)
    .bind(tenant)
    .bind(member)
    .execute(&pool)
    .await
    .unwrap();

    let (status, _) = parts(
        ws_remove_member(
            Path((tenant, ws, member)),
            Extension(TenantContext(tenant)),
            Extension(auth_user(owner)),
            Extension(conn.clone()),
            Extension(test_authz(&pool)),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let mut guard = conn.lock().await;
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(ws)
    .bind(member)
    .fetch_one(&mut *guard)
    .await
    .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn remove_last_privileged_workspace_member_is_rejected(pool: PgPool) {
    let owner = create_user(&pool, "owner@p1remove.com").await;
    let tenant = insert_tenant(&pool, "P1 Remove").await;
    add_tenant_member(&pool, tenant, owner, "member").await;
    let ws = insert_workspace(&pool, tenant, owner, "remove-guard").await;
    add_workspace_member(&pool, tenant, ws, owner, "owner").await;

    let conn = rls_conn(&pool, tenant).await;
    let (status, body) = parts(
        ws_remove_member(
            Path((tenant, ws, owner)),
            Extension(TenantContext(tenant)),
            Extension(auth_user(owner)),
            Extension(conn),
            Extension(test_authz(&pool)),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "bad-request");
}

#[sqlx::test(migrations = "../../migrations")]
async fn remove_one_privileged_member_when_another_remains(pool: PgPool) {
    let owner = create_user(&pool, "owner@p1remove2.com").await;
    let admin = create_user(&pool, "admin@p1remove2.com").await;
    let tenant = insert_tenant(&pool, "P1 Remove 2").await;
    add_tenant_member(&pool, tenant, owner, "member").await;
    add_tenant_member(&pool, tenant, admin, "member").await;
    let ws = insert_workspace(&pool, tenant, owner, "remove-ok").await;
    add_workspace_member(&pool, tenant, ws, owner, "owner").await;
    add_workspace_member(&pool, tenant, ws, admin, "admin").await;

    let conn = rls_conn(&pool, tenant).await;
    let (status, _) = parts(
        ws_remove_member(
            Path((tenant, ws, admin)),
            Extension(TenantContext(tenant)),
            Extension(auth_user(owner)),
            Extension(conn),
            Extension(test_authz(&pool)),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[sqlx::test(migrations = "../../migrations")]
async fn concurrent_privileged_member_removals_leave_one_privileged_member(pool: PgPool) {
    let owner = create_user(&pool, "owner@p15concurrent.com").await;
    let admin = create_user(&pool, "admin@p15concurrent.com").await;
    let tenant = insert_tenant(&pool, "P1.5 Concurrent").await;
    add_tenant_member(&pool, tenant, owner, "member").await;
    add_tenant_member(&pool, tenant, admin, "member").await;
    let ws = insert_workspace(&pool, tenant, owner, "concurrent-remove").await;
    add_workspace_member(&pool, tenant, ws, owner, "owner").await;
    add_workspace_member(&pool, tenant, ws, admin, "admin").await;

    let owner_conn = rls_conn(&pool, tenant).await;
    let admin_conn = rls_conn(&pool, tenant).await;
    let authz = test_authz(&pool);
    let owner_removes_admin =
        remove_member_and_commit(owner_conn, authz.clone(), tenant, ws, admin, owner);
    let admin_removes_owner =
        remove_member_and_commit(admin_conn, authz.clone(), tenant, ws, owner, admin);
    let (status_a, status_b) = tokio::join!(owner_removes_admin, admin_removes_owner);

    let success_count = [status_a, status_b]
        .into_iter()
        .filter(|status| *status == StatusCode::NO_CONTENT)
        .count();
    let rejected_count = [status_a, status_b]
        .into_iter()
        .filter(|status| *status == StatusCode::BAD_REQUEST)
        .count();
    assert_eq!(success_count, 1);
    assert_eq!(rejected_count, 1);

    let privileged_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)
         FROM workspace_members
         WHERE workspace_id = $1 AND role IN ('owner', 'admin')",
    )
    .bind(ws)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(privileged_count, 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn remove_unknown_workspace_member_is_not_found(pool: PgPool) {
    let owner = create_user(&pool, "owner@t56n.com").await;
    let tenant = insert_tenant(&pool, "Acme").await;
    add_tenant_member(&pool, tenant, owner, "owner").await;
    let ws = insert_workspace(&pool, tenant, owner, "eng").await;

    let conn = rls_conn(&pool, tenant).await;
    let result = ws_remove_member(
        Path((tenant, ws, Uuid::new_v4())),
        Extension(TenantContext(tenant)),
        Extension(auth_user(owner)),
        Extension(conn),
        Extension(test_authz(&pool)),
    )
    .await;
    assert!(matches!(result, Err(ApiError::NotFound)));
}

#[sqlx::test(migrations = "../../migrations")]
async fn list_members_is_tenant_isolated(pool: PgPool) {
    // A member added to tenant B's workspace must not be visible from tenant A.
    let user_a = create_user(&pool, "a@t56iso.com").await;
    let user_b = create_user(&pool, "b@t56iso.com").await;
    let tenant_a = insert_tenant(&pool, "A").await;
    let tenant_b = insert_tenant(&pool, "B").await;
    add_tenant_member(&pool, tenant_a, user_a, "owner").await;
    add_tenant_member(&pool, tenant_b, user_b, "owner").await;
    let ws_b = insert_workspace(&pool, tenant_b, user_b, "engb").await;
    sqlx::query(
        "INSERT INTO workspace_members (workspace_id, tenant_id, user_id, role)
         VALUES ($1, $2, $3, 'member')",
    )
    .bind(ws_b)
    .bind(tenant_b)
    .bind(user_b)
    .execute(&pool)
    .await
    .unwrap();

    // Listing under tenant A's RLS context for tenant B's workspace id is hidden.
    let conn = rls_conn(&pool, tenant_a).await;
    let (status, _) = parts(
        ws_list_members(
            Path((tenant_a, ws_b)),
            Extension(TenantContext(tenant_a)),
            Extension(auth_user(user_a)),
            Extension(conn),
            Extension(test_authz(&pool)),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
