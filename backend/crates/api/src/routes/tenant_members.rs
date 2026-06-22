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
use crate::error::ApiError;
use crate::middleware::rls::SharedConnection;
use crate::routes::tenants::{ensure_path_matches_context, require_owner};

const ROLE_OWNER: &str = "owner";
const DEFAULT_INVITE_ROLE: &str = "member";

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

/// `GET /tenants/{tid}/members` — list members of the current tenant.
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

/// `POST /tenants/{tid}/members` — invite a user to the tenant by email.
///
/// Owner-only. Creates a pending `invitations` row. RLS `WITH CHECK
/// (tenant_id = gmrag_current_tenant())` is satisfied because `tid` equals the
/// resolved tenant context.
pub async fn invite_member(
    Path(tid): Path<Uuid>,
    Extension(ctx): Extension<TenantContext>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
    Json(body): Json<InviteBody>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;

    let email = body.email.trim();
    if email.is_empty() {
        return Err(ApiError::BadRequest("invite email must not be empty".into()));
    }
    let role = body
        .role
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .unwrap_or(DEFAULT_INVITE_ROLE)
        .to_string();

    require_owner(&conn, auth_user.user_id).await?;

    let mut guard = conn.lock().await;
    let invitation = sqlx::query_as::<_, InvitationRow>(
        "INSERT INTO invitations (tenant_id, email, role, invited_by)
         VALUES ($1, $2, $3, $4)
         RETURNING id, token, status",
    )
    .bind(tid)
    .bind(email)
    .bind(&role)
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
            "role": role,
            "token": invitation.token,
            "status": invitation.status,
        })),
    ))
}

/// `DELETE /tenants/{tid}/members/{user_id}` — remove a member (owner-only).
///
/// Refuses to remove the tenant's last remaining `owner` so the tenant can
/// never become ownerless.
pub async fn remove_member(
    Path((tid, target_user_id)): Path<(Uuid, Uuid)>,
    Extension(ctx): Extension<TenantContext>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;
    require_owner(&conn, auth_user.user_id).await?;

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

    sqlx::query("DELETE FROM tenant_members WHERE user_id = $1")
        .bind(target_user_id)
        .execute(&mut *guard)
        .await
        .map_err(|e| ApiError::Internal(format!("db error: {e}")))?;
    drop(guard);

    Ok(StatusCode::NO_CONTENT)
}
