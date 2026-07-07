//! Tenant member management routes (T54).
//!
//! All routes are tenant-scoped: they run inside the
//! `tenant_middleware` + `rls_middleware` chain and execute on the per-request
//! [`SharedConnection`], so PostgreSQL RLS restricts every query to the current
//! tenant.
//!
//! - `GET /tenants/{tid}/members` — list members (any member of the tenant).
//! - `POST /tenants/{tid}/members` — invite by email (owner-only): inserts a
//!   pending row into `invitations`; membership is created later when the
//!   invite is accepted.
//! - `DELETE /tenants/{tid}/members/{user_id}` — remove a member (owner-only),
//!   refusing to remove the tenant's last `owner`.

use axum::extract::{Extension, Path};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::extractor::AuthUser;
use crate::auth::tenant::TenantContext;
use crate::authz::{
    tenant_role_tuple, write_or_unavailable, AuthzService, REL_ADMIN, REL_MEMBER, REL_OWNER,
};
use crate::error::ApiError;
use crate::middleware::rls::SharedConnection;
use crate::roles::TenantMemberRole;
use crate::routes::tenants::{ensure_path_matches_context, require_owner};

const ROLE_OWNER: &str = "owner";
const DEFAULT_INVITE_ROLE: TenantMemberRole = TenantMemberRole::Member;

#[derive(Serialize, sqlx::FromRow)]
struct MemberRow {
    user_id: Uuid,
    role: String,
    email: String,
    name: String,
}

#[derive(Deserialize)]
pub struct InviteBody {
    pub email: String,
    pub role: Option<String>,
}

#[derive(Serialize, sqlx::FromRow)]
struct InvitationRow {
    id: Uuid,
    token: Uuid,
    status: String,
}

/// List members of the current tenant.
#[utoipa::path(
    get,
    path = "/tenants/{tid}/members",
    operation_id = "list_tenant_members",
    tag = "TenantMembers",
    params(
        ("tid" = Uuid, Path, description = "Tenant ID"),
        ("X-Tenant-ID" = Uuid, Header, description = "Must match path tid"),
    ),
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Member list", body = crate::openapi::schemas::TenantMembersResponse),
        (status = 400, description = "Bad request", body = crate::openapi::schemas::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::openapi::schemas::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::openapi::schemas::ErrorResponse),
    )
)]
pub async fn list_members(
    Path(tid): Path<Uuid>,
    Extension(ctx): Extension<TenantContext>,
    Extension(conn): Extension<SharedConnection>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;

    let mut guard = conn.lock().await;
    let rows = sqlx::query_as::<_, MemberRow>(
        "SELECT tm.user_id, tm.role, u.email, u.name
         FROM tenant_members tm
         JOIN users u ON u.id = tm.user_id
         ORDER BY u.email",
    )
    .fetch_all(&mut *guard)
    .await
    .map_err(|e| ApiError::Internal(format!("db error: {e}")))?;
    drop(guard);

    Ok(Json(serde_json::json!({ "members": rows })))
}

/// Invite a user to the tenant by email (owner-only).
#[utoipa::path(
    post,
    path = "/tenants/{tid}/members",
    tag = "TenantMembers",
    params(
        ("tid" = Uuid, Path, description = "Tenant ID"),
        ("X-Tenant-ID" = Uuid, Header, description = "Must match path tid"),
    ),
    security(("bearer_auth" = [])),
    request_body = crate::openapi::schemas::InviteMemberRequest,
    responses(
        (status = 201, description = "Invitation created", body = crate::openapi::schemas::InviteMemberResponse),
        (status = 400, description = "Bad request", body = crate::openapi::schemas::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::openapi::schemas::ErrorResponse),
        (status = 403, description = "Forbidden — owner only", body = crate::openapi::schemas::ErrorResponse),
        (status = 503, description = "Authorization unavailable", body = crate::openapi::schemas::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::openapi::schemas::ErrorResponse),
    )
)]
pub async fn invite_member(
    Path(tid): Path<Uuid>,
    Extension(ctx): Extension<TenantContext>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
    Extension(authz): Extension<AuthzService>,
    Json(body): Json<InviteBody>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;

    let email = body.email.trim();
    if email.is_empty() {
        return Err(ApiError::BadRequest(
            "invite email must not be empty".into(),
        ));
    }
    let role = match body.role.as_deref() {
        None => DEFAULT_INVITE_ROLE,
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                DEFAULT_INVITE_ROLE
            } else {
                TenantMemberRole::parse(trimmed).ok_or_else(|| {
                    ApiError::BadRequest(format!(
                        "invalid tenant role '{trimmed}'; must be one of: owner, admin, member"
                    ))
                })?
            }
        }
    };
    let role_str = role.as_str().to_string();

    require_owner(&authz, tid, auth_user.user_id).await?;

    let mut guard = conn.lock().await;
    let invitation = sqlx::query_as::<_, InvitationRow>(
        "INSERT INTO invitations (tenant_id, email, role, invited_by)
         VALUES ($1, $2, $3, $4)
         RETURNING id, token, status",
    )
    .bind(tid)
    .bind(email)
    .bind(&role_str)
    .bind(auth_user.user_id)
    .fetch_one(&mut *guard)
    .await
    .map_err(|e| ApiError::Internal(format!("db error: {e}")))?;
    drop(guard);

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": invitation.id,
            "email": email,
            "role": role_str,
            "token": invitation.token,
            "status": invitation.status,
        })),
    ))
}

/// Remove a tenant member (owner-only).
#[utoipa::path(
    delete,
    path = "/tenants/{tid}/members/{user_id}",
    operation_id = "remove_tenant_member",
    tag = "TenantMembers",
    params(
        ("tid" = Uuid, Path, description = "Tenant ID"),
        ("user_id" = Uuid, Path, description = "Member user ID"),
        ("X-Tenant-ID" = Uuid, Header, description = "Must match path tid"),
    ),
    security(("bearer_auth" = [])),
    responses(
        (status = 204, description = "Member removed"),
        (status = 400, description = "Cannot remove last owner", body = crate::openapi::schemas::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::openapi::schemas::ErrorResponse),
        (status = 403, description = "Forbidden — owner only", body = crate::openapi::schemas::ErrorResponse),
        (status = 404, description = "Member not found", body = crate::openapi::schemas::ErrorResponse),
        (status = 503, description = "Authorization unavailable", body = crate::openapi::schemas::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::openapi::schemas::ErrorResponse),
    )
)]
pub async fn remove_member(
    Path((tid, target_user_id)): Path<(Uuid, Uuid)>,
    Extension(ctx): Extension<TenantContext>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
    Extension(authz): Extension<AuthzService>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;
    require_owner(&authz, tid, auth_user.user_id).await?;

    let mut guard = conn.lock().await;

    let target_role: Option<String> =
        sqlx::query_scalar("SELECT role FROM tenant_members WHERE user_id = $1")
            .bind(target_user_id)
            .fetch_optional(&mut *guard)
            .await
            .map_err(|e| ApiError::Internal(format!("db error: {e}")))?;

    let target_role = match target_role {
        Some(r) => r,
        None => {
            drop(guard);
            return Err(ApiError::NotFound);
        }
    };

    if target_role == ROLE_OWNER {
        let owner_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM tenant_members WHERE role = $1")
                .bind(ROLE_OWNER)
                .fetch_one(&mut *guard)
                .await
                .map_err(|e| ApiError::Internal(format!("db error: {e}")))?;
        if owner_count <= 1 {
            drop(guard);
            return Err(ApiError::BadRequest(
                "cannot remove the last owner of the tenant".into(),
            ));
        }
    }

    write_or_unavailable(
        &authz,
        Vec::new(),
        vec![
            tenant_role_tuple(target_user_id, REL_OWNER, tid),
            tenant_role_tuple(target_user_id, REL_ADMIN, tid),
            tenant_role_tuple(target_user_id, REL_MEMBER, tid),
        ],
    )
    .await?;

    sqlx::query("DELETE FROM tenant_members WHERE user_id = $1")
        .bind(target_user_id)
        .execute(&mut *guard)
        .await
        .map_err(|e| ApiError::Internal(format!("db error: {e}")))?;
    drop(guard);

    Ok(StatusCode::NO_CONTENT)
}
