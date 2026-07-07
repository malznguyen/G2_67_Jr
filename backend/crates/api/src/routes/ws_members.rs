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
use crate::authz::{workspace_role_tuple, write_or_unavailable, AuthzService};
use crate::error::ApiError;
use crate::middleware::rls::SharedConnection;
use crate::roles::WorkspaceMemberRole;
use crate::routes::tenants::ensure_path_matches_context;
use crate::routes::workspace_auth::{require_workspace_access_hidden, require_workspace_manager};

const DEFAULT_MEMBER_ROLE: WorkspaceMemberRole = WorkspaceMemberRole::Member;

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

/// List members of a workspace.
#[utoipa::path(
    get,
    path = "/tenants/{tid}/workspaces/{wid}/members",
    operation_id = "list_workspace_members",
    tag = "WorkspaceMembers",
    params(
        ("tid" = Uuid, Path, description = "Tenant ID"),
        ("wid" = Uuid, Path, description = "Workspace ID"),
        ("X-Tenant-ID" = Uuid, Header, description = "Must match path tid"),
    ),
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Member list", body = crate::openapi::schemas::WorkspaceMembersResponse),
        (status = 400, description = "Bad request", body = crate::openapi::schemas::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::openapi::schemas::ErrorResponse),
        (status = 404, description = "Workspace not found or no access", body = crate::openapi::schemas::ErrorResponse),
        (status = 503, description = "Authorization unavailable", body = crate::openapi::schemas::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::openapi::schemas::ErrorResponse),
    )
)]
pub async fn list_members(
    Path((tid, wid)): Path<(Uuid, Uuid)>,
    Extension(ctx): Extension<TenantContext>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
    Extension(authz): Extension<AuthzService>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;
    require_workspace_access_hidden(&conn, &authz, wid, auth_user.user_id).await?;

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

/// Add a member to a workspace.
#[utoipa::path(
    post,
    path = "/tenants/{tid}/workspaces/{wid}/members",
    tag = "WorkspaceMembers",
    params(
        ("tid" = Uuid, Path, description = "Tenant ID"),
        ("wid" = Uuid, Path, description = "Workspace ID"),
        ("X-Tenant-ID" = Uuid, Header, description = "Must match path tid"),
    ),
    security(("bearer_auth" = [])),
    request_body = crate::openapi::schemas::AddWorkspaceMemberRequest,
    responses(
        (status = 201, description = "Member added", body = crate::openapi::schemas::AddWorkspaceMemberResponse),
        (status = 400, description = "Bad request", body = crate::openapi::schemas::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::openapi::schemas::ErrorResponse),
        (status = 403, description = "Forbidden — workspace manager only", body = crate::openapi::schemas::ErrorResponse),
        (status = 404, description = "Workspace not found", body = crate::openapi::schemas::ErrorResponse),
        (status = 503, description = "Authorization unavailable", body = crate::openapi::schemas::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::openapi::schemas::ErrorResponse),
    )
)]
pub async fn add_member(
    Path((tid, wid)): Path<(Uuid, Uuid)>,
    Extension(ctx): Extension<TenantContext>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
    Extension(authz): Extension<AuthzService>,
    Json(body): Json<AddMemberBody>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;

    let role = match body.role.as_deref() {
        None => DEFAULT_MEMBER_ROLE,
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                DEFAULT_MEMBER_ROLE
            } else {
                WorkspaceMemberRole::parse(trimmed).ok_or_else(|| {
                    ApiError::BadRequest(format!(
                        "invalid workspace role '{trimmed}'; must be one of: owner, admin, member"
                    ))
                })?
            }
        }
    };
    let role_str = role.as_str().to_string();

    require_workspace_manager(&conn, &authz, wid, auth_user.user_id).await?;

    let mut guard = conn.lock().await;
    let target_is_tenant_member: bool = sqlx::query_scalar(
        "SELECT EXISTS(
             SELECT 1 FROM tenant_members
             WHERE tenant_id = $1 AND user_id = $2
         )",
    )
    .bind(tid)
    .bind(body.user_id)
    .fetch_one(&mut *guard)
    .await
    .map_err(|e| ApiError::Internal(format!("db error: {e}")))?;

    if !target_is_tenant_member {
        drop(guard);
        return Err(ApiError::BadRequest(
            "target user must be a member of the tenant".into(),
        ));
    }

    sqlx::query(
        "INSERT INTO workspace_members (workspace_id, tenant_id, user_id, role)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(wid)
    .bind(tid)
    .bind(body.user_id)
    .bind(&role_str)
    .execute(&mut *guard)
    .await
    .map_err(|e| ApiError::Internal(format!("db error: {e}")))?;
    if let Err(e) = write_or_unavailable(
        &authz,
        vec![workspace_role_tuple(body.user_id, &role_str, wid)],
        Vec::new(),
    )
    .await
    {
        let _ =
            sqlx::query("DELETE FROM workspace_members WHERE workspace_id = $1 AND user_id = $2")
                .bind(wid)
                .bind(body.user_id)
                .execute(&mut *guard)
                .await;
        drop(guard);
        return Err(e);
    }
    drop(guard);

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "workspace_id": wid,
            "user_id": body.user_id,
            "role": role_str,
        })),
    ))
}

/// Remove a member from a workspace.
#[utoipa::path(
    delete,
    path = "/tenants/{tid}/workspaces/{wid}/members/{user_id}",
    operation_id = "remove_workspace_member",
    tag = "WorkspaceMembers",
    params(
        ("tid" = Uuid, Path, description = "Tenant ID"),
        ("wid" = Uuid, Path, description = "Workspace ID"),
        ("user_id" = Uuid, Path, description = "Member user ID"),
        ("X-Tenant-ID" = Uuid, Header, description = "Must match path tid"),
    ),
    security(("bearer_auth" = [])),
    responses(
        (status = 204, description = "Member removed"),
        (status = 400, description = "Bad request", body = crate::openapi::schemas::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::openapi::schemas::ErrorResponse),
        (status = 403, description = "Forbidden — workspace manager only", body = crate::openapi::schemas::ErrorResponse),
        (status = 404, description = "Member not found", body = crate::openapi::schemas::ErrorResponse),
        (status = 503, description = "Authorization unavailable", body = crate::openapi::schemas::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::openapi::schemas::ErrorResponse),
    )
)]
pub async fn remove_member(
    Path((tid, wid, target_user_id)): Path<(Uuid, Uuid, Uuid)>,
    Extension(ctx): Extension<TenantContext>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
    Extension(authz): Extension<AuthzService>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;
    require_workspace_manager(&conn, &authz, wid, auth_user.user_id).await?;

    let mut guard = conn.lock().await;
    let locked_members: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT user_id, role
         FROM workspace_members
         WHERE workspace_id = $1
         ORDER BY user_id
         FOR UPDATE",
    )
    .bind(wid)
    .fetch_all(&mut *guard)
    .await
    .map_err(|e| ApiError::Internal(format!("db error: {e}")))?;

    let target_role = match locked_members
        .iter()
        .find(|(user_id, _)| *user_id == target_user_id)
        .map(|(_, role)| role.as_str())
    {
        Some(role) => role,
        None => {
            drop(guard);
            return Err(ApiError::NotFound);
        }
    };

    if matches!(target_role, "owner" | "admin") {
        let privileged_count = locked_members
            .iter()
            .filter(|(_, role)| matches!(role.as_str(), "owner" | "admin"))
            .count();

        if privileged_count <= 1 {
            drop(guard);
            return Err(ApiError::BadRequest(
                "cannot remove the last workspace owner or admin".into(),
            ));
        }
    }

    write_or_unavailable(
        &authz,
        Vec::new(),
        vec![workspace_role_tuple(target_user_id, target_role, wid)],
    )
    .await?;

    sqlx::query("DELETE FROM workspace_members WHERE workspace_id = $1 AND user_id = $2")
        .bind(wid)
        .bind(target_user_id)
        .execute(&mut *guard)
        .await
        .map_err(|e| ApiError::Internal(format!("db error: {e}")))?;
    drop(guard);

    Ok(StatusCode::NO_CONTENT)
}
