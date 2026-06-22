//! Workspace CRUD routes (T55).
//!
//! All routes are tenant-scoped: they run inside the
//! `tenant_middleware` + `rls_middleware` chain and execute on the per-request
//! [`SharedConnection`], so PostgreSQL RLS restricts every query to the current
//! tenant. The `{tid}` path segment is additionally guarded against the
//! resolved [`TenantContext`] via [`ensure_path_matches_context`].
//!
//! - `GET /tenants/{tid}/workspaces` — list workspaces in the current tenant.
//! - `POST /tenants/{tid}/workspaces` — create a workspace (`created_by` = caller).
//! - `PATCH /tenants/{tid}/workspaces/{wid}` — rename a workspace.
//! - `DELETE /tenants/{tid}/workspaces/{wid}` — delete a workspace (cascade).

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

#[derive(Serialize, sqlx::FromRow)]
struct WorkspaceRow {
    id: Uuid,
    name: String,
    slug: String,
    created_by: Uuid,
    created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Deserialize)]
pub struct CreateWorkspaceBody {
    pub name: String,
    pub slug: String,
}

#[derive(Deserialize)]
pub struct UpdateWorkspaceBody {
    pub name: String,
    pub slug: String,
}

/// `GET /tenants/{tid}/workspaces` — list workspaces of the current tenant.
pub async fn list_workspaces(
    Path(tid): Path<Uuid>,
    Extension(ctx): Extension<TenantContext>,
    Extension(conn): Extension<SharedConnection>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;

    let mut guard = conn.lock().await;
    let rows = sqlx::query_as::<_, WorkspaceRow>(
        "SELECT id, name, slug, created_by, created_at
         FROM workspaces
         ORDER BY created_at",
    )
    .fetch_all(&mut *guard)
    .await
    .map_err(|e| ApiError::Internal(format!("db error: {e}")))?;
    drop(guard);

    Ok(Json(serde_json::json!({ "workspaces": rows })))
}

/// `POST /tenants/{tid}/workspaces` — create a workspace.
///
/// `created_by` is the authenticated caller. RLS `USING (tenant_id =
/// gmrag_current_tenant())` doubles as the `WITH CHECK` for the insert, so
/// binding `tenant_id = tid` (which equals the resolved context) succeeds.
pub async fn create_workspace(
    Path(tid): Path<Uuid>,
    Extension(ctx): Extension<TenantContext>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
    Json(body): Json<CreateWorkspaceBody>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;

    let name = body.name.trim();
    let slug = body.slug.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest("workspace name must not be empty".into()));
    }
    if slug.is_empty() {
        return Err(ApiError::BadRequest("workspace slug must not be empty".into()));
    }

    let mut guard = conn.lock().await;
    let row = sqlx::query_as::<_, WorkspaceRow>(
        "INSERT INTO workspaces (tenant_id, name, slug, created_by)
         VALUES ($1, $2, $3, $4)
         RETURNING id, name, slug, created_by, created_at",
    )
    .bind(tid)
    .bind(name)
    .bind(slug)
    .bind(auth_user.user_id)
    .fetch_one(&mut *guard)
    .await
    .map_err(|e| ApiError::Internal(format!("db error: {e}")))?;
    drop(guard);

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": row.id,
            "name": row.name,
            "slug": row.slug,
            "created_by": row.created_by,
            "created_at": row.created_at,
        })),
    ))
}

/// `PATCH /tenants/{tid}/workspaces/{wid}` — rename a workspace.
pub async fn update_workspace(
    Path((tid, wid)): Path<(Uuid, Uuid)>,
    Extension(ctx): Extension<TenantContext>,
    Extension(conn): Extension<SharedConnection>,
    Json(body): Json<UpdateWorkspaceBody>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;

    let name = body.name.trim();
    let slug = body.slug.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest("workspace name must not be empty".into()));
    }
    if slug.is_empty() {
        return Err(ApiError::BadRequest("workspace slug must not be empty".into()));
    }

    let mut guard = conn.lock().await;
    let updated = sqlx::query("UPDATE workspaces SET name = $1, slug = $2 WHERE id = $3")
        .bind(name)
        .bind(slug)
        .bind(wid)
        .execute(&mut *guard)
        .await
        .map_err(|e| ApiError::Internal(format!("db error: {e}")))?;
    drop(guard);

    if updated.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(Json(serde_json::json!({
        "id": wid,
        "name": name,
        "slug": slug,
    })))
}

/// `DELETE /tenants/{tid}/workspaces/{wid}` — delete a workspace.
///
/// `ON DELETE CASCADE` on child FKs (workspace_members, documents) removes
/// dependent rows. Referential cascades are not subject to RLS policies.
pub async fn delete_workspace(
    Path((tid, wid)): Path<(Uuid, Uuid)>,
    Extension(ctx): Extension<TenantContext>,
    Extension(conn): Extension<SharedConnection>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;

    let mut guard = conn.lock().await;
    let deleted = sqlx::query("DELETE FROM workspaces WHERE id = $1")
        .bind(wid)
        .execute(&mut *guard)
        .await
        .map_err(|e| ApiError::Internal(format!("db error: {e}")))?;
    drop(guard);

    if deleted.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}
