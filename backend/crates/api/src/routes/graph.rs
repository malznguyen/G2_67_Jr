//! Workspace knowledge-graph read API (T63).

use axum::extract::{Extension, Path, Query};
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::extractor::AuthUser;
use crate::auth::tenant::TenantContext;
use crate::authz::AuthzService;
use crate::chat::retrieval::{
    accessible_document_ids, node_visible_via_provenance, RetrievalError,
};
use crate::error::ApiError;
use crate::middleware::rls::SharedConnection;
use crate::openapi::schemas::WorkspaceGraphResponse;
use crate::routes::tenants::ensure_path_matches_context;
use crate::routes::workspace_auth::require_workspace_access_hidden;

const DEFAULT_GRAPH_PAGE_LIMIT: u32 = 200;
const MAX_GRAPH_PAGE_LIMIT: u32 = 500;

#[derive(Debug, Deserialize)]
pub struct GraphQueryParams {
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Serialize, sqlx::FromRow)]
struct GraphNodeRow {
    id: Uuid,
    kind: String,
    label: String,
    properties: serde_json::Value,
    created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize, sqlx::FromRow)]
struct GraphEdgeRow {
    id: Uuid,
    src_node_id: Uuid,
    dst_node_id: Uuid,
    kind: String,
    weight: f32,
    properties: serde_json::Value,
    created_at: chrono::DateTime<chrono::Utc>,
}

fn effective_limit(limit: Option<u32>) -> u32 {
    limit
        .unwrap_or(DEFAULT_GRAPH_PAGE_LIMIT)
        .clamp(1, MAX_GRAPH_PAGE_LIMIT)
}

fn encode_cursor(row: &GraphNodeRow) -> String {
    format!("{}:{}", row.created_at.to_rfc3339(), row.id)
}

fn decode_cursor(cursor: &str) -> Result<(chrono::DateTime<chrono::Utc>, Uuid), ApiError> {
    if cursor.len() < 38 {
        return Err(ApiError::BadRequest("invalid cursor".into()));
    }
    let id = Uuid::parse_str(&cursor[cursor.len() - 36..])
        .map_err(|_| ApiError::BadRequest("invalid cursor".into()))?;
    if cursor.as_bytes().get(cursor.len().saturating_sub(37)) != Some(&b':') {
        return Err(ApiError::BadRequest("invalid cursor".into()));
    }
    let ts_str = &cursor[..cursor.len() - 37];
    let created_at = chrono::DateTime::parse_from_rfc3339(ts_str)
        .map_err(|_| ApiError::BadRequest("invalid cursor".into()))?
        .with_timezone(&chrono::Utc);
    Ok((created_at, id))
}

fn map_retrieval_error(err: RetrievalError) -> ApiError {
    match err {
        RetrievalError::NotWorkspaceMember => ApiError::NotFound,
        other => ApiError::Internal(other.to_string()),
    }
}

/// Full workspace knowledge graph (member-gated, cursor-paginated).
#[utoipa::path(
    get,
    path = "/tenants/{tid}/workspaces/{wid}/graph",
    tag = "Graph",
    params(
        ("tid" = Uuid, Path, description = "Tenant ID"),
        ("wid" = Uuid, Path, description = "Workspace ID"),
        ("X-Tenant-ID" = Uuid, Header, description = "Must match path tid"),
        ("cursor" = Option<String>, Query, description = "Pagination cursor (RFC3339:UUID)"),
        ("limit" = Option<u32>, Query, description = "Page size (default 200, max 500)"),
    ),
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Graph nodes and edges", body = crate::openapi::schemas::WorkspaceGraphResponse),
        (status = 400, description = "Bad request", body = crate::openapi::schemas::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::openapi::schemas::ErrorResponse),
        (status = 404, description = "Workspace not found or not a member", body = crate::openapi::schemas::ErrorResponse),
        (status = 503, description = "Authorization unavailable", body = crate::openapi::schemas::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::openapi::schemas::ErrorResponse),
    )
)]
pub async fn get_workspace_graph(
    Path((tid, wid)): Path<(Uuid, Uuid)>,
    Query(params): Query<GraphQueryParams>,
    Extension(ctx): Extension<TenantContext>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
    Extension(authz): Extension<AuthzService>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;

    let limit = effective_limit(params.limit);
    let fetch_limit = limit.saturating_add(1) as i64;

    let cursor_bounds = match params.cursor.as_deref() {
        None => None,
        Some(raw) => Some(decode_cursor(raw)?),
    };

    require_workspace_access_hidden(&conn, &authz, wid, auth_user.user_id).await?;

    let mut guard = conn.lock().await;

    let mut node_rows = if let Some((cursor_ts, cursor_id)) = cursor_bounds {
        sqlx::query_as::<_, GraphNodeRow>(
            "SELECT id, kind, label, properties, created_at
             FROM graph_nodes
             WHERE workspace_id = $1
               AND (created_at, id) > ($2, $3)
             ORDER BY created_at, id
             LIMIT $4",
        )
        .bind(wid)
        .bind(cursor_ts)
        .bind(cursor_id)
        .bind(fetch_limit)
        .fetch_all(&mut *guard)
        .await
    } else {
        sqlx::query_as::<_, GraphNodeRow>(
            "SELECT id, kind, label, properties, created_at
             FROM graph_nodes
             WHERE workspace_id = $1
             ORDER BY created_at, id
             LIMIT $2",
        )
        .bind(wid)
        .bind(fetch_limit)
        .fetch_all(&mut *guard)
        .await
    }
    .map_err(|e| ApiError::Internal(format!("load graph nodes: {e}")))?;

    let next_cursor = if node_rows.len() > limit as usize {
        // Drop the lookahead row; cursor is the last item actually returned
        // so the next page uses `(created_at, id) > (last_ts, last_id)`.
        node_rows.pop();
        Some(encode_cursor(
            node_rows.last().expect("limit > 0 implies non-empty page"),
        ))
    } else {
        None
    };

    let accessible = accessible_document_ids(&mut guard, &authz, wid, auth_user.user_id)
        .await
        .map_err(map_retrieval_error)?;

    let mut visible_nodes = Vec::with_capacity(node_rows.len());
    for row in node_rows {
        if node_visible_via_provenance(&mut guard, row.id, &accessible)
            .await
            .map_err(map_retrieval_error)?
        {
            visible_nodes.push(row);
        }
    }

    let node_ids: Vec<Uuid> = visible_nodes.iter().map(|n| n.id).collect();
    let edges = if node_ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query_as::<_, GraphEdgeRow>(
            "SELECT e.id, e.src_node_id, e.dst_node_id, e.kind, e.weight, e.properties, e.created_at
             FROM graph_edges e
             WHERE e.src_node_id = ANY($1) AND e.dst_node_id = ANY($1)
             ORDER BY e.created_at",
        )
        .bind(&node_ids)
        .fetch_all(&mut *guard)
        .await
        .map_err(|e| ApiError::Internal(format!("load graph edges: {e}")))?
    };

    drop(guard);

    Ok(Json(WorkspaceGraphResponse {
        nodes: visible_nodes
            .into_iter()
            .map(|row| crate::openapi::schemas::GraphNodeItem {
                id: row.id,
                kind: row.kind,
                label: row.label,
                properties: row.properties,
                created_at: row.created_at,
            })
            .collect(),
        edges: edges
            .into_iter()
            .map(|row| crate::openapi::schemas::GraphEdgeItem {
                id: row.id,
                src_node_id: row.src_node_id,
                dst_node_id: row.dst_node_id,
                kind: row.kind,
                weight: row.weight,
                properties: row.properties,
                created_at: row.created_at,
            })
            .collect(),
        next_cursor,
    }))
}
