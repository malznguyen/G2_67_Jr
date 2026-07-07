//! Chat sessions CRUD + RAG SSE endpoint (T61/T62).

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::{Extension, Path};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures::{Stream, StreamExt};
use gmrag_core::config::{DeepSeekConfig, OllamaConfig, RateLimitConfig};
use gmrag_core::QdrantStore;
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::auth::extractor::AuthUser;
use crate::auth::tenant::TenantContext;
use crate::authz::{
    chat_owner_tuple, chat_session_obj, chat_tenant_tuple, chat_workspace_tuple,
    check_or_unavailable, delete_object_or_unavailable, list_objects_or_unavailable,
    parsed_uuid_set, user_obj, write_or_unavailable, AuthzService, CheckRequest, Consistency,
    REL_OWNER, REL_VIEWER, TYPE_CHAT_SESSION,
};
use crate::chat::streaming::{assistant_text_from_events, meter_rag_chat_completion};
use crate::chat::{
    enrich_stream_events, retrieve_all_with_metering, stream_rag_response, ChatSsePayload,
    ChatStreamEvent, ChunkHit, EnrichedChatStreamEvent, GraphContext, RetrievalParams,
};
use crate::error::ApiError;
use crate::llm::byok::resolve_llm_config;
use crate::llm::provider::{DeepSeekProvider, LlmProvider};
use crate::middleware::rate_limit::{SseConnectionLimiter, SseSlotGuard};
use crate::middleware::rls::SharedConnection;
use crate::pool::AppPool;
use crate::routes::tenants::ensure_path_matches_context;
use crate::routes::workspace_auth::require_workspace_access;

/// Tenant LLM configuration injected at startup (T61).
#[derive(Clone)]
pub struct LlmRuntime {
    pub deepseek: DeepSeekConfig,
    pub ollama: OllamaConfig,
    pub tenant_key_encryption_key: Option<[u8; 32]>,
    /// T84D Phase 3.3: how many past messages to thread into the LLM
    /// context per turn (`GMRAG_CHAT_HISTORY_LIMIT`, default 10).
    pub chat_history_limit: usize,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct ChatSessionRow {
    pub id: Uuid,
    pub title: String,
    pub workspace_id: Option<Uuid>,
    pub model: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Deserialize)]
pub struct CreateSessionBody {
    pub title: Option<String>,
    pub workspace_id: Option<Uuid>,
    pub model: Option<String>,
}

#[derive(Deserialize)]
pub struct PostChatBody {
    pub message: String,
}

struct SessionContext {
    session_id: Uuid,
    workspace_id: Option<Uuid>,
    #[allow(dead_code)]
    model: Option<String>,
}

/// T84D Phase 3.2 — one row of `chat_messages` for the history endpoint.
#[derive(Serialize, sqlx::FromRow)]
pub struct ChatMessageRow {
    pub id: Uuid,
    pub role: String,
    pub content: String,
    pub token_count: Option<i32>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

fn sse_from_enriched(ev: &EnrichedChatStreamEvent) -> Event {
    let payload = ChatSsePayload::from(ev);
    Event::default()
        .json_data(payload)
        .expect("ChatSsePayload serializes")
}

fn sse_error(code: &str, message: impl Into<String>) -> Event {
    Event::default()
        .json_data(ChatSsePayload::Error {
            code: code.into(),
            message: message.into(),
        })
        .expect("error payload serializes")
}

/// List chat sessions visible to the caller.
#[utoipa::path(
    get,
    path = "/tenants/{tid}/chat_sessions",
    tag = "Chat",
    params(
        ("tid" = Uuid, Path, description = "Tenant ID"),
        ("X-Tenant-ID" = Uuid, Header, description = "Must match path tid"),
    ),
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Session list (unpaginated)", body = crate::openapi::schemas::ChatSessionsResponse),
        (status = 400, description = "Bad request", body = crate::openapi::schemas::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::openapi::schemas::ErrorResponse),
        (status = 503, description = "Authorization unavailable", body = crate::openapi::schemas::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::openapi::schemas::ErrorResponse),
    )
)]
pub async fn list_sessions(
    Path(tid): Path<Uuid>,
    Extension(ctx): Extension<TenantContext>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
    Extension(authz): Extension<AuthzService>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;

    let objects = list_objects_or_unavailable(
        &authz,
        &user_obj(auth_user.user_id),
        REL_VIEWER,
        TYPE_CHAT_SESSION,
        Consistency::MinimizeLatency,
    )
    .await?;
    let (session_ids, malformed) = parsed_uuid_set(objects, TYPE_CHAT_SESSION);
    if malformed > 0 {
        tracing::warn!(
            malformed,
            "openfga returned malformed chat_session object ids"
        );
    }
    if session_ids.is_empty() {
        return Ok(Json(serde_json::json!({ "sessions": [] })));
    }

    let mut guard = conn.lock().await;
    let rows = sqlx::query_as::<_, ChatSessionRow>(
        "SELECT cs.id, cs.title, cs.workspace_id, cs.model, cs.created_at, cs.updated_at
         FROM chat_sessions cs
         WHERE cs.id = ANY($1)
         ORDER BY cs.updated_at DESC",
    )
    .bind(&session_ids)
    .fetch_all(&mut *guard)
    .await
    .map_err(|e| ApiError::Internal(format!("list chat sessions: {e}")))?;
    drop(guard);

    Ok(Json(serde_json::json!({ "sessions": rows })))
}

/// Create a chat session owned by the caller.
#[utoipa::path(
    post,
    path = "/tenants/{tid}/chat_sessions",
    tag = "Chat",
    params(
        ("tid" = Uuid, Path, description = "Tenant ID"),
        ("X-Tenant-ID" = Uuid, Header, description = "Must match path tid"),
    ),
    security(("bearer_auth" = [])),
    request_body = crate::openapi::schemas::CreateChatSessionRequest,
    responses(
        (status = 201, description = "Session created", body = crate::openapi::schemas::CreateChatSessionResponse),
        (status = 400, description = "Bad request", body = crate::openapi::schemas::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::openapi::schemas::ErrorResponse),
        (status = 403, description = "Forbidden — workspace access required", body = crate::openapi::schemas::ErrorResponse),
        (status = 404, description = "Workspace not found", body = crate::openapi::schemas::ErrorResponse),
        (status = 503, description = "Authorization unavailable", body = crate::openapi::schemas::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::openapi::schemas::ErrorResponse),
    )
)]
pub async fn create_session(
    Path(tid): Path<Uuid>,
    Extension(ctx): Extension<TenantContext>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
    Extension(authz): Extension<AuthzService>,
    Json(body): Json<CreateSessionBody>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;

    let title = body.title.unwrap_or_default();
    if let Some(workspace_id) = body.workspace_id {
        require_workspace_access(&conn, &authz, workspace_id, auth_user.user_id).await?;
    }

    let mut guard = conn.lock().await;
    let id: (Uuid,) = sqlx::query_as(
        "INSERT INTO chat_sessions (tenant_id, user_id, workspace_id, title, model)
         VALUES ($1, $2, $3, $4, $5)
         RETURNING id",
    )
    .bind(tid)
    .bind(auth_user.user_id)
    .bind(body.workspace_id)
    .bind(&title)
    .bind(&body.model)
    .fetch_one(&mut *guard)
    .await
    .map_err(|e| ApiError::Internal(format!("create chat session: {e}")))?;
    let session_id = id.0;
    let mut tuples = vec![
        chat_tenant_tuple(tid, session_id),
        chat_owner_tuple(auth_user.user_id, session_id),
    ];
    if let Some(workspace_id) = body.workspace_id {
        tuples.push(chat_workspace_tuple(workspace_id, session_id));
    }
    if let Err(e) = write_or_unavailable(&authz, tuples, Vec::new()).await {
        let _ = sqlx::query("DELETE FROM chat_sessions WHERE id = $1")
            .bind(session_id)
            .execute(&mut *guard)
            .await;
        drop(guard);
        return Err(e);
    }
    drop(guard);

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "id": session_id })),
    ))
}

/// Delete a chat session (owner-only).
#[utoipa::path(
    delete,
    path = "/tenants/{tid}/chat_sessions/{sid}",
    tag = "Chat",
    params(
        ("tid" = Uuid, Path, description = "Tenant ID"),
        ("sid" = Uuid, Path, description = "Chat session ID"),
        ("X-Tenant-ID" = Uuid, Header, description = "Must match path tid"),
    ),
    security(("bearer_auth" = [])),
    responses(
        (status = 204, description = "Session deleted"),
        (status = 400, description = "Bad request", body = crate::openapi::schemas::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::openapi::schemas::ErrorResponse),
        (status = 403, description = "Forbidden — owner only", body = crate::openapi::schemas::ErrorResponse),
        (status = 404, description = "Session not found", body = crate::openapi::schemas::ErrorResponse),
        (status = 503, description = "Authorization unavailable", body = crate::openapi::schemas::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::openapi::schemas::ErrorResponse),
    )
)]
pub async fn delete_session(
    Path((tid, sid)): Path<(Uuid, Uuid)>,
    Extension(ctx): Extension<TenantContext>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
    Extension(authz): Extension<AuthzService>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;

    let mut guard = conn.lock().await;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM chat_sessions WHERE id = $1)")
            .bind(sid)
            .fetch_one(&mut *guard)
            .await
            .map_err(|e| ApiError::Internal(format!("load chat session: {e}")))?;
    if !exists {
        drop(guard);
        return Err(ApiError::NotFound);
    }

    let is_owner = check_or_unavailable(
        &authz,
        CheckRequest::new(
            user_obj(auth_user.user_id),
            REL_OWNER,
            chat_session_obj(sid),
        ),
    )
    .await?;
    if !is_owner {
        drop(guard);
        return Err(ApiError::Forbidden(
            "only the chat session owner may delete it".into(),
        ));
    }

    delete_object_or_unavailable(&authz, &chat_session_obj(sid)).await?;

    sqlx::query("DELETE FROM chat_sessions WHERE id = $1")
        .bind(sid)
        .execute(&mut *guard)
        .await
        .map_err(|e| ApiError::Internal(format!("delete chat session: {e}")))?;
    drop(guard);

    Ok(StatusCode::NO_CONTENT)
}

/// T84D Phase 3.2 — list messages for a chat session (viewer-gated).
///
/// Returns `chat_messages` for `sid` ordered by `created_at ASC`. Caller
/// must hold the `viewer` relation on the chat session (ReBAC T64);
/// missing or denied → 404 (no existence leak).
#[utoipa::path(
    get,
    path = "/tenants/{tid}/chat_sessions/{sid}/messages",
    tag = "Chat",
    params(
        ("tid" = Uuid, Path, description = "Tenant ID"),
        ("sid" = Uuid, Path, description = "Chat session ID"),
        ("X-Tenant-ID" = Uuid, Header, description = "Must match path tid"),
    ),
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Messages ordered by created_at ASC",
            body = crate::openapi::schemas::ChatMessagesResponse),
        (status = 400, description = "Bad request", body = crate::openapi::schemas::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::openapi::schemas::ErrorResponse),
        (status = 404, description = "Session not found or no viewer access", body = crate::openapi::schemas::ErrorResponse),
        (status = 503, description = "Authorization unavailable", body = crate::openapi::schemas::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::openapi::schemas::ErrorResponse),
    )
)]
pub async fn list_messages(
    Path((tid, sid)): Path<(Uuid, Uuid)>,
    Extension(ctx): Extension<TenantContext>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
    Extension(authz): Extension<AuthzService>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;

    // Viewer-gate (404 on missing-or-denied — no existence leak).
    {
        let mut guard = conn.lock().await;
        authorize_chat_session(&authz, &mut guard, sid, auth_user.user_id).await?;
    }

    let mut guard = conn.lock().await;
    let rows = sqlx::query_as::<_, ChatMessageRow>(
        "SELECT id, role, content, token_count, created_at
         FROM chat_messages
         WHERE session_id = $1
         ORDER BY created_at ASC",
    )
    .bind(sid)
    .fetch_all(&mut *guard)
    .await
    .map_err(|e| ApiError::Internal(format!("list chat messages: {e}")))?;
    drop(guard);

    Ok(Json(serde_json::json!({ "messages": rows })))
}

/// RAG chat over Server-Sent Events (viewer-gated).
///
/// Response is `text/event-stream` with JSON `data:` lines matching [`ChatSseEvent`].
/// In-stream failures use `{ "type": "error", "code": "...", "message": "..." }`
/// (not the HTTP `{ "error": { ... } }` envelope). Codes include `stream-failed`
/// and `persist-failed`. Pre-stream validation errors still use the standard HTTP
/// error JSON. Swagger UI has limited SSE support; use curl or EventSource for
/// full stream testing.
#[utoipa::path(
    post,
    path = "/tenants/{tid}/chat_sessions/{sid}/chat",
    tag = "Chat",
    params(
        ("tid" = Uuid, Path, description = "Tenant ID"),
        ("sid" = Uuid, Path, description = "Chat session ID"),
        ("X-Tenant-ID" = Uuid, Header, description = "Must match path tid"),
    ),
    security(("bearer_auth" = [])),
    request_body = crate::openapi::schemas::PostChatRequest,
    responses(
        (status = 200, description = "SSE stream of ChatSseEvent payloads",
            content_type = "text/event-stream",
            body = crate::openapi::schemas::ChatSseEvent),
        (status = 400, description = "Bad request", body = crate::openapi::schemas::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::openapi::schemas::ErrorResponse),
        (status = 404, description = "Session not found or no viewer access", body = crate::openapi::schemas::ErrorResponse),
        (status = 429, description = "Too many open chat streams", body = crate::openapi::schemas::ErrorResponse),
        (status = 503, description = "Authorization unavailable", body = crate::openapi::schemas::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::openapi::schemas::ErrorResponse),
    )
)]
#[allow(clippy::too_many_arguments)]
pub async fn post_chat(
    Path((tid, sid)): Path<(Uuid, Uuid)>,
    Extension(ctx): Extension<TenantContext>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
    Extension(app_pool): Extension<AppPool>,
    Extension(qdrant): Extension<QdrantStore>,
    Extension(llm_runtime): Extension<LlmRuntime>,
    Extension(authz): Extension<AuthzService>,
    Extension(rate_cfg): Extension<RateLimitConfig>,
    Extension(sse_limiter): Extension<SseConnectionLimiter>,
    Json(body): Json<PostChatBody>,
) -> Result<Response, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;

    if body.message.trim().is_empty() {
        return Err(ApiError::BadRequest("message must not be empty".into()));
    }

    let sse_slot = if rate_cfg.enabled {
        match sse_limiter
            .try_acquire(tid, rate_cfg.chat_concurrent_per_tenant)
            .await
        {
            Ok(slot) => Some(slot),
            Err(retry_after_secs) => {
                crate::metrics::metrics().inc_rate_limit_rejection("chat_concurrent");
                let mut response = (
                    StatusCode::TOO_MANY_REQUESTS,
                    Json(serde_json::json!({
                        "error": {
                            "code": "rate-limit-exceeded",
                            "message": "too many open chat streams",
                            "category": "chat_concurrent"
                        }
                    })),
                )
                    .into_response();
                let retry = HeaderValue::from_str(&retry_after_secs.max(1).to_string())
                    .unwrap_or_else(|_| HeaderValue::from_static("1"));
                response.headers_mut().insert(header::RETRY_AFTER, retry);
                return Ok(response);
            }
        }
    } else {
        None
    };

    let mut guard = conn.lock().await;
    let resolved = resolve_llm_config(
        &mut guard,
        tid,
        &llm_runtime.deepseek,
        &llm_runtime.ollama,
        llm_runtime.tenant_key_encryption_key.as_ref(),
    )
    .await
    .map_err(|e| ApiError::Internal(format!("resolve llm config: {e}")))?;

    let session = load_session_for_chat(&authz, &mut guard, sid, auth_user.user_id).await?;
    drop(guard);

    let provider = Arc::new(DeepSeekProvider::new(resolved.provider));
    post_chat_sse(
        tid,
        session,
        body.message,
        auth_user.user_id,
        conn,
        app_pool,
        authz,
        &qdrant,
        provider,
        llm_runtime.chat_history_limit,
        sse_slot,
    )
    .await
    .map(IntoResponse::into_response)
}

/// Authorize viewer access to a chat session (404 if missing or denied).
pub async fn authorize_chat_session(
    authz: &AuthzService,
    conn: &mut PgConnection,
    sid: Uuid,
    user_id: Uuid,
) -> Result<(), ApiError> {
    load_session_for_chat(authz, conn, sid, user_id)
        .await
        .map(|_| ())
}

async fn load_session_for_chat(
    authz: &AuthzService,
    conn: &mut PgConnection,
    sid: Uuid,
    user_id: Uuid,
) -> Result<SessionContext, ApiError> {
    let row: Option<(Option<Uuid>, Option<String>)> =
        sqlx::query_as("SELECT workspace_id, model FROM chat_sessions WHERE id = $1")
            .bind(sid)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|e| ApiError::Internal(format!("load chat session: {e}")))?;

    let Some((workspace_id, model)) = row else {
        return Err(ApiError::NotFound);
    };

    let can_view = check_or_unavailable(
        authz,
        CheckRequest::new(user_obj(user_id), REL_VIEWER, chat_session_obj(sid)),
    )
    .await?;
    if !can_view {
        return Err(ApiError::NotFound);
    }

    Ok(SessionContext {
        session_id: sid,
        workspace_id,
        model,
    })
}

/// T84D Phase 3.3 — load the last `limit` chat messages for `sid` (in
/// chronological order) to thread into the LLM context. Returns
/// `Vec<ChatMessage>` ready to prepend between system + current user
/// messages in `stream_rag_response`.
async fn load_chat_history(
    conn: &mut sqlx::PgConnection,
    sid: Uuid,
    limit: usize,
) -> Result<Vec<crate::llm::provider::ChatMessage>, ApiError> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);
    // DESC LIMIT then reverse → chronological order, the last `limit`
    // messages recorded for this session (excluding the turn we just
    // inserted, which the caller inserts BEFORE invoking this helper).
    let rows: Vec<(String, String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        r#"
        SELECT role, content, created_at
        FROM chat_messages
        WHERE session_id = $1
        ORDER BY created_at DESC
        LIMIT $2
        "#,
    )
    .bind(sid)
    .bind(limit_i64)
    .fetch_all(conn)
    .await
    .map_err(|e| ApiError::Internal(format!("load chat history: {e}")))?;
    let mut ordered: Vec<_> = rows
        .into_iter()
        .map(|(role, content, _)| crate::llm::provider::ChatMessage::new(role, content))
        .collect();
    ordered.reverse();
    Ok(ordered)
}

/// T84D Phase 5 / Phase 0 TASK-P0-03 — compensate an orphan user message.
///
/// If upstream streaming fails before a durable assistant response is
/// stored, the user message we inserted at the start of the turn would
/// remain as a user-only orphan turn. This deletes EXACTLY that one
/// message row (by `id`), in a tenant-scoped transaction, so no other
/// concurrent messages are touched. Never deletes by timestamp or broad
/// session criteria.
///
/// Best-effort: a compensation failure is warn-logged but does NOT
/// suppress the `stream-failed` event the client still receives.
async fn compensate_user_message(
    pool: &AppPool,
    tenant_id: Uuid,
    user_message_id: Uuid,
) -> Result<(), sqlx::Error> {
    let acquired = pool.acquire().await?;
    let mut conn = acquired.detach();

    sqlx::Executor::execute(&mut conn, "BEGIN").await?;
    sqlx::Executor::execute(&mut conn, "SET LOCAL ROLE gmrag_app").await?;
    sqlx::query(&format!("SET LOCAL app.tenant_id = '{tenant_id}'"))
        .execute(&mut conn)
        .await?;

    let res = sqlx::query("DELETE FROM chat_messages WHERE id = $1")
        .bind(user_message_id)
        .execute(&mut conn)
        .await;

    match res {
        Ok(_) => {
            sqlx::Executor::execute(&mut conn, "COMMIT").await?;
            Ok(())
        }
        Err(e) => {
            let _ = sqlx::Executor::execute(&mut conn, "ROLLBACK").await;
            Err(e)
        }
    }
}

/// SSE `done` event carrying the preserved upstream `finish_reason`.
fn sse_done(finish_reason: Option<String>) -> Event {
    Event::default()
        .json_data(ChatSsePayload::Done { finish_reason })
        .expect("done payload serializes")
}

struct SseOutcomeGuard {
    terminal_recorded: bool,
}

impl SseOutcomeGuard {
    fn new() -> Self {
        Self {
            terminal_recorded: false,
        }
    }

    fn record(&mut self, outcome: &'static str) {
        if !self.terminal_recorded {
            crate::metrics::metrics().inc_sse_outcome(outcome);
            self.terminal_recorded = true;
        }
    }
}

impl Drop for SseOutcomeGuard {
    fn drop(&mut self) {
        if !self.terminal_recorded {
            crate::metrics::metrics().inc_sse_outcome("dropped");
            self.terminal_recorded = true;
        }
    }
}

/// Core SSE chat implementation.
///
/// Phase 0 TASK-P0-03 (durable-before-done) contract:
/// - text/citation events are streamed as they arrive;
/// - the upstream `Done` event is HELD — `done` is only emitted AFTER the
///   assistant message + usage are successfully persisted;
/// - on persistence failure: emit `persist-failed` (no `done`);
/// - on upstream stream failure: emit `stream-failed` (no `done`) and
///   compensate the just-created user message so no user-only orphan turn
///   remains. Compensation failure is warn-logged but the client still
///   receives `stream-failed`;
/// - exactly one terminal application event is emitted (`done` XOR
///   `persist-failed` XOR `stream-failed`).
#[allow(clippy::too_many_arguments)]
async fn post_chat_sse(
    tenant_id: Uuid,
    session: SessionContext,
    user_message: String,
    user_id: Uuid,
    conn: SharedConnection,
    pool: AppPool,
    authz: AuthzService,
    qdrant: &QdrantStore,
    provider: Arc<dyn LlmProvider>,
    chat_history_limit: usize,
    sse_slot: Option<SseSlotGuard>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let sid = session.session_id;

    // Phase A — request transaction: user message, retrieval, history.
    let (chunks, graph, history, user_message_id) = {
        let mut guard = conn.lock().await;

        let user_message_id: (Uuid,) = sqlx::query_as(
            "INSERT INTO chat_messages (tenant_id, session_id, role, content)
             VALUES ($1, $2, 'user', $3)
             RETURNING id",
        )
        .bind(tenant_id)
        .bind(sid)
        .bind(&user_message)
        .fetch_one(&mut *guard)
        .await
        .map_err(|e| ApiError::Internal(format!("insert user message: {e}")))?;

        let (chunks, graph) = if let Some(workspace_id) = session.workspace_id {
            let params = RetrievalParams::new(tenant_id, workspace_id, user_id, &user_message);
            retrieve_all_with_metering(&mut guard, &authz, qdrant, provider.as_ref(), &params)
                .await
                .map_err(|e| ApiError::Internal(format!("retrieval: {e}")))?
        } else {
            (Vec::new(), GraphContext::default())
        };

        // T84D Phase 3.3: load the session history (now including the user
        // message we just inserted) and thread it into the LLM context.
        let history = load_chat_history(&mut guard, sid, chat_history_limit).await?;

        sqlx::query("UPDATE chat_sessions SET updated_at = now() WHERE id = $1")
            .bind(sid)
            .execute(&mut *guard)
            .await
            .map_err(|e| ApiError::Internal(format!("touch session: {e}")))?;

        (chunks, graph, history, user_message_id.0)
    };

    let query = user_message.clone();
    let chunks_for_stream = chunks.clone();
    let graph_for_stream = graph.clone();
    let history_for_stream = history.clone();
    let pool_for_post = pool.clone();
    let pool_for_comp = pool.clone();
    let provider_for_stream = Arc::clone(&provider);

    let stream = async_stream::stream! {
        let _sse_slot = sse_slot;
        let mut outcome_guard = SseOutcomeGuard::new();
        let mut raw_events: Vec<ChatStreamEvent> = Vec::new();
        // Held-back upstream Done finish_reason — only emitted after durable
        // persistence succeeds (durable-before-done).
        let mut held_finish_reason: Option<String> = None;

        let mut upstream = match stream_rag_response(
            provider_for_stream.as_ref(),
            &chunks_for_stream,
            &graph_for_stream,
            &query,
            &history_for_stream,
        )
        .await
        {
            Ok(s) => s,
            Err(e) => {
                // Upstream failed to even start — compensate the user turn.
                if let Err(comp) =
                    compensate_user_message(&pool_for_comp, tenant_id, user_message_id).await
                {
                    tracing::warn!(
                        error = %comp,
                        tenant_id = %tenant_id,
                        session_id = %sid,
                        user_message_id = %user_message_id,
                        "compensation failed after stream start failure"
                    );
                }
                outcome_guard.record("error");
                yield Ok(sse_error("stream-failed", e.to_string()));
                return;
            }
        };

        let mut stream_failed: Option<String> = None;
        while let Some(item) = upstream.next().await {
            match item {
                Ok(ev) => {
                    // Hold back the Done event: persist before emitting done.
                    if let ChatStreamEvent::Done { finish_reason } = &ev {
                        held_finish_reason = finish_reason.clone();
                        // Still record it so assistant_text_from_events sees the
                        // full event sequence (Done carries no text anyway).
                        raw_events.push(ev.clone());
                        continue;
                    }
                    raw_events.push(ev.clone());
                    for enriched in enrich_stream_events(&chunks_for_stream, &[ev]) {
                        yield Ok(sse_from_enriched(&enriched));
                    }
                }
                Err(e) => {
                    stream_failed = Some(e.to_string());
                    break;
                }
            }
        }

        if let Some(msg) = stream_failed {
            // Upstream stream failed mid-flight — compensate the user turn.
            if let Err(comp) =
                compensate_user_message(&pool_for_comp, tenant_id, user_message_id).await
            {
                tracing::warn!(
                    error = %comp,
                    tenant_id = %tenant_id,
                    session_id = %sid,
                    user_message_id = %user_message_id,
                    "compensation failed after stream failure"
                );
            }
            outcome_guard.record("error");
            yield Ok(sse_error("stream-failed", msg));
            return;
        }

        let assistant_text = assistant_text_from_events(&raw_events);
        match persist_chat_completion(
            &pool_for_post,
            tenant_id,
            sid,
            Arc::clone(&provider_for_stream),
            &chunks_for_stream,
            &graph_for_stream,
            &query,
            &raw_events,
            &assistant_text,
        )
        .await
        {
            Ok(()) => {
                // Durable-before-done: persistence succeeded, emit the
                // single terminal success event with the preserved
                // finish_reason.
                outcome_guard.record("done");
                yield Ok(sse_done(held_finish_reason));
            }
            Err(e) => {
                // Persistence failed — do NOT emit done. The assistant row
                // was rolled back inside persist_chat_completion.
                outcome_guard.record("error");
                yield Ok(sse_error("persist-failed", e.to_string()));
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

#[allow(clippy::too_many_arguments)]
async fn persist_chat_completion(
    pool: &AppPool,
    tenant_id: Uuid,
    sid: Uuid,
    provider: Arc<dyn LlmProvider>,
    chunks: &[ChunkHit],
    graph: &GraphContext,
    query: &str,
    raw_events: &[ChatStreamEvent],
    assistant_text: &str,
) -> Result<(), sqlx::Error> {
    let acquired = pool.acquire().await?;
    let mut conn = acquired.detach();

    sqlx::Executor::execute(&mut conn, "BEGIN").await?;
    sqlx::Executor::execute(&mut conn, "SET LOCAL ROLE gmrag_app").await?;
    sqlx::query(&format!("SET LOCAL app.tenant_id = '{tenant_id}'"))
        .execute(&mut conn)
        .await?;

    match persist_chat_completion_in_tx(
        &mut conn,
        tenant_id,
        sid,
        provider.as_ref(),
        chunks,
        graph,
        query,
        raw_events,
        assistant_text,
    )
    .await
    {
        Ok(()) => {
            sqlx::Executor::execute(&mut conn, "COMMIT").await?;
            Ok(())
        }
        Err(err) => {
            let _ = sqlx::Executor::execute(&mut conn, "ROLLBACK").await;
            Err(err)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn persist_chat_completion_in_tx(
    pg: &mut PgConnection,
    tenant_id: Uuid,
    sid: Uuid,
    provider: &dyn LlmProvider,
    chunks: &[ChunkHit],
    graph: &GraphContext,
    query: &str,
    raw_events: &[ChatStreamEvent],
    assistant_text: &str,
) -> Result<(), sqlx::Error> {
    meter_rag_chat_completion(pg, tenant_id, provider, chunks, graph, query, raw_events)
        .await
        .map_err(|e| sqlx::Error::Protocol(format!("meter chat: {e}")))?;

    sqlx::query(
        "INSERT INTO chat_messages (tenant_id, session_id, role, content)
         VALUES ($1, $2, 'assistant', $3)",
    )
    .bind(tenant_id)
    .bind(sid)
    .bind(assistant_text)
    .execute(&mut *pg)
    .await?;

    sqlx::query("UPDATE chat_sessions SET updated_at = now() WHERE id = $1")
        .bind(sid)
        .execute(&mut *pg)
        .await?;

    Ok(())
}

/// Test helper: SSE chat with pre-built retrieval context (avoids Qdrant in unit tests).
#[allow(clippy::too_many_arguments)]
pub async fn post_chat_sse_with_context(
    tenant_id: Uuid,
    session_id: Uuid,
    workspace_id: Option<Uuid>,
    user_message: String,
    conn: SharedConnection,
    pool: AppPool,
    provider: Arc<dyn LlmProvider>,
    chunks: Vec<ChunkHit>,
    graph: GraphContext,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let session = SessionContext {
        session_id,
        workspace_id,
        model: None,
    };
    post_chat_sse_with_context_inner(
        tenant_id,
        session,
        user_message,
        conn,
        pool,
        provider,
        chunks,
        graph,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn post_chat_sse_with_context_inner(
    tenant_id: Uuid,
    session: SessionContext,
    user_message: String,
    conn: SharedConnection,
    pool: AppPool,
    provider: Arc<dyn LlmProvider>,
    chunks: Vec<ChunkHit>,
    graph: GraphContext,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let sid = session.session_id;

    let user_message_id: Uuid = {
        let mut guard = conn.lock().await;
        let row: (Uuid,) = sqlx::query_as(
            "INSERT INTO chat_messages (tenant_id, session_id, role, content)
             VALUES ($1, $2, 'user', $3)
             RETURNING id",
        )
        .bind(tenant_id)
        .bind(sid)
        .bind(&user_message)
        .fetch_one(&mut *guard)
        .await
        .map_err(|e| ApiError::Internal(format!("insert user message: {e}")))?;
        row.0
    };

    let query = user_message.clone();
    let chunks_for_stream = chunks.clone();
    let graph_for_stream = graph.clone();
    let pool_for_post = pool.clone();
    let pool_for_comp = pool.clone();
    let provider_for_stream = Arc::clone(&provider);

    let stream = async_stream::stream! {
        let mut raw_events: Vec<ChatStreamEvent> = Vec::new();
        let mut held_finish_reason: Option<String> = None;

        let mut upstream = match stream_rag_response(
            provider_for_stream.as_ref(),
            &chunks_for_stream,
            &graph_for_stream,
            &query,
            // T84D Phase 3.3: the with-context test helper does NOT load
            // chat history (its tests inject pre-built retrieval context
            // directly). Pass an empty slice to preserve prior behaviour.
            &[],
        )
        .await
        {
            Ok(s) => s,
            Err(e) => {
                if let Err(comp) =
                    compensate_user_message(&pool_for_comp, tenant_id, user_message_id).await
                {
                    tracing::warn!(
                        error = %comp,
                        tenant_id = %tenant_id,
                        session_id = %sid,
                        user_message_id = %user_message_id,
                        "compensation failed after stream start failure"
                    );
                }
                yield Ok(sse_error("stream-failed", e.to_string()));
                return;
            }
        };

        let mut stream_failed: Option<String> = None;
        while let Some(item) = upstream.next().await {
            match item {
                Ok(ev) => {
                    if let ChatStreamEvent::Done { finish_reason } = &ev {
                        held_finish_reason = finish_reason.clone();
                        raw_events.push(ev.clone());
                        continue;
                    }
                    raw_events.push(ev.clone());
                    for enriched in enrich_stream_events(&chunks_for_stream, &[ev]) {
                        yield Ok(sse_from_enriched(&enriched));
                    }
                }
                Err(e) => {
                    stream_failed = Some(e.to_string());
                    break;
                }
            }
        }

        if let Some(msg) = stream_failed {
            if let Err(comp) =
                compensate_user_message(&pool_for_comp, tenant_id, user_message_id).await
            {
                tracing::warn!(
                    error = %comp,
                    tenant_id = %tenant_id,
                    session_id = %sid,
                    user_message_id = %user_message_id,
                    "compensation failed after stream failure"
                );
            }
            yield Ok(sse_error("stream-failed", msg));
            return;
        }

        let assistant_text = assistant_text_from_events(&raw_events);
        match persist_chat_completion(
            &pool_for_post,
            tenant_id,
            sid,
            Arc::clone(&provider_for_stream),
            &chunks_for_stream,
            &graph_for_stream,
            &query,
            &raw_events,
            &assistant_text,
        )
        .await
        {
            Ok(()) => {
                yield Ok(sse_done(held_finish_reason));
            }
            Err(e) => {
                yield Ok(sse_error("persist-failed", e.to_string()));
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}
