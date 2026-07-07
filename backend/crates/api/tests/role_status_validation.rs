//! Phase 0 (TASK-P0-01 + TASK-P0-02) integration tests.
//!
//! Verifies the canonical role + status vocabularies end to end:
//! - valid tenant/workspace roles are accepted by the route handlers;
//! - invalid tenant/workspace roles are rejected with HTTP 400 before any
//!   DB insert (no partial row is written);
//! - the DB CHECK constraints reject invalid roles/statuses directly (the
//!   API validation backstop);
//! - the seed file no longer uses the legacy `documents.status='ready'`
//!   value.
//!
//! These are `#[sqlx::test]` cases: they need a running PostgreSQL instance
//! (migrations are applied automatically to an isolated DB, including the
//! Phase 0 CHECK constraints). When Postgres is unavailable the whole binary
//! is skipped by sqlx's harness — this is an environmental blocker, not a
//! code failure.

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
use gmrag_api::routes::tenant_members::{invite_member, InviteBody};
use gmrag_api::routes::ws_members::{add_member as ws_add_member, AddMemberBody};

#[path = "support/authz.rs"]
mod authz_support;
use authz_support::test_authz;

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

// ─── TASK-P0-01: route-level role validation ─────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn invite_member_accepts_canonical_tenant_roles(pool: PgPool) {
    let owner = create_user(&pool, "owner@p0role.com").await;
    let tenant = insert_tenant(&pool, "P0Roles").await;
    add_tenant_member(&pool, tenant, owner, "owner").await;

    for role in ["owner", "admin", "member"] {
        let conn = rls_conn(&pool, tenant).await;
        let (status, body) = parts(
            invite_member(
                Path(tenant),
                Extension(TenantContext(tenant)),
                Extension(auth_user(owner)),
                Extension(conn),
                Extension(test_authz(&pool)),
                Json(InviteBody {
                    email: format!("{role}@p0role.com"),
                    role: Some(role.into()),
                }),
            )
            .await,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "role {role} should be accepted"
        );
        assert_eq!(body["role"], role);
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn invite_member_rejects_invalid_tenant_role_with_400(pool: PgPool) {
    let owner = create_user(&pool, "owner@p0rolebad.com").await;
    let tenant = insert_tenant(&pool, "P0RolesBad").await;
    add_tenant_member(&pool, tenant, owner, "owner").await;

    for bad in ["viewer", "editor", "OWNER", "root"] {
        let conn = rls_conn(&pool, tenant).await;
        let (status, body) = parts(
            invite_member(
                Path(tenant),
                Extension(TenantContext(tenant)),
                Extension(auth_user(owner)),
                Extension(conn.clone()),
                Extension(test_authz(&pool)),
                Json(InviteBody {
                    email: format!("x-{bad}@p0rolebad.com"),
                    role: Some(bad.into()),
                }),
            )
            .await,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "tenant role '{bad}' must be rejected with 400"
        );
        assert_eq!(body["error"]["code"], "bad-request");
    }

    // No invitation rows should have been written for invalid roles.
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM invitations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0, "invalid role must not create an invitation row");
}

#[sqlx::test(migrations = "../../migrations")]
async fn invite_member_empty_role_defaults_to_member(pool: PgPool) {
    let owner = create_user(&pool, "owner@p0empty.com").await;
    let tenant = insert_tenant(&pool, "P0Empty").await;
    add_tenant_member(&pool, tenant, owner, "owner").await;

    // An empty / whitespace role string is treated as the default `member`,
    // not as an invalid role — mirroring the prior behaviour and the
    // `Option<String>` semantics (None and "" both mean "use the default").
    for empty in ["", "   "] {
        let conn = rls_conn(&pool, tenant).await;
        let (status, body) = parts(
            invite_member(
                Path(tenant),
                Extension(TenantContext(tenant)),
                Extension(auth_user(owner)),
                Extension(conn),
                Extension(test_authz(&pool)),
                Json(InviteBody {
                    email: format!("e-{empty:x?}@p0empty.com"),
                    role: Some(empty.into()),
                }),
            )
            .await,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "empty role should default to member"
        );
        assert_eq!(body["role"], "member");
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn add_workspace_member_accepts_canonical_roles(pool: PgPool) {
    let owner = create_user(&pool, "owner@p0ws.com").await;
    let member = create_user(&pool, "member@p0ws.com").await;
    let tenant = insert_tenant(&pool, "P0Ws").await;
    add_tenant_member(&pool, tenant, owner, "owner").await;
    add_tenant_member(&pool, tenant, member, "member").await;
    let ws = insert_workspace(&pool, tenant, owner, "eng").await;

    // A canonical 'admin' role is accepted (201). The other canonical roles
    // ('owner', 'member') share the same validation path; the DB CHECK tests
    // below cover all three at the storage layer.
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
                role: Some("admin".into()),
            }),
        )
        .await,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "canonical role 'admin' should be accepted"
    );
    assert_eq!(body["role"], "admin");
}

#[sqlx::test(migrations = "../../migrations")]
async fn add_workspace_member_rejects_invalid_role_with_400(pool: PgPool) {
    let owner = create_user(&pool, "owner@p0wsbad.com").await;
    let member = create_user(&pool, "member@p0wsbad.com").await;
    let tenant = insert_tenant(&pool, "P0WsBad").await;
    add_tenant_member(&pool, tenant, owner, "owner").await;
    add_tenant_member(&pool, tenant, member, "member").await;
    let ws = insert_workspace(&pool, tenant, owner, "eng").await;

    for bad in ["viewer", "editor", "Owner", "root"] {
        let conn = rls_conn(&pool, tenant).await;
        let (status, body) = parts(
            ws_add_member(
                Path((tenant, ws)),
                Extension(TenantContext(tenant)),
                Extension(auth_user(owner)),
                Extension(conn.clone()),
                Extension(test_authz(&pool)),
                Json(AddMemberBody {
                    user_id: member,
                    role: Some(bad.into()),
                }),
            )
            .await,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "workspace role '{bad}' must be rejected with 400"
        );
        assert_eq!(body["error"]["code"], "bad-request");
    }

    let n: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM workspace_members WHERE workspace_id = $1")
            .bind(ws)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(n, 0, "invalid role must not create a workspace_members row");
}

// ─── TASK-P0-01: DB CHECK backstop ───────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn db_check_rejects_invalid_tenant_member_role(pool: PgPool) {
    let user = create_user(&pool, "u@p0chk.com").await;
    let tenant = insert_tenant(&pool, "P0Chk").await;

    // Superuser write bypasses RLS but NOT the CHECK constraint.
    let res = sqlx::query(
        "INSERT INTO tenant_members (tenant_id, user_id, role) VALUES ($1, $2, 'overlord')",
    )
    .bind(tenant)
    .bind(user)
    .execute(&pool)
    .await;
    assert!(res.is_err(), "tenant_members CHECK must reject 'overlord'");

    // Canonical roles succeed.
    for role in ["owner", "admin", "member"] {
        let other = create_user(&pool, &format!("ok-{role}@p0chk.com")).await;
        sqlx::query("INSERT INTO tenant_members (tenant_id, user_id, role) VALUES ($1, $2, $3)")
            .bind(tenant)
            .bind(other)
            .bind(role)
            .execute(&pool)
            .await
            .expect("canonical role must insert");
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn db_check_rejects_invalid_workspace_member_role(pool: PgPool) {
    let owner = create_user(&pool, "wso@p0chk.com").await;
    let member = create_user(&pool, "wsm@p0chk.com").await;
    let tenant = insert_tenant(&pool, "P0WsChk").await;
    add_tenant_member(&pool, tenant, owner, "owner").await;
    let ws = insert_workspace(&pool, tenant, owner, "eng").await;

    let res = sqlx::query(
        "INSERT INTO workspace_members (workspace_id, tenant_id, user_id, role)
         VALUES ($1, $2, $3, 'editor')",
    )
    .bind(ws)
    .bind(tenant)
    .bind(member)
    .execute(&pool)
    .await;
    assert!(res.is_err(), "workspace_members CHECK must reject 'editor'");

    // Canonical 'member' succeeds.
    sqlx::query(
        "INSERT INTO workspace_members (workspace_id, tenant_id, user_id, role)
         VALUES ($1, $2, $3, 'member')",
    )
    .bind(ws)
    .bind(tenant)
    .bind(member)
    .execute(&pool)
    .await
    .expect("canonical 'member' must insert");
}

// ─── TASK-P0-02: DB CHECK backstop for statuses ──────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn db_check_rejects_invalid_document_status(pool: PgPool) {
    let owner = create_user(&pool, "doc@p0chk.com").await;
    let tenant = insert_tenant(&pool, "P0DocChk").await;
    add_tenant_member(&pool, tenant, owner, "owner").await;
    let ws = insert_workspace(&pool, tenant, owner, "eng").await;

    let res = sqlx::query(
        "INSERT INTO documents (tenant_id, workspace_id, owner_id, title, status)
         VALUES ($1, $2, $3, 'bad', 'ready')",
    )
    .bind(tenant)
    .bind(ws)
    .bind(owner)
    .execute(&pool)
    .await;
    assert!(res.is_err(), "documents CHECK must reject legacy 'ready'");

    for status in ["uploaded", "processing", "indexed", "failed"] {
        sqlx::query(
            "INSERT INTO documents (tenant_id, workspace_id, owner_id, title, status)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(tenant)
        .bind(ws)
        .bind(owner)
        .bind(format!("doc-{status}"))
        .bind(status)
        .execute(&pool)
        .await
        .expect("canonical document status must insert");
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn db_check_rejects_invalid_ingest_outbox_status(pool: PgPool) {
    let owner = create_user(&pool, "out@p0chk.com").await;
    let tenant = insert_tenant(&pool, "P0OutChk").await;
    add_tenant_member(&pool, tenant, owner, "owner").await;
    let ws = insert_workspace(&pool, tenant, owner, "eng").await;
    let doc: Uuid = sqlx::query_scalar(
        "INSERT INTO documents (tenant_id, workspace_id, owner_id, title, status)
         VALUES ($1, $2, $3, 'd', 'indexed') RETURNING id",
    )
    .bind(tenant)
    .bind(ws)
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();

    let res = sqlx::query(
        "INSERT INTO ingest_outbox (tenant_id, document_id, payload, status)
         VALUES ($1, $2, '{}'::jsonb, 'sent')",
    )
    .bind(tenant)
    .bind(doc)
    .execute(&pool)
    .await;
    assert!(res.is_err(), "ingest_outbox CHECK must reject 'sent'");

    for status in ["pending", "dispatched"] {
        sqlx::query(
            "INSERT INTO ingest_outbox (tenant_id, document_id, payload, status)
             VALUES ($1, $2, '{}'::jsonb, $3)",
        )
        .bind(tenant)
        .bind(doc)
        .bind(status)
        .execute(&pool)
        .await
        .expect("canonical outbox status must insert");
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn db_check_rejects_invalid_invitation_status(pool: PgPool) {
    let owner = create_user(&pool, "inv@p0chk.com").await;
    let tenant = insert_tenant(&pool, "P0InvChk").await;
    add_tenant_member(&pool, tenant, owner, "owner").await;

    let res = sqlx::query(
        "INSERT INTO invitations (tenant_id, email, role, invited_by, status)
         VALUES ($1, 'x@y.com', 'member', $2, 'done')",
    )
    .bind(tenant)
    .bind(owner)
    .execute(&pool)
    .await;
    assert!(res.is_err(), "invitations CHECK must reject 'done'");

    for status in ["pending", "accepted", "expired", "revoked"] {
        sqlx::query(
            "INSERT INTO invitations (tenant_id, email, role, invited_by, status)
             VALUES ($1, $2, 'member', $3, $4)",
        )
        .bind(tenant)
        .bind(format!("{status}@p0chk.com"))
        .bind(owner)
        .bind(status)
        .execute(&pool)
        .await
        .expect("canonical invitation status must insert");
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn db_check_rejects_invalid_chat_message_role(pool: PgPool) {
    let owner = create_user(&pool, "chat@p0chk.com").await;
    let tenant = insert_tenant(&pool, "P0ChatChk").await;
    add_tenant_member(&pool, tenant, owner, "owner").await;
    let sid: Uuid = sqlx::query_scalar(
        "INSERT INTO chat_sessions (tenant_id, user_id, title) VALUES ($1, $2, 's') RETURNING id",
    )
    .bind(tenant)
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();

    let res = sqlx::query(
        "INSERT INTO chat_messages (tenant_id, session_id, role, content)
         VALUES ($1, $2, 'tool', 'hi')",
    )
    .bind(tenant)
    .bind(sid)
    .execute(&pool)
    .await;
    assert!(res.is_err(), "chat_messages CHECK must reject 'tool'");

    for role in ["user", "assistant", "system"] {
        sqlx::query(
            "INSERT INTO chat_messages (tenant_id, session_id, role, content)
             VALUES ($1, $2, $3, 'hi')",
        )
        .bind(tenant)
        .bind(sid)
        .bind(role)
        .execute(&pool)
        .await
        .expect("canonical chat role must insert");
    }
}

// ─── TASK-P0-02: seed no longer uses 'ready' ─────────────────────────────────

#[test]
fn seed_file_does_not_use_ready_document_status() {
    let seed_sql = include_str!("../../../../infra/postgres/seed.sql");
    assert!(
        !seed_sql.contains("'ready'"),
        "seed.sql must not use the legacy documents.status='ready' value"
    );
    assert!(
        seed_sql.contains("'indexed'"),
        "seed.sql should use the canonical 'indexed' document status"
    );
}
