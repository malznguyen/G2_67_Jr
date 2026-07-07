//! Workspace authorization helpers.

use uuid::Uuid;

use crate::authz::{
    check_or_unavailable, user_obj, workspace_obj, AuthzService, CheckRequest, REL_ACCESSOR,
    REL_MANAGER,
};
use crate::error::ApiError;
use crate::middleware::rls::SharedConnection;

/// Require permission to manage a workspace in the current RLS tenant.
///
/// Management is granted to tenant owners, workspace owners, and workspace
/// admins. Tenant admins do not receive implicit workspace authority.
pub async fn require_workspace_manager(
    conn: &SharedConnection,
    authz: &AuthzService,
    workspace_id: Uuid,
    caller_id: Uuid,
) -> Result<(), ApiError> {
    if !workspace_exists(conn, workspace_id).await? {
        return Err(ApiError::NotFound);
    }
    let allowed = check_or_unavailable(
        authz,
        CheckRequest::new(
            user_obj(caller_id),
            REL_MANAGER,
            workspace_obj(workspace_id),
        ),
    )
    .await?;
    if allowed {
        return Ok(());
    }

    Err(ApiError::Forbidden(
        "workspace owner or admin required".into(),
    ))
}

/// Require permission to read or use a workspace in the current RLS tenant.
///
/// Access is granted to tenant owners and any direct workspace member. Tenant
/// admins do not receive implicit workspace authority.
pub async fn require_workspace_access(
    conn: &SharedConnection,
    authz: &AuthzService,
    workspace_id: Uuid,
    caller_id: Uuid,
) -> Result<(), ApiError> {
    if !workspace_exists(conn, workspace_id).await? {
        return Err(ApiError::NotFound);
    }
    let allowed = check_or_unavailable(
        authz,
        CheckRequest::new(
            user_obj(caller_id),
            REL_ACCESSOR,
            workspace_obj(workspace_id),
        ),
    )
    .await?;
    if allowed {
        return Ok(());
    }

    Err(ApiError::Forbidden("workspace access required".into()))
}

async fn workspace_exists(conn: &SharedConnection, workspace_id: Uuid) -> Result<bool, ApiError> {
    let mut guard = conn.lock().await;
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM workspaces
            WHERE id = $1
        )",
    )
    .bind(workspace_id)
    .fetch_one(&mut *guard)
    .await
    .map_err(|e| ApiError::Internal(format!("db error: {e}")))?;
    drop(guard);
    Ok(exists)
}

/// Require workspace read/use access, but hide same-tenant denied access as 404.
pub async fn require_workspace_access_hidden(
    conn: &SharedConnection,
    authz: &AuthzService,
    workspace_id: Uuid,
    caller_id: Uuid,
) -> Result<(), ApiError> {
    match require_workspace_access(conn, authz, workspace_id, caller_id).await {
        Err(ApiError::Forbidden(_)) => Err(ApiError::NotFound),
        other => other,
    }
}
