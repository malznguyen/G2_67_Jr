//! Metering and audit read routes (T69).
//!
//! Owner-only GET endpoints for usage aggregates, tenant quotas, and audit logs.

use axum::extract::{Extension, Path};
use axum::response::IntoResponse;
use axum::Json;
use serde::Serialize;
use uuid::Uuid;

use crate::auth::extractor::AuthUser;
use crate::auth::tenant::TenantContext;
use crate::error::ApiError;
use crate::middleware::rls::SharedConnection;
use crate::routes::tenants::{ensure_path_matches_context, require_owner};

const DEFAULT_MAX_DOCUMENTS: i32 = 100;
const DEFAULT_MAX_WORKSPACES: i32 = 10;
const DEFAULT_MAX_STORAGE_BYTES: i64 = 10_737_418_240;
const DEFAULT_MAX_MEMBERS: i32 = 50;
const AUDIT_LOG_LIMIT: i64 = 100;

#[derive(Serialize, sqlx::FromRow)]
struct UsageMetricRow {
    metric: String,
    total: i64,
}

#[derive(Serialize, sqlx::FromRow)]
struct QuotaRow {
    max_documents: i32,
    max_workspaces: i32,
    max_storage_bytes: i64,
    max_members: i32,
    updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize)]
struct QuotaResponse {
    configured: bool,
    max_documents: i32,
    max_workspaces: i32,
    max_storage_bytes: i64,
    max_members: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Serialize, sqlx::FromRow)]
struct AuditLogRow {
    id: Uuid,
    actor_id: Option<Uuid>,
    action: String,
    resource_type: Option<String>,
    resource_id: Option<Uuid>,
    metadata: Option<serde_json::Value>,
    created_at: chrono::DateTime<chrono::Utc>,
}

/// Aggregate usage by metric (owner-only).
#[utoipa::path(
    get,
    path = "/tenants/{tid}/metering/usage",
    tag = "Metering",
    params(
        ("tid" = Uuid, Path, description = "Tenant ID"),
        ("X-Tenant-Id" = Uuid, Header, description = "Must match path tid"),
    ),
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Usage aggregates", body = crate::openapi::schemas::UsageResponse),
        (status = 400, description = "Bad request", body = crate::openapi::schemas::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::openapi::schemas::ErrorResponse),
        (status = 403, description = "Forbidden — owner only", body = crate::openapi::schemas::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::openapi::schemas::ErrorResponse),
    )
)]
pub async fn get_usage(
    Path(tid): Path<Uuid>,
    Extension(ctx): Extension<TenantContext>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;
    require_owner(&conn, auth_user.user_id).await?;

    let mut guard = conn.lock().await;
    let rows = sqlx::query_as::<_, UsageMetricRow>(
        r#"
        SELECT metric, COALESCE(SUM(delta), 0)::bigint AS total
        FROM usage_events
        WHERE tenant_id = $1
        GROUP BY metric
        ORDER BY metric
        "#,
    )
    .bind(tid)
    .fetch_all(&mut *guard)
    .await
    .map_err(|e| ApiError::Internal(format!("db error: {e}")))?;
    drop(guard);

    Ok(Json(serde_json::json!({ "usage": rows })))
}

/// Read tenant quota limits (owner-only).
#[utoipa::path(
    get,
    path = "/tenants/{tid}/quotas",
    tag = "Metering",
    params(
        ("tid" = Uuid, Path, description = "Tenant ID"),
        ("X-Tenant-Id" = Uuid, Header, description = "Must match path tid"),
    ),
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Quota limits", body = crate::openapi::schemas::QuotaResponse),
        (status = 400, description = "Bad request", body = crate::openapi::schemas::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::openapi::schemas::ErrorResponse),
        (status = 403, description = "Forbidden — owner only", body = crate::openapi::schemas::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::openapi::schemas::ErrorResponse),
    )
)]
pub async fn get_quotas(
    Path(tid): Path<Uuid>,
    Extension(ctx): Extension<TenantContext>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;
    require_owner(&conn, auth_user.user_id).await?;

    let mut guard = conn.lock().await;
    let row = sqlx::query_as::<_, QuotaRow>(
        r#"
        SELECT max_documents, max_workspaces, max_storage_bytes, max_members, updated_at
        FROM tenant_quotas
        WHERE tenant_id = $1
        "#,
    )
    .bind(tid)
    .fetch_optional(&mut *guard)
    .await
    .map_err(|e| ApiError::Internal(format!("db error: {e}")))?;
    drop(guard);

    let response = match row {
        Some(row) => QuotaResponse {
            configured: true,
            max_documents: row.max_documents,
            max_workspaces: row.max_workspaces,
            max_storage_bytes: row.max_storage_bytes,
            max_members: row.max_members,
            updated_at: Some(row.updated_at),
        },
        None => QuotaResponse {
            configured: false,
            max_documents: DEFAULT_MAX_DOCUMENTS,
            max_workspaces: DEFAULT_MAX_WORKSPACES,
            max_storage_bytes: DEFAULT_MAX_STORAGE_BYTES,
            max_members: DEFAULT_MAX_MEMBERS,
            updated_at: None,
        },
    };

    Ok(Json(response))
}

/// Recent audit log entries, newest first (max 100, owner-only).
#[utoipa::path(
    get,
    path = "/tenants/{tid}/audit_logs",
    tag = "Metering",
    params(
        ("tid" = Uuid, Path, description = "Tenant ID"),
        ("X-Tenant-Id" = Uuid, Header, description = "Must match path tid"),
    ),
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Audit logs (capped at 100)", body = crate::openapi::schemas::AuditLogsResponse),
        (status = 400, description = "Bad request", body = crate::openapi::schemas::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::openapi::schemas::ErrorResponse),
        (status = 403, description = "Forbidden — owner only", body = crate::openapi::schemas::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::openapi::schemas::ErrorResponse),
    )
)]
pub async fn get_audit_logs(
    Path(tid): Path<Uuid>,
    Extension(ctx): Extension<TenantContext>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;
    require_owner(&conn, auth_user.user_id).await?;

    let mut guard = conn.lock().await;
    let rows = sqlx::query_as::<_, AuditLogRow>(
        r#"
        SELECT id, actor_id, action, resource_type, resource_id, metadata, created_at
        FROM audit_log
        WHERE tenant_id = $1
        ORDER BY created_at DESC
        LIMIT $2
        "#,
    )
    .bind(tid)
    .bind(AUDIT_LOG_LIMIT)
    .fetch_all(&mut *guard)
    .await
    .map_err(|e| ApiError::Internal(format!("db error: {e}")))?;
    drop(guard);

    Ok(Json(serde_json::json!({ "logs": rows })))
}
