//! Workspace knowledge-graph read API (T63).

use axum::extract::{Extension, Path};
use axum::response::IntoResponse;
use axum::Json;
use serde::Serialize;
use uuid::Uuid;

use crate::auth::extractor::AuthUser;
use crate::auth::tenant::TenantContext;
use crate::error::ApiError;
use crate::middleware::rls::SharedConnection;
use crate::rbac::check::check_relation;
use crate::rbac::model::{ObjectRef, Principal, Relation, NS_WORKSPACE};
use crate::routes::tenants::ensure_path_matches_context;

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

/// `GET /tenants/{tid}/workspaces/{wid}/graph` — full workspace graph (T63).
///
/// Authorization: workspace membership via `check_relation(workspace, member, user)`.
/// The `workspace#viewer` relation is undefined in the ReBAC model; `member` is the
/// correct gate for workspace-scoped resources. Missing row or denied check → `404`.
pub async fn get_workspace_graph(
    Path((tid, wid)): Path<(Uuid, Uuid)>,
    Extension(ctx): Extension<TenantContext>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;

    let mut guard = conn.lock().await;

    let workspace_exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workspaces WHERE id = $1)")
        .bind(wid)
        .fetch_one(&mut *guard)
        .await
        .map_err(|e| ApiError::Internal(format!("load workspace: {e}")))?;
    if !workspace_exists {
        drop(guard);
        return Err(ApiError::NotFound);
    }

    let is_member = check_relation(
        &mut guard,
        &ObjectRef::new(NS_WORKSPACE, wid),
        Relation::Member,
        Principal::User(auth_user.user_id),
    )
    .await
    .map_err(|e| ApiError::Internal(format!("member check: {e}")))?;
    if !is_member {
        drop(guard);
        return Err(ApiError::NotFound);
    }

    let nodes = sqlx::query_as::<_, GraphNodeRow>(
        "SELECT id, kind, label, properties, created_at
         FROM graph_nodes
         WHERE workspace_id = $1
         ORDER BY created_at",
    )
    .bind(wid)
    .fetch_all(&mut *guard)
    .await
    .map_err(|e| ApiError::Internal(format!("load graph nodes: {e}")))?;

    let edges = sqlx::query_as::<_, GraphEdgeRow>(
        "SELECT e.id, e.src_node_id, e.dst_node_id, e.kind, e.weight, e.properties, e.created_at
         FROM graph_edges e
         JOIN graph_nodes s ON s.id = e.src_node_id AND s.workspace_id = $1
         JOIN graph_nodes d ON d.id = e.dst_node_id AND d.workspace_id = $1
         ORDER BY e.created_at",
    )
    .bind(wid)
    .fetch_all(&mut *guard)
    .await
    .map_err(|e| ApiError::Internal(format!("load graph edges: {e}")))?;

    drop(guard);

    Ok(Json(serde_json::json!({ "nodes": nodes, "edges": edges })))
}
