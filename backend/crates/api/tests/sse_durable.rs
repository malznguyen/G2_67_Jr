//! Phase 0 (TASK-P0-03) — canonical SSE durable-before-done tests.
//!
//! Verifies the SSE terminal contract:
//! - success: text/citation... then exactly one `done` (after persistence);
//! - persistence failure: `persist-failed` and NO `done`;
//! - upstream stream failure: `stream-failed`, NO `done`, and the exact
//!   just-created user message is removed (compensation) while prior
//!   messages are preserved;
//! - `finish_reason` is preserved in the `done` event;
//! - exactly one terminal application event is emitted.
//!
//! Uses a fake `LlmProvider` (no live LLM). `#[sqlx::test]` cases need a
//! running PostgreSQL instance; when it is unavailable the binary is
//! skipped by the sqlx harness (environmental blocker, not a code failure).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::response::IntoResponse;
use sqlx::PgPool;
use uuid::Uuid;

use gmrag_api::chat::{ChunkHit, GraphContext};
use gmrag_api::llm::provider::{
    ChatDelta, ChatMessage, ChatStream, ChatStreamFuture, GraphExtraction, LlmError, LlmProvider,
    ProviderFuture,
};
use gmrag_api::middleware::rls::SharedConnection;
use gmrag_api::pool::AppPool;
use gmrag_api::routes::chat::post_chat_sse_with_context;

// ─── helpers ─────────────────────────────────────────────────────────────────

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

async fn insert_chat_session(pool: &PgPool, tenant_id: Uuid, user_id: Uuid, title: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO chat_sessions (id, tenant_id, user_id, title)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(id)
    .bind(tenant_id)
    .bind(user_id)
    .bind(title)
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

async fn collect_sse_body(sse: impl IntoResponse) -> String {
    let resp = sse.into_response();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn parse_sse_json_chunks(body: &str) -> Vec<serde_json::Value> {
    body.lines()
        .filter_map(|line| line.strip_prefix("data:").map(str::trim))
        .filter(|payload| !payload.is_empty())
        .map(|payload| serde_json::from_str(payload).expect("valid sse json"))
        .collect()
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

fn delta(content: &str, finish: Option<&str>) -> ChatDelta {
    ChatDelta {
        content: content.into(),
        finish_reason: finish.map(str::to_string),
    }
}

/// Provider that streams a fixed sequence of deltas then completes.
struct OkProvider {
    body: &'static str,
    calls: AtomicUsize,
}

impl LlmProvider for OkProvider {
    fn embed_query<'a>(&'a self, _q: &'a str) -> ProviderFuture<'a, Vec<f32>> {
        Box::pin(async { Ok(vec![0.1; 768]) })
    }
    fn chat_stream<'a>(&'a self, _m: &'a [ChatMessage]) -> ChatStreamFuture<'a> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let body = self.body;
        Box::pin(async move {
            let stream: ChatStream = Box::pin(futures::stream::iter(
                body.lines().map(|l| Ok(parse_ok_line(l))),
            ));
            Ok(stream)
        })
    }
    fn graph_extract<'a>(&'a self, _t: &'a str) -> ProviderFuture<'a, GraphExtraction> {
        Box::pin(async { Ok(GraphExtraction::default()) })
    }
    fn provider(&self) -> &str {
        "ok-mock"
    }
    fn chat_model(&self) -> &str {
        "mock-model"
    }
}

fn ok_body() -> &'static str {
    concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"See [chunk:1]\"},\"finish_reason\":null}]}\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\" answer\"},\"finish_reason\":null}]}\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n",
    )
}

/// Build an OkProvider whose stream is pre-parsed from `ok_body()` lines.
fn ok_provider() -> OkProvider {
    OkProvider {
        body: ok_body(),
        calls: AtomicUsize::new(0),
    }
}

/// A streaming provider implemented from a Vec<ChatDelta> so the mid-stream
/// failure path can inject an error after a partial delta.
///
/// `LlmError` is not `Clone`, so the deltas are held in an `Option<Vec>` and
/// taken once when the stream is built (test providers are single-use).
struct VecProvider {
    deltas: std::sync::Mutex<Option<Vec<Result<ChatDelta, LlmError>>>>,
}

impl LlmProvider for VecProvider {
    fn embed_query<'a>(&'a self, _q: &'a str) -> ProviderFuture<'a, Vec<f32>> {
        Box::pin(async { Ok(vec![0.1; 768]) })
    }
    fn chat_stream<'a>(&'a self, _m: &'a [ChatMessage]) -> ChatStreamFuture<'a> {
        let taken = self.deltas.lock().unwrap().take();
        Box::pin(async move {
            let stream: ChatStream = match taken {
                Some(deltas) => Box::pin(futures::stream::iter(deltas)),
                None => Box::pin(futures::stream::empty()),
            };
            Ok(stream)
        })
    }
    fn graph_extract<'a>(&'a self, _t: &'a str) -> ProviderFuture<'a, GraphExtraction> {
        Box::pin(async { Ok(GraphExtraction::default()) })
    }
    fn provider(&self) -> &str {
        "vec-mock"
    }
    fn chat_model(&self) -> &str {
        "mock-model"
    }
}

fn vec_provider(deltas: Vec<Result<ChatDelta, LlmError>>) -> VecProvider {
    VecProvider {
        deltas: std::sync::Mutex::new(Some(deltas)),
    }
}

fn terminal_event_types(events: &[serde_json::Value]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|e| {
            let t = e["type"].as_str()?;
            match t {
                "done" | "error" => Some(t),
                _ => None,
            }
        })
        .collect()
}

// Silence unused-import warnings for helpers wired only for completeness.
#[allow(dead_code)]
fn _touch(_e: impl IntoResponse) {}

// ─── success path ────────────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn sse_success_emits_done_after_persistence(pool: PgPool) {
    let tenant = insert_tenant(&pool, "sse-ok").await;
    let owner = create_user(&pool, "ok@sse.com").await;
    add_tenant_member(&pool, tenant, owner, "member").await;
    let sid = insert_chat_session(&pool, tenant, owner, "ok").await;

    let provider = ok_provider();
    let llm: Arc<dyn LlmProvider> = Arc::new(provider);

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

    let body = collect_sse_body(sse).await;
    let events = parse_sse_json_chunks(&body);

    // text + citation streamed before done.
    assert!(events.iter().any(|e| e["type"] == "text"));
    assert!(events
        .iter()
        .any(|e| e["type"] == "citation" && e["index"] == 1));

    let terminals = terminal_event_types(&events);
    assert_eq!(terminals, vec!["done"], "exactly one terminal event: done");

    // done is the LAST application event.
    assert_eq!(events.last().unwrap()["type"], "done");
    // finish_reason preserved.
    assert_eq!(events.last().unwrap()["finish_reason"], "stop");

    // Persistence occurred: an assistant message row exists for this session.
    let assistant_n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM chat_messages WHERE session_id = $1 AND role = 'assistant'",
    )
    .bind(sid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(assistant_n, 1, "assistant message must be persisted");
}

#[sqlx::test(migrations = "../../migrations")]
async fn sse_done_not_emitted_before_persistence(pool: PgPool) {
    // The done event must come after the assistant row is durable. We verify
    // by checking that when the body is fully collected (done emitted), the
    // assistant row is already present — i.e. done is not emitted speculatively.
    let tenant = insert_tenant(&pool, "sse-order").await;
    let owner = create_user(&pool, "order@sse.com").await;
    add_tenant_member(&pool, tenant, owner, "member").await;
    let sid = insert_chat_session(&pool, tenant, owner, "order").await;

    let llm: Arc<dyn LlmProvider> = Arc::new(ok_provider());
    let conn = rls_conn(&pool, tenant).await;
    let sse = post_chat_sse_with_context(
        tenant,
        sid,
        None,
        "q".into(),
        conn,
        AppPool(pool.clone()),
        llm,
        vec![sample_chunk()],
        GraphContext::default(),
    )
    .await
    .unwrap();
    let body = collect_sse_body(sse).await;
    let events = parse_sse_json_chunks(&body);
    assert!(events.iter().any(|e| e["type"] == "done"));

    let assistant_n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM chat_messages WHERE session_id = $1 AND role = 'assistant'",
    )
    .bind(sid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(assistant_n, 1);
}

// ─── persistence failure ─────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn sse_persistence_failure_emits_persist_failed_no_done(pool: PgPool) {
    let tenant = insert_tenant(&pool, "sse-persistfail").await;
    let owner = create_user(&pool, "pf@sse.com").await;
    add_tenant_member(&pool, tenant, owner, "member").await;
    let sid = insert_chat_session(&pool, tenant, owner, "pf").await;

    let llm: Arc<dyn LlmProvider> = Arc::new(ok_provider());

    // Build the request-side SharedConnection (acquire + detach a
    // connection) BEFORE closing the pool so the user-message insert can
    // still run. The detached connection stays usable after the pool is
    // closed.
    let conn = rls_conn(&pool, tenant).await;

    // Now close the pool so persist_chat_completion's pool.acquire() fails
    // → persist-failed, no done. The AppPool wraps the same (now closed)
    // pool handle.
    let closed_pool = pool.clone();
    closed_pool.close().await;

    let sse = post_chat_sse_with_context(
        tenant,
        sid,
        None,
        "q".into(),
        conn,
        AppPool(closed_pool),
        llm,
        vec![sample_chunk()],
        GraphContext::default(),
    )
    .await
    .expect("sse handler still builds");

    let body = collect_sse_body(sse).await;
    let events = parse_sse_json_chunks(&body);

    // text/citation still streamed.
    assert!(events.iter().any(|e| e["type"] == "text"));
    // No done; exactly one terminal error event with persist-failed.
    assert!(
        !events.iter().any(|e| e["type"] == "done"),
        "done must NOT be emitted on persistence failure"
    );
    let errors: Vec<_> = events.iter().filter(|e| e["type"] == "error").collect();
    assert_eq!(errors.len(), 1, "exactly one terminal error event");
    assert_eq!(errors[0]["code"], "persist-failed");
    assert_eq!(events.last().unwrap()["type"], "error");
}

// ─── upstream stream failure + compensation ──────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn sse_stream_failure_emits_stream_failed_and_compensates_user_message(pool: PgPool) {
    let tenant = insert_tenant(&pool, "sse-streamfail").await;
    let owner = create_user(&pool, "sf@sse.com").await;
    add_tenant_member(&pool, tenant, owner, "member").await;
    let sid = insert_chat_session(&pool, tenant, owner, "sf").await;

    // Pre-existing prior message that compensation must NOT touch.
    sqlx::query(
        "INSERT INTO chat_messages (tenant_id, session_id, role, content)
         VALUES ($1, $2, 'user', 'prior turn')",
    )
    .bind(tenant)
    .bind(sid)
    .execute(&pool)
    .await
    .unwrap();
    let prior_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM chat_messages WHERE session_id = $1")
            .bind(sid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(prior_count, 1);

    let llm: Arc<dyn LlmProvider> = Arc::new(vec_provider(vec![
        Ok(delta("partial ", None)),
        Err(LlmError::Parse("upstream exploded".into())),
    ]));

    let conn = rls_conn(&pool, tenant).await;
    let sse = post_chat_sse_with_context(
        tenant,
        sid,
        None,
        "this turn fails".into(),
        conn,
        AppPool(pool.clone()),
        llm,
        vec![sample_chunk()],
        GraphContext::default(),
    )
    .await
    .unwrap();

    let body = collect_sse_body(sse).await;
    let events = parse_sse_json_chunks(&body);

    // Partial text was streamed before the failure.
    assert!(events.iter().any(|e| e["type"] == "text"));
    assert!(
        !events.iter().any(|e| e["type"] == "done"),
        "done must NOT be emitted on stream failure"
    );
    let errors: Vec<_> = events.iter().filter(|e| e["type"] == "error").collect();
    assert_eq!(errors.len(), 1, "exactly one terminal error event");
    assert_eq!(errors[0]["code"], "stream-failed");

    // Compensation: the just-created user message is gone, the prior turn
    // remains, and NO assistant row was stored.
    let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chat_messages WHERE session_id = $1")
        .bind(sid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        after, 1,
        "only the prior message should remain after compensation"
    );

    let assistant_n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM chat_messages WHERE session_id = $1 AND role = 'assistant'",
    )
    .bind(sid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(assistant_n, 0, "no assistant row after stream failure");

    // The surviving row is the prior turn, not the failed one.
    let surviving_content: String =
        sqlx::query_scalar("SELECT content FROM chat_messages WHERE session_id = $1")
            .bind(sid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(surviving_content, "prior turn");
}

#[sqlx::test(migrations = "../../migrations")]
async fn sse_stream_failure_does_not_store_partial_assistant_text(pool: PgPool) {
    let tenant = insert_tenant(&pool, "sse-nopartial").await;
    let owner = create_user(&pool, "np@sse.com").await;
    add_tenant_member(&pool, tenant, owner, "member").await;
    let sid = insert_chat_session(&pool, tenant, owner, "np").await;

    let llm: Arc<dyn LlmProvider> = Arc::new(vec_provider(vec![
        Ok(delta("partial answer text ", None)),
        Err(LlmError::Parse("boom".into())),
    ]));

    let conn = rls_conn(&pool, tenant).await;
    let sse = post_chat_sse_with_context(
        tenant,
        sid,
        None,
        "q".into(),
        conn,
        AppPool(pool.clone()),
        llm,
        vec![],
        GraphContext::default(),
    )
    .await
    .unwrap();
    let body = collect_sse_body(sse).await;
    let events = parse_sse_json_chunks(&body);
    assert!(events.iter().any(|e| e["type"] == "text"));

    let assistant_n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM chat_messages WHERE session_id = $1 AND role = 'assistant'",
    )
    .bind(sid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        assistant_n, 0,
        "partial text must NOT be stored as a successful assistant row"
    );

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chat_messages WHERE session_id = $1")
        .bind(sid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(total, 0, "compensation removes the orphan user turn");
}

#[sqlx::test(migrations = "../../migrations")]
async fn sse_citation_unknown_behavior_unchanged(pool: PgPool) {
    let tenant = insert_tenant(&pool, "sse-cu").await;
    let owner = create_user(&pool, "cu@sse.com").await;
    add_tenant_member(&pool, tenant, owner, "member").await;
    let sid = insert_chat_session(&pool, tenant, owner, "cu").await;

    // No chunks → a [chunk:42] tag resolves to citation_unknown.
    let llm: Arc<dyn LlmProvider> = Arc::new(vec_provider(vec![
        Ok(delta("see [chunk:42] ", None)),
        Ok(delta("", Some("stop"))),
    ]));

    let conn = rls_conn(&pool, tenant).await;
    let sse = post_chat_sse_with_context(
        tenant,
        sid,
        None,
        "q".into(),
        conn,
        AppPool(pool.clone()),
        llm,
        vec![],
        GraphContext::default(),
    )
    .await
    .unwrap();
    let body = collect_sse_body(sse).await;
    let events = parse_sse_json_chunks(&body);
    assert!(
        events
            .iter()
            .any(|e| e["type"] == "citation_unknown" && e["index"] == 42),
        "citation_unknown behavior must be preserved"
    );
    assert!(events.iter().any(|e| e["type"] == "done"));
}

fn parse_ok_line(line: &str) -> ChatDelta {
    let payload = line.strip_prefix("data:").unwrap_or(line).trim();
    let v: serde_json::Value = serde_json::from_str(payload).unwrap();
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

// Smoke-check the raw SSE body parser used by ok_provider().
#[test]
fn ok_body_parser_smoke() {
    let line = "data: {\"choices\":[{\"delta\":{\"content\":\"x\"},\"finish_reason\":null}]}";
    let d = parse_ok_line(line);
    assert_eq!(d.content, "x");
    assert!(d.finish_reason.is_none());
}
