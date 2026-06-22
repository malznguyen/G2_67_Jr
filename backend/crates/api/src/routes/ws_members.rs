//! Workspace member management routes (T56).
//!
//! All routes are tenant-scoped: they run inside the
//! `tenant_middleware` + `rls_middleware` chain and execute on the per-request
//! [`SharedConnection`], so PostgreSQL RLS restricts every query to the current
//! tenant. The `{tid}` path segment is additionally guarded against the
//! resolved [`TenantContext`] via [`ensure_path_matches_context`].
//!
//! - `GET /tenants/{tid}/workspaces/{wid}/members` — list workspace members.
//! - `POST /tenants/{tid}/workspaces/{wid}/members` — add a member.
//! - `DELETE /tenants/{tid}/workspaces/{wid}/members/{user_id}` — remove a member.

use axum::extract::{Extension, Path};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::extractor::AuthUser;
use crate::auth::tenant::TenantContext;
use crate::error::ApiError;
use crate::middleware::rls::SharedConnection;
use crate::routes::tenants::ensure_path_matches_context;

const DEFAULT_MEMBER_ROLE: &str = "member";

#[derive(Serialize, sqlx::FromRow)]
struct WsMemberRow {
    user_id: Uuid,
    role: String,
    email: String,
    name: String,
}

#[derive(Deserialize)]
pub struct AddMemberBody {
    pub user_id: Uuid,
    pub role: Option<String>,
}

/// `GET /tenants/{tid}/workspaces/{wid}/members` — list members of a workspace.
pub async fn list_members(
    Path((tid, wid)): Path<(Uuid, Uuid)>,
    Extension(ctx): Extension<TenantContext>,
    Extension(conn): Extension<SharedConnection>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;

    let mut guard = conn.lock().await;
    let rows = sqlx::query_as::<_, WsMemberRow>(
        "SELECT wm.user_id, wm.role, u.email, u.name
         FROM workspace_members wm
         JOIN users u ON u.id = wm.user_id
         WHERE wm.workspace_id = $1
         ORDER BY u.email",
    )
    .bind(wid)
    .fetch_all(&mut *guard)
    .await
    .map_err(|e| ApiError::Internal(format!("db error: {e}")))?;
    drop(guard);

    Ok(Json(serde_json::json!({ "members": rows })))
}

/// `POST /tenants/{tid}/workspaces/{wid}/members` — add a member to a workspace.
///
/// `tenant_id` is denormalized on `workspace_members`; binding it to the
/// resolved tenant satisfies the RLS `USING`/`WITH CHECK` predicate.
pub async fn add_member(
    Path((tid, wid)): Path<(Uuid, Uuid)>,
    Extension(ctx): Extension<TenantContext>,
    Extension(_auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
    Json(body): Json<AddMemberBody>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;

    let role = body
        .role
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .unwrap_or(DEFAULT_MEMBER_ROLE)
        .to_string();

    let mut guard = conn.lock().await;
    sqlx::query(
        "INSERT INTO workspace_members (workspace_id, tenant_id, user_id, role)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(wid)
    .bind(tid)
    .bind(body.user_id)
    .bind(&role)
    .execute(&mut *guard)
    .await
    .map_err(|e| ApiError::Internal(format!("db error: {e}")))?;
    drop(guard);

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "workspace_id": wid,
            "user_id": body.user_id,
            "role": role,
        })),
    ))
}

/// `DELETE /tenants/{tid}/workspaces/{wid}/members/{user_id}` — remove a member.
pub async fn remove_member(
    Path((tid, wid, target_user_id)): Path<(Uuid, Uuid, Uuid)>,
    Extension(ctx): Extension<TenantContext>,
    Extension(conn): Extension<SharedConnection>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;

    let mut guard = conn.lock().await;
    let deleted =
        sqlx::query("DELETE FROM workspace_members WHERE workspace_id = $1 AND user_id = $2")
            .bind(wid)
            .bind(target_user_id)
            .execute(&mut *guard)
            .await
            .map_err(|e| ApiError::Internal(format!("db error: {e}")))?;
    drop(guard);

    if deleted.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}
