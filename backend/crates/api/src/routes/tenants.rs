//! Tenant CRUD routes (T52 + T53).
//!
//! - `GET /tenants` and `POST /tenants` are **cross-tenant / pre-tenant**
//!   operations: they run before any `X-Tenant-Id` context exists, so they use
//!   [`AdminPool`] (bypasses RLS), mirroring `GET /users/me`. `POST` creates a
//!   tenant and auto-adds the creator as `owner`.
//! - `PATCH /tenants/{tid}` and `DELETE /tenants/{tid}` are tenant-scoped and
//!   owner-only. They run inside the `tenant_middleware` + `rls_middleware`
//!   chain and execute on the per-request [`SharedConnection`] so RLS is
//!   enforced.

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
use crate::pool::AdminPool;

/// Role assigned to the user who creates a tenant.
const ROLE_OWNER: &str = "owner";

#[derive(Serialize, sqlx::FromRow)]
struct TenantListRow {
    id: Uuid,
    name: String,
    created_at: chrono::DateTime<chrono::Utc>,
    role: String,
}

#[derive(Deserialize)]
pub struct CreateTenantBody {
    pub name: String,
}

#[derive(Deserialize)]
pub struct UpdateTenantBody {
    pub name: String,
}

/// `GET /tenants` — list every tenant the authenticated user belongs to.
///
/// Cross-tenant: uses [`AdminPool`] (bypasses RLS) because the listing spans
/// all of the user's memberships, not a single active tenant.
pub async fn list_tenants(
    Extension(auth_user): Extension<AuthUser>,
    Extension(AdminPool(pool)): Extension<AdminPool>,
) -> Result<impl IntoResponse, ApiError> {
    let rows = sqlx::query_as::<_, TenantListRow>(
        "SELECT t.id, t.name, t.created_at, tm.role
         FROM tenant_members tm
         JOIN tenants t ON t.id = tm.tenant_id
         WHERE tm.user_id = $1
         ORDER BY t.created_at",
    )
    .bind(auth_user.user_id)
    .fetch_all(&pool)
    .await
    .map_err(|e| ApiError::Internal(format!("db error: {e}")))?;

    Ok(Json(serde_json::json!({ "tenants": rows })))
}

/// `POST /tenants` — create a tenant and make the creator its `owner`.
///
/// Cross-tenant: uses [`AdminPool`]. The `tenants` table has `FORCE` RLS with
/// `WITH CHECK (id = gmrag_current_tenant())`, so the insert can only succeed
/// on a connection that bypasses RLS (no tenant context exists yet).
pub async fn create_tenant(
    Extension(auth_user): Extension<AuthUser>,
    Extension(AdminPool(pool)): Extension<AdminPool>,
    Json(body): Json<CreateTenantBody>,
) -> Result<impl IntoResponse, ApiError> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest("tenant name must not be empty".into()));
    }

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| ApiError::Internal(format!("db error: {e}")))?;

    let tenant_id: Uuid = sqlx::query_scalar("INSERT INTO tenants (name) VALUES ($1) RETURNING id")
        .bind(name)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| ApiError::Internal(format!("db error: {e}")))?;

    sqlx::query("INSERT INTO tenant_members (tenant_id, user_id, role) VALUES ($1, $2, $3)")
        .bind(tenant_id)
        .bind(auth_user.user_id)
        .bind(ROLE_OWNER)
        .execute(&mut *tx)
        .await
        .map_err(|e| ApiError::Internal(format!("db error: {e}")))?;

    tx.commit()
        .await
        .map_err(|e| ApiError::Internal(format!("db error: {e}")))?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": tenant_id,
            "name": name,
            "role": ROLE_OWNER,
        })),
    ))
}

/// `PATCH /tenants/{tid}` — rename a tenant (owner-only).
pub async fn update_tenant(
    Path(tid): Path<Uuid>,
    Extension(ctx): Extension<TenantContext>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
    Json(body): Json<UpdateTenantBody>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;

    let name = body.name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest("tenant name must not be empty".into()));
    }

    require_owner(&conn, auth_user.user_id).await?;

    let mut guard = conn.lock().await;
    let updated = sqlx::query("UPDATE tenants SET name = $1 WHERE id = $2")
        .bind(name)
        .bind(tid)
        .execute(&mut *guard)
        .await
        .map_err(|e| ApiError::Internal(format!("db error: {e}")))?;
    drop(guard);

    if updated.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(Json(serde_json::json!({ "id": tid, "name": name })))
}

/// `DELETE /tenants/{tid}` — delete a tenant and cascade its data (owner-only).
///
/// `ON DELETE CASCADE` on the child FKs removes members, workspaces, documents,
/// etc. Referential cascades are performed by the system and are not subject to
/// RLS policies.
pub async fn delete_tenant(
    Path(tid): Path<Uuid>,
    Extension(ctx): Extension<TenantContext>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;
    require_owner(&conn, auth_user.user_id).await?;

    let mut guard = conn.lock().await;
    let deleted = sqlx::query("DELETE FROM tenants WHERE id = $1")
        .bind(tid)
        .execute(&mut *guard)
        .await
        .map_err(|e| ApiError::Internal(format!("db error: {e}")))?;
    drop(guard);

    if deleted.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}

/// Guard: the `{tid}` path segment must equal the resolved [`TenantContext`]
/// (which is derived from the `X-Tenant-Id` header validated by the tenant
/// middleware). This prevents acting on a tenant other than the one the caller
/// authenticated against.
pub(crate) fn ensure_path_matches_context(
    tid: Uuid,
    ctx: &TenantContext,
) -> Result<(), ApiError> {
    if tid != ctx.0 {
        return Err(ApiError::BadRequest(
            "path tenant id does not match X-Tenant-Id".into(),
        ));
    }
    Ok(())
}

/// Authorisation guard: the caller must be an `owner` of the current tenant.
///
/// Reads `tenant_members.role` on the RLS-scoped [`SharedConnection`], so only
/// the current tenant's membership is visible.
pub(crate) async fn require_owner(
    conn: &SharedConnection,
    user_id: Uuid,
) -> Result<(), ApiError> {
    let mut guard = conn.lock().await;
    let role: Option<String> =
        sqlx::query_scalar("SELECT role FROM tenant_members WHERE user_id = $1")
            .bind(user_id)
            .fetch_optional(&mut *guard)
            .await
            .map_err(|e| ApiError::Internal(format!("db error: {e}")))?;
    drop(guard);

    match role.as_deref() {
        Some(ROLE_OWNER) => Ok(()),
        _ => Err(ApiError::Forbidden(
            "only a tenant owner may perform this action".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_mismatch_is_bad_request() {
        let ctx = TenantContext(Uuid::new_v4());
        let other = Uuid::new_v4();
        let err = ensure_path_matches_context(other, &ctx).unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(_)));
        assert_eq!(err.code(), "bad-request");
    }

    #[test]
    fn path_match_is_ok() {
        let id = Uuid::new_v4();
        let ctx = TenantContext(id);
        assert!(ensure_path_matches_context(id, &ctx).is_ok());
    }
}
