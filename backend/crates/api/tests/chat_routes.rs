//! Integration tests for chat routes (T61/T62).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::extract::{Extension, Path};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use gmrag_api::auth::extractor::AuthUser;
use gmrag_api::auth::jwt::JwtClaims;
use gmrag_api::auth::tenant::TenantContext;
use gmrag_api::chat::{ChunkHit, GraphContext};
use gmrag_api::error::ApiError;
use gmrag_api::llm::provider::{
    ChatMessage, ChatStream, ChatStreamFuture, GraphExtraction, LlmProvider, ProviderFuture,
};
use gmrag_api::middleware::rls::SharedConnection;
use gmrag_api::pool::AppPool;
use gmrag_api::routes::chat::{
    authorize_chat_session, create_session, delete_session, list_sessions,
    post_chat_sse_with_context, CreateSessionBody,
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
    sqlx::query(
        "INSERT INTO tenant_members (tenant_id, user_id, role) VALUES ($1, $2, $3)
         ON CONFLICT DO NOTHING",
    )
    .bind(tenant_id)
    .bind(user_id)
    .bind(role)
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_chat_session(
    pool: &PgPool,
    tenant_id: Uuid,
    user_id: Uuid,
    workspace_id: Option<Uuid>,
    title: &str,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO chat_sessions (id, tenant_id, user_id, workspace_id, title)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(tenant_id)
    .bind(user_id)
    .bind(workspace_id)
    .bind(title)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn insert_workspace(
    pool: &PgPool,
    tenant_id: Uuid,
    created_by: Uuid,
    name: &str,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workspaces (id, tenant_id, name, slug, created_by) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(tenant_id)
    .bind(name)
    .bind(format!("ws-{id}"))
    .bind(created_by)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn add_workspace_member(pool: &PgPool, ws_id: Uuid, tenant_id: Uuid, user_id: Uuid) {
    sqlx::query(
        "INSERT INTO workspace_members (workspace_id, tenant_id, user_id) VALUES ($1, $2, $3)",
    )
    .bind(ws_id)
    .bind(tenant_id)
    .bind(user_id)
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_grant(
    pool: &PgPool,
    tenant_id: Uuid,
    session_id: Uuid,
    principal_type: &str,
    principal_id: Uuid,
    permission: &str,
) {
    sqlx::query(
        "INSERT INTO resource_acl (tenant_id, resource_type, resource_id, principal_type, principal_id, permission)
         VALUES ($1, 'chat_session', $2, $3, $4, $5)",
    )
    .bind(tenant_id)
    .bind(session_id)
    .bind(principal_type)
    .bind(principal_id)
    .bind(permission)
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

fn parse_sse_json_chunks(body: &str) -> Vec<Value> {
    body.lines()
        .filter_map(|line| line.strip_prefix("data:").map(str::trim))
        .filter(|payload| !payload.is_empty())
        .map(|payload| serde_json::from_str(payload).expect("valid sse json"))
        .collect()
}

struct StaticChatProvider {
    body: &'static str,
    calls: AtomicUsize,
}

impl LlmProvider for StaticChatProvider {
    fn embed_query<'a>(&'a self, _query: &'a str) -> ProviderFuture<'a, Vec<f32>> {
        Box::pin(async { Ok(vec![0.1; 768]) })
    }

    fn chat_stream<'a>(&'a self, _messages: &'a [ChatMessage]) -> ChatStreamFuture<'a> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let body = self.body;
        Box::pin(async move {
            let stream: ChatStream = Box::pin(futures::stream::iter(body.lines().map(|line| {
                Ok(parse_test_sse_line(line))
            })));
            Ok(stream)
        })
    }

    fn graph_extract<'a>(&'a self, _text: &'a str) -> ProviderFuture<'a, GraphExtraction> {
        Box::pin(async { Ok(GraphExtraction::default()) })
    }

    fn provider(&self) -> &str {
        "static-mock"
    }

    fn chat_model(&self) -> &str {
        "mock-model"
    }
}

fn parse_test_sse_line(line: &str) -> gmrag_api::llm::provider::ChatDelta {
    use gmrag_api::llm::provider::ChatDelta;
    let payload = line.strip_prefix("data:").unwrap_or(line).trim();
    if payload == "[DONE]" {
        return ChatDelta {
            content: String::new(),
            finish_reason: Some("stop".into()),
        };
    }
    let v: Value = serde_json::from_str(payload).unwrap();
    ChatDelta {
        content: v["choices"][0]["delta"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string(),
        finish_reason: v["choices"][0]["finish_reason"]
            .as_str()
            .map(str::to_string),
    }
}

fn sample_chunk() -> ChunkHit {
    ChunkHit {
        citation_index: 1,
        point_id: Uuid::new_v4(),
        document_id: Uuid::new_v4(),
        chunk_index: 0,
        content: "excerpt".into(),
        filename: Some("doc.pdf".into()),
        score: 1.0,
        page_start: None,
        page_end: None,
    }
}

async fn collect_sse_body(sse: impl IntoResponse) -> (StatusCode, String, String) {
    let resp = sse.into_response();
    let status = resp.status();
    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, content_type, String::from_utf8(bytes.to_vec()).unwrap())
}

// ─── T61: SSE chat ───────────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn post_chat_streams_sse_events(pool: PgPool) {
    let tenant = insert_tenant(&pool, "chat-sse-tenant").await;
    let owner = create_user(&pool, "owner-sse@test.com").await;
    add_tenant_member(&pool, tenant, owner, "member").await;
    let sid = insert_chat_session(&pool, tenant, owner, None, "SSE test").await;

    let provider = Arc::new(StaticChatProvider {
        body: concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"See [chunk:1]\"},\"finish_reason\":null}]}\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\" answer\"},\"finish_reason\":null}]}\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n",
        ),
        calls: AtomicUsize::new(0),
    });
    let llm: Arc<dyn LlmProvider> = Arc::clone(&provider) as Arc<dyn LlmProvider>;

    let conn = rls_conn(&pool, tenant).await;
    let sse = post_chat_sse_with_context(
        tenant,
        sid,
        None,
        "question?".into(),
        conn,
        AppPool(pool.clone()),
        llm,
        vec![sample_chunk()],
        GraphContext::default(),
    )
    .await
    .expect("sse handler");

    let (status, content_type, body) = collect_sse_body(sse).await;
    assert_eq!(status, StatusCode::OK);
    assert!(content_type.starts_with("text/event-stream"));

    let events = parse_sse_json_chunks(&body);
    assert!(events.iter().any(|e| e["type"] == "text"));
    assert!(events.iter().any(|e| e["type"] == "citation" && e["index"] == 1));
    assert!(events.iter().any(|e| e["type"] == "done"));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn post_chat_denied_without_viewer(pool: PgPool) {
    let tenant = insert_tenant(&pool, "chat-deny-tenant").await;
    let owner = create_user(&pool, "owner-deny@test.com").await;
    let stranger = create_user(&pool, "stranger-deny@test.com").await;
    add_tenant_member(&pool, tenant, owner, "member").await;
    add_tenant_member(&pool, tenant, stranger, "member").await;
    let sid = insert_chat_session(&pool, tenant, owner, None, "private").await;

    let conn = rls_conn(&pool, tenant).await;
    let mut guard = conn.lock().await;
    let err = authorize_chat_session(&mut guard, sid, stranger).await;
    assert!(matches!(err, Err(ApiError::NotFound)));
}

#[sqlx::test(migrations = "../../migrations")]
async fn post_chat_cross_tenant_returns_not_found(pool: PgPool) {
    let tenant_a = insert_tenant(&pool, "chat-tenant-a").await;
    let tenant_b = insert_tenant(&pool, "chat-tenant-b").await;
    let owner = create_user(&pool, "owner-xtenant@test.com").await;
    add_tenant_member(&pool, tenant_b, owner, "member").await;
    let sid = insert_chat_session(&pool, tenant_b, owner, None, "other tenant").await;

    let conn = rls_conn(&pool, tenant_a).await;
    let mut guard = conn.lock().await;
    let err = authorize_chat_session(&mut guard, sid, owner).await;
    assert!(matches!(err, Err(ApiError::NotFound)));
}

// ─── T62: sessions CRUD ─────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn list_sessions_returns_owned_and_shared(pool: PgPool) {
    let tenant = insert_tenant(&pool, "list-sessions-tenant").await;
    let owner = create_user(&pool, "list-owner@test.com").await;
    let viewer = create_user(&pool, "list-viewer@test.com").await;
    add_tenant_member(&pool, tenant, owner, "member").await;
    add_tenant_member(&pool, tenant, viewer, "member").await;

    let owned = insert_chat_session(&pool, tenant, owner, None, "mine").await;
    let shared = insert_chat_session(&pool, tenant, owner, None, "shared").await;
    insert_grant(&pool, tenant, shared, "user", viewer, "viewer").await;
    insert_chat_session(&pool, tenant, owner, None, "hidden").await;

    let conn = rls_conn(&pool, tenant).await;
    let resp = list_sessions(
        Path(tenant),
        Extension(TenantContext(tenant)),
        Extension(auth_user(viewer)),
        Extension(conn),
    )
    .await
    .unwrap()
    .into_response();

    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    let ids: Vec<Uuid> = body["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| Uuid::parse_str(s["id"].as_str().unwrap()).unwrap())
        .collect();

    assert!(ids.contains(&shared));
    assert!(!ids.contains(&owned));
}

/// C14 regression: a chat_session in a workspace where the caller is a member
/// (but has no explicit `resource_acl` grant and is not the owner) must be
/// visible in `list_sessions` via workspace inheritance — mirroring
/// `check_relation(chat_session, viewer, user)` rewrite
/// `tuple_to_userset(workspace → member)`.
#[sqlx::test(migrations = "../../migrations")]
async fn list_sessions_includes_workspace_member_inheritance(pool: PgPool) {
    let tenant = insert_tenant(&pool, "list-ws-inheritance").await;
    let owner = create_user(&pool, "ws-owner@test.com").await;
    let member = create_user(&pool, "ws-member@test.com").await;
    add_tenant_member(&pool, tenant, owner, "member").await;
    add_tenant_member(&pool, tenant, member, "member").await;

    let ws = insert_workspace(&pool, tenant, owner, "Shared WS").await;
    add_workspace_member(&pool, ws, tenant, owner).await;
    add_workspace_member(&pool, ws, tenant, member).await;

    // Owner creates a session in the workspace. `member` has no explicit
    // grant and is not the owner — visibility comes only from workspace
    // membership inheritance.
    let ws_session = insert_chat_session(&pool, tenant, owner, Some(ws), "ws session").await;

    // A session with no workspace_id must NOT leak via workspace inheritance.
    let private_session = insert_chat_session(&pool, tenant, owner, None, "private").await;

    let conn = rls_conn(&pool, tenant).await;
    let resp = list_sessions(
        Path(tenant),
        Extension(TenantContext(tenant)),
        Extension(auth_user(member)),
        Extension(conn),
    )
    .await
    .unwrap()
    .into_response();

    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    let ids: Vec<Uuid> = body["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| Uuid::parse_str(s["id"].as_str().unwrap()).unwrap())
        .collect();

    assert!(
        ids.contains(&ws_session),
        "workspace member must see workspace-scoped session via inheritance"
    );
    assert!(
        !ids.contains(&private_session),
        "non-owner must not see private session without grant"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn create_session_sets_owner_to_caller(pool: PgPool) {
    let tenant = insert_tenant(&pool, "create-session-tenant").await;
    let user = create_user(&pool, "create-user@test.com").await;
    add_tenant_member(&pool, tenant, user, "member").await;

    let conn = rls_conn(&pool, tenant).await;
    let resp = create_session(
        Path(tenant),
        Extension(TenantContext(tenant)),
        Extension(auth_user(user)),
        Extension(conn.clone()),
        Json(CreateSessionBody {
            title: Some("New chat".into()),
            workspace_id: None,
            model: None,
        }),
    )
    .await
    .unwrap()
    .into_response();

    assert_eq!(resp.status(), StatusCode::CREATED);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    let sid = Uuid::parse_str(body["id"].as_str().unwrap()).unwrap();

    let mut guard = conn.lock().await;
    let owner: Uuid = sqlx::query_scalar("SELECT user_id FROM chat_sessions WHERE id = $1")
        .bind(sid)
        .fetch_one(&mut *guard)
        .await
        .unwrap();
    assert_eq!(owner, user);
}

#[sqlx::test(migrations = "../../migrations")]
async fn delete_session_requires_owner(pool: PgPool) {
    let tenant = insert_tenant(&pool, "delete-session-tenant").await;
    let owner = create_user(&pool, "del-owner@test.com").await;
    let viewer = create_user(&pool, "del-viewer@test.com").await;
    add_tenant_member(&pool, tenant, owner, "member").await;
    add_tenant_member(&pool, tenant, viewer, "member").await;

    let sid = insert_chat_session(&pool, tenant, owner, None, "to delete").await;
    insert_grant(&pool, tenant, sid, "user", viewer, "viewer").await;

    let conn = rls_conn(&pool, tenant).await;
    let forbidden = delete_session(
        Path((tenant, sid)),
        Extension(TenantContext(tenant)),
        Extension(auth_user(viewer)),
        Extension(conn.clone()),
    )
    .await;
    assert!(matches!(forbidden, Err(ApiError::Forbidden(_))));

    let ok = delete_session(
        Path((tenant, sid)),
        Extension(TenantContext(tenant)),
        Extension(auth_user(owner)),
        Extension(conn),
    )
    .await
    .unwrap()
    .into_response();
    assert_eq!(ok.status(), StatusCode::NO_CONTENT);
}

#[sqlx::test(migrations = "../../migrations")]
async fn delete_session_cross_tenant(pool: PgPool) {
    let tenant_a = insert_tenant(&pool, "del-tenant-a").await;
    let tenant_b = insert_tenant(&pool, "del-tenant-b").await;
    let owner = create_user(&pool, "del-xtenant@test.com").await;
    add_tenant_member(&pool, tenant_b, owner, "member").await;
    let sid = insert_chat_session(&pool, tenant_b, owner, None, "other").await;

    let conn = rls_conn(&pool, tenant_a).await;
    let err = delete_session(
        Path((tenant_a, sid)),
        Extension(TenantContext(tenant_a)),
        Extension(auth_user(owner)),
        Extension(conn),
    )
    .await;
    assert!(matches!(err, Err(ApiError::NotFound)));
}
