//! Chat sessions CRUD + RAG SSE endpoint (T61/T62).

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::{Extension, Path};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::Json;
use futures::{Stream, StreamExt};
use gmrag_core::config::{DeepSeekConfig, OllamaConfig};
use gmrag_core::QdrantStore;
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::auth::extractor::AuthUser;
use crate::auth::tenant::TenantContext;
use crate::chat::{
    enrich_stream_events, retrieve_all_with_metering, stream_rag_response, ChatSsePayload,
    ChatStreamEvent, ChunkHit, EnrichedChatStreamEvent, GraphContext, RetrievalParams,
};
use crate::chat::streaming::{assistant_text_from_events, meter_rag_chat_completion};
use crate::error::ApiError;
use crate::llm::byok::resolve_llm_config;
use crate::llm::provider::{DeepSeekProvider, LlmProvider};
use crate::middleware::rls::SharedConnection;
use crate::pool::AppPool;
use crate::rbac::check::check_relation;
use crate::rbac::model::{ObjectRef, Principal, Relation, NS_CHAT_SESSION};
use crate::routes::tenants::ensure_path_matches_context;

/// Tenant LLM configuration injected at startup (T61).
#[derive(Clone)]
pub struct LlmRuntime {
    pub deepseek: DeepSeekConfig,
    pub ollama: OllamaConfig,
    pub tenant_key_encryption_key: Option<[u8; 32]>,
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

/// `GET /tenants/{tid}/chat_sessions` — list sessions visible to the caller (T62).
pub async fn list_sessions(
    Path(tid): Path<Uuid>,
    Extension(ctx): Extension<TenantContext>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;

    let user_id = auth_user.user_id;
    let mut guard = conn.lock().await;
    let rows = sqlx::query_as::<_, ChatSessionRow>(
        "SELECT cs.id, cs.title, cs.workspace_id, cs.model, cs.created_at, cs.updated_at
         FROM chat_sessions cs
         WHERE (
                 cs.user_id = $1
                 OR (cs.workspace_id IS NOT NULL AND EXISTS (
                       SELECT 1 FROM workspace_members wm
                       WHERE wm.workspace_id = cs.workspace_id
                         AND wm.user_id = $1))
                 OR EXISTS (
                       SELECT 1 FROM resource_acl ra
                       WHERE ra.resource_type = 'chat_session'
                         AND ra.resource_id = cs.id
                         AND ra.permission IN ('owner', 'editor', 'viewer')
                         AND (
                               (ra.principal_type = 'user' AND ra.principal_id = $1)
                               OR (ra.principal_type = 'workspace' AND EXISTS (
                                     SELECT 1 FROM workspace_members wmg
                                     WHERE wmg.workspace_id = ra.principal_id
                                       AND wmg.user_id = $1))
                             ))
               )
         ORDER BY cs.updated_at DESC",
    )
    .bind(user_id)
    .fetch_all(&mut *guard)
    .await
    .map_err(|e| ApiError::Internal(format!("list chat sessions: {e}")))?;
    drop(guard);

    Ok(Json(serde_json::json!({ "sessions": rows })))
}

/// `POST /tenants/{tid}/chat_sessions` — create session owned by caller (T62).
pub async fn create_session(
    Path(tid): Path<Uuid>,
    Extension(ctx): Extension<TenantContext>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
    Json(body): Json<CreateSessionBody>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;

    let title = body.title.unwrap_or_default();
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
    drop(guard);

    Ok((StatusCode::CREATED, Json(serde_json::json!({ "id": id.0 }))))
}

/// `DELETE /tenants/{tid}/chat_sessions/{sid}` — owner-only delete (T62).
pub async fn delete_session(
    Path((tid, sid)): Path<(Uuid, Uuid)>,
    Extension(ctx): Extension<TenantContext>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;

    let mut guard = conn.lock().await;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM chat_sessions WHERE id = $1)")
        .bind(sid)
        .fetch_one(&mut *guard)
        .await
        .map_err(|e| ApiError::Internal(format!("load chat session: {e}")))?;
    if !exists {
        drop(guard);
        return Err(ApiError::NotFound);
    }

    let is_owner = check_relation(
        &mut guard,
        &ObjectRef::new(NS_CHAT_SESSION, sid),
        Relation::Owner,
        Principal::User(auth_user.user_id),
    )
    .await
    .map_err(|e| ApiError::Internal(format!("owner check: {e}")))?;
    if !is_owner {
        drop(guard);
        return Err(ApiError::Forbidden(
            "only the chat session owner may delete it".into(),
        ));
    }

    sqlx::query("DELETE FROM chat_sessions WHERE id = $1")
        .bind(sid)
        .execute(&mut *guard)
        .await
        .map_err(|e| ApiError::Internal(format!("delete chat session: {e}")))?;
    drop(guard);

    Ok(StatusCode::NO_CONTENT)
}

/// `POST /tenants/{tid}/chat_sessions/{sid}/chat` — RAG chat SSE (T61).
#[allow(clippy::too_many_arguments)]
pub async fn post_chat(
    Path((tid, sid)): Path<(Uuid, Uuid)>,
    Extension(ctx): Extension<TenantContext>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
    Extension(app_pool): Extension<AppPool>,
    Extension(qdrant): Extension<QdrantStore>,
    Extension(llm_runtime): Extension<LlmRuntime>,
    Json(body): Json<PostChatBody>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;

    if body.message.trim().is_empty() {
        return Err(ApiError::BadRequest("message must not be empty".into()));
    }

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

    let session = load_session_for_chat(&mut guard, sid, auth_user.user_id).await?;
    drop(guard);

    let provider = Arc::new(DeepSeekProvider::new(resolved.provider));
    post_chat_sse(
        tid,
        session,
        body.message,
        auth_user.user_id,
        conn,
        app_pool,
        &qdrant,
        provider,
    )
    .await
}

/// Authorize viewer access to a chat session (404 if missing or denied).
pub async fn authorize_chat_session(
    conn: &mut PgConnection,
    sid: Uuid,
    user_id: Uuid,
) -> Result<(), ApiError> {
    load_session_for_chat(conn, sid, user_id).await.map(|_| ())
}

async fn load_session_for_chat(
    conn: &mut PgConnection,
    sid: Uuid,
    user_id: Uuid,
) -> Result<SessionContext, ApiError> {
    let row: Option<(Option<Uuid>, Option<String>)> = sqlx::query_as(
        "SELECT workspace_id, model FROM chat_sessions WHERE id = $1",
    )
    .bind(sid)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| ApiError::Internal(format!("load chat session: {e}")))?;

    let Some((workspace_id, model)) = row else {
        return Err(ApiError::NotFound);
    };

    let can_view = check_relation(
        conn,
        &ObjectRef::new(NS_CHAT_SESSION, sid),
        Relation::Viewer,
        Principal::User(user_id),
    )
    .await
    .map_err(|e| ApiError::Internal(format!("viewer check: {e}")))?;
    if !can_view {
        return Err(ApiError::NotFound);
    }

    Ok(SessionContext {
        session_id: sid,
        workspace_id,
        model,
    })
}

/// Core SSE chat implementation.
#[allow(clippy::too_many_arguments)]
async fn post_chat_sse(
    tenant_id: Uuid,
    session: SessionContext,
    user_message: String,
    user_id: Uuid,
    conn: SharedConnection,
    pool: AppPool,
    qdrant: &QdrantStore,
    provider: Arc<dyn LlmProvider>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let sid = session.session_id;

    // Phase A — request transaction: history, user message, retrieval.
    let (chunks, graph) = {
        let mut guard = conn.lock().await;

        sqlx::query(
            "INSERT INTO chat_messages (tenant_id, session_id, role, content)
             VALUES ($1, $2, 'user', $3)",
        )
        .bind(tenant_id)
        .bind(sid)
        .bind(&user_message)
        .execute(&mut *guard)
        .await
        .map_err(|e| ApiError::Internal(format!("insert user message: {e}")))?;

        let (chunks, graph) = if let Some(workspace_id) = session.workspace_id {
            let params = RetrievalParams::new(tenant_id, workspace_id, user_id, &user_message);
            retrieve_all_with_metering(&mut guard, qdrant, provider.as_ref(), &params)
                .await
                .map_err(|e| ApiError::Internal(format!("retrieval: {e}")))?
        } else {
            (Vec::new(), GraphContext::default())
        };

        sqlx::query("UPDATE chat_sessions SET updated_at = now() WHERE id = $1")
            .bind(sid)
            .execute(&mut *guard)
            .await
            .map_err(|e| ApiError::Internal(format!("touch session: {e}")))?;

        (chunks, graph)
    };

    let query = user_message.clone();
    let chunks_for_stream = chunks.clone();
    let graph_for_stream = graph.clone();
    let pool_for_post = pool.clone();
    let provider_for_stream = Arc::clone(&provider);

    let stream = async_stream::stream! {
        let mut raw_events: Vec<ChatStreamEvent> = Vec::new();

        let mut upstream = match stream_rag_response(
            provider_for_stream.as_ref(),
            &chunks_for_stream,
            &graph_for_stream,
            &query,
        )
        .await
        {
            Ok(s) => s,
            Err(e) => {
                yield Ok(sse_error("stream-failed", e.to_string()));
                return;
            }
        };

        while let Some(item) = upstream.next().await {
            match item {
                Ok(ev) => {
                    raw_events.push(ev.clone());
                    for enriched in enrich_stream_events(&chunks_for_stream, &[ev]) {
                        yield Ok(sse_from_enriched(&enriched));
                    }
                }
                Err(e) => {
                    yield Ok(sse_error("stream-failed", e.to_string()));
                    return;
                }
            }
        }

        let assistant_text = assistant_text_from_events(&raw_events);
        if let Err(e) = persist_chat_completion(
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
            yield Ok(sse_error("persist-failed", e.to_string()));
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

    {
        let mut guard = conn.lock().await;
        sqlx::query(
            "INSERT INTO chat_messages (tenant_id, session_id, role, content)
             VALUES ($1, $2, 'user', $3)",
        )
        .bind(tenant_id)
        .bind(sid)
        .bind(&user_message)
        .execute(&mut *guard)
        .await
        .map_err(|e| ApiError::Internal(format!("insert user message: {e}")))?;
    }

    let query = user_message.clone();
    let chunks_for_stream = chunks.clone();
    let graph_for_stream = graph.clone();
    let pool_for_post = pool.clone();
    let provider_for_stream = Arc::clone(&provider);

    let stream = async_stream::stream! {
        let mut raw_events: Vec<ChatStreamEvent> = Vec::new();

        let mut upstream = match stream_rag_response(
            provider_for_stream.as_ref(),
            &chunks_for_stream,
            &graph_for_stream,
            &query,
        )
        .await
        {
            Ok(s) => s,
            Err(e) => {
                yield Ok(sse_error("stream-failed", e.to_string()));
                return;
            }
        };

        while let Some(item) = upstream.next().await {
            match item {
                Ok(ev) => {
                    raw_events.push(ev.clone());
                    for enriched in enrich_stream_events(&chunks_for_stream, &[ev]) {
                        yield Ok(sse_from_enriched(&enriched));
                    }
                }
                Err(e) => {
                    yield Ok(sse_error("stream-failed", e.to_string()));
                    return;
                }
            }
        }

        let assistant_text = assistant_text_from_events(&raw_events);
        let _ = persist_chat_completion(
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
        .await;
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}
