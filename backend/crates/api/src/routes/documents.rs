//! Document list route (T57).
//!
//! Tenant-scoped: runs inside the `tenant_middleware` + `rls_middleware` chain
//! and executes on the per-request [`SharedConnection`], so PostgreSQL RLS
//! confines every query to the current tenant. On top of tenant isolation, the
//! list applies a per-user visibility + ACL filter:
//!
//! A document is returned iff
//!   - `visibility = 'shared'`, OR
//!   - `owner_id = current_user`, OR
//!   - it belongs to a workspace the current user is a member of.
//!
//! An optional `workspace_id` query parameter further narrows the result.

use axum::extract::{Extension, Path, Query};
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::extractor::AuthUser;
use crate::auth::tenant::TenantContext;
use crate::error::ApiError;
use crate::middleware::rls::SharedConnection;
use crate::routes::tenants::ensure_path_matches_context;

/// Marks a document as readable by anyone in the tenant.
const VISIBILITY_SHARED: &str = "shared";

#[derive(Serialize, sqlx::FromRow)]
struct DocumentRow {
    id: Uuid,
    title: String,
    visibility: String,
    owner_id: Uuid,
    workspace_id: Option<Uuid>,
    status: String,
    created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Deserialize)]
pub struct DocListParams {
    pub workspace_id: Option<Uuid>,
}

/// `GET /tenants/{tid}/documents` — list documents visible to the caller.
///
/// RLS already restricts rows to the current tenant; the `WHERE` clause then
/// applies the visibility + ACL predicate. `$2` (the optional `workspace_id`)
/// is cast to `uuid` so the `IS NULL` short-circuit works when omitted.
pub async fn list_documents(
    Path(tid): Path<Uuid>,
    Extension(ctx): Extension<TenantContext>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
    Query(params): Query<DocListParams>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;

    let mut guard = conn.lock().await;
    let rows = sqlx::query_as::<_, DocumentRow>(
        "SELECT id, title, visibility, owner_id, workspace_id, status, created_at
         FROM documents
         WHERE (
                 visibility = $1
                 OR owner_id = $2
                 OR (workspace_id IS NOT NULL AND EXISTS (
                       SELECT 1 FROM workspace_members wm
                       WHERE wm.workspace_id = documents.workspace_id
                         AND wm.user_id = $2))
               )
           AND ($3::uuid IS NULL OR workspace_id = $3)
         ORDER BY created_at DESC",
    )
    .bind(VISIBILITY_SHARED)
    .bind(auth_user.user_id)
    .bind(params.workspace_id)
    .fetch_all(&mut *guard)
    .await
    .map_err(|e| ApiError::Internal(format!("db error: {e}")))?;
    drop(guard);

    Ok(Json(serde_json::json!({ "documents": rows })))
}
