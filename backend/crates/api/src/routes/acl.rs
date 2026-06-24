//! ACL grant routes (T67) — manage ReBAC relation tuples on `resource_acl`.
//!
//! Tenant-scoped: every handler runs inside the `tenant_middleware` +
//! `rls_middleware` chain on the per-request [`SharedConnection`], so RLS
//! confines all queries to the current tenant. On top of tenant isolation:
//!
//! - `GET /tenants/{tid}/acl?resource_type=&resource_id=` — list the grants on
//!   a resource. The caller must be able to **view** the resource.
//! - `POST /tenants/{tid}/acl` — create a grant (share). **Owner-only**: per
//!   the ResourceBAC matrix only an owner may share, so editors/viewers cannot
//!   escalate by re-sharing.
//! - `DELETE /tenants/{tid}/acl/{grant_id}` — revoke a grant. **Owner-only**.
//!
//! Every create/revoke writes an `audit_log` row (`acl.grant` / `acl.revoke`).
//! Only `editor` / `viewer` are shareable; `owner` is the resource's own owner
//! column and `member` is derived from `workspace_members`, so neither is
//! grantable here.

use axum::extract::{Extension, Path, Query};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::extractor::AuthUser;
use crate::auth::tenant::TenantContext;
use crate::error::ApiError;
use crate::middleware::rls::SharedConnection;
use crate::rbac::check::check_relation;
use crate::rbac::model::{ObjectRef, Principal, Relation, NS_CHAT_SESSION, NS_DOCUMENT};
use crate::routes::tenants::ensure_path_matches_context;

/// Namespaces on which sharing is supported.
fn is_shareable_namespace(ns: &str) -> bool {
    matches!(ns, NS_DOCUMENT | NS_CHAT_SESSION)
}

#[derive(Deserialize)]
pub struct AclListParams {
    pub resource_type: String,
    pub resource_id: Uuid,
}

#[derive(Deserialize)]
pub struct CreateGrantBody {
    pub resource_type: String,
    pub resource_id: Uuid,
    pub principal_type: String,
    pub principal_id: Uuid,
    pub relation: String,
}

#[derive(Serialize, sqlx::FromRow)]
struct GrantRow {
    id: Uuid,
    principal_type: String,
    principal_id: Uuid,
    #[sqlx(rename = "permission")]
    relation: String,
    created_at: chrono::DateTime<chrono::Utc>,
}

/// Validate the requested namespace + relation and build the typed objects.
fn parse_grant(
    resource_type: &str,
    resource_id: Uuid,
    principal_type: &str,
    principal_id: Uuid,
    relation: &str,
) -> Result<(ObjectRef, Principal, Relation), ApiError> {
    if !is_shareable_namespace(resource_type) {
        return Err(ApiError::BadRequest(format!(
            "resource_type '{resource_type}' is not shareable"
        )));
    }
    let relation = Relation::parse(relation)
        .filter(|r| r.is_grantable())
        .ok_or_else(|| {
            ApiError::BadRequest(format!(
                "relation '{relation}' is not grantable (use 'editor' or 'viewer')"
            ))
        })?;
    if relation == Relation::Owner {
        return Err(ApiError::BadRequest(
            "ownership is implicit and cannot be granted".into(),
        ));
    }
    let principal = Principal::from_parts(principal_type, principal_id).ok_or_else(|| {
        ApiError::BadRequest(format!("principal_type '{principal_type}' is invalid"))
    })?;
    Ok((ObjectRef::new(resource_type, resource_id), principal, relation))
}

/// Owner-guard: the caller must hold the `owner` relation on the object.
///
/// A missing/cross-tenant resource is invisible under RLS, so the owner check
/// returns `false` → `403` (we do not distinguish "not found" from "not
/// yours", to avoid leaking the resource's existence).
async fn require_owner(
    guard: &mut sqlx::PgConnection,
    object: &ObjectRef,
    user_id: Uuid,
) -> Result<(), ApiError> {
    let is_owner = check_relation(guard, object, Relation::Owner, Principal::User(user_id))
        .await
        .map_err(|e| ApiError::Internal(format!("acl owner check: {e}")))?;
    if is_owner {
        Ok(())
    } else {
        Err(ApiError::Forbidden(
            "only the resource owner may manage sharing".into(),
        ))
    }
}

/// List ACL grants on a resource (viewer-gated).
#[utoipa::path(
    get,
    path = "/tenants/{tid}/acl",
    tag = "ACL",
    params(
        ("tid" = Uuid, Path, description = "Tenant ID"),
        ("X-Tenant-Id" = Uuid, Header, description = "Must match path tid"),
        ("resource_type" = String, Query, description = "document or chat_session"),
        ("resource_id" = Uuid, Query, description = "Resource UUID"),
    ),
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Grant list", body = crate::openapi::schemas::GrantsResponse),
        (status = 400, description = "Bad request", body = crate::openapi::schemas::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::openapi::schemas::ErrorResponse),
        (status = 404, description = "Resource not found or no viewer access", body = crate::openapi::schemas::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::openapi::schemas::ErrorResponse),
    )
)]
pub async fn list_grants(
    Path(tid): Path<Uuid>,
    Extension(ctx): Extension<TenantContext>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
    Query(params): Query<AclListParams>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;
    if !is_shareable_namespace(&params.resource_type) {
        return Err(ApiError::BadRequest(format!(
            "resource_type '{}' is not shareable",
            params.resource_type
        )));
    }
    let object = ObjectRef::new(params.resource_type.clone(), params.resource_id);

    let mut guard = conn.lock().await;
    let can_view = check_relation(
        &mut guard,
        &object,
        Relation::Viewer,
        Principal::User(auth_user.user_id),
    )
    .await
    .map_err(|e| ApiError::Internal(format!("acl view check: {e}")))?;
    if !can_view {
        drop(guard);
        return Err(ApiError::NotFound);
    }

    let rows = sqlx::query_as::<_, GrantRow>(
        "SELECT id, principal_type, principal_id, permission, created_at
         FROM resource_acl
         WHERE resource_type = $1 AND resource_id = $2
         ORDER BY created_at",
    )
    .bind(&params.resource_type)
    .bind(params.resource_id)
    .fetch_all(&mut *guard)
    .await
    .map_err(|e| ApiError::Internal(format!("db error: {e}")))?;
    drop(guard);

    Ok(Json(serde_json::json!({ "grants": rows })))
}

/// Create an ACL grant (owner-only).
#[utoipa::path(
    post,
    path = "/tenants/{tid}/acl",
    tag = "ACL",
    params(
        ("tid" = Uuid, Path, description = "Tenant ID"),
        ("X-Tenant-Id" = Uuid, Header, description = "Must match path tid"),
    ),
    security(("bearer_auth" = [])),
    request_body = crate::openapi::schemas::CreateGrantRequest,
    responses(
        (status = 201, description = "Grant created", body = crate::openapi::schemas::CreateGrantResponse),
        (status = 400, description = "Bad request", body = crate::openapi::schemas::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::openapi::schemas::ErrorResponse),
        (status = 403, description = "Forbidden — owner only", body = crate::openapi::schemas::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::openapi::schemas::ErrorResponse),
    )
)]
pub async fn create_grant(
    Path(tid): Path<Uuid>,
    Extension(ctx): Extension<TenantContext>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
    Json(body): Json<CreateGrantBody>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;
    let (object, principal, relation) = parse_grant(
        &body.resource_type,
        body.resource_id,
        &body.principal_type,
        body.principal_id,
        &body.relation,
    )?;

    let mut guard = conn.lock().await;
    require_owner(&mut guard, &object, auth_user.user_id).await?;

    // Idempotent insert: an identical grant is a no-op (UNIQUE constraint).
    let grant_id: Uuid = sqlx::query_scalar(
        "INSERT INTO resource_acl
           (tenant_id, resource_type, resource_id, principal_type, principal_id, permission)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (resource_type, resource_id, principal_type, principal_id, permission)
           DO UPDATE SET permission = EXCLUDED.permission
         RETURNING id",
    )
    .bind(tid)
    .bind(&object.namespace)
    .bind(object.id)
    .bind(principal.type_str())
    .bind(principal.id())
    .bind(relation.as_str())
    .fetch_one(&mut *guard)
    .await
    .map_err(|e| ApiError::Internal(format!("insert grant: {e}")))?;

    write_audit(
        &mut guard,
        tid,
        auth_user.user_id,
        "acl.grant",
        &object,
        serde_json::json!({
            "principal_type": principal.type_str(),
            "principal_id": principal.id(),
            "relation": relation.as_str(),
        }),
    )
    .await?;
    drop(guard);

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": grant_id,
            "resource_type": object.namespace,
            "resource_id": object.id,
            "principal_type": principal.type_str(),
            "principal_id": principal.id(),
            "relation": relation.as_str(),
        })),
    ))
}

/// Revoke an ACL grant (owner-only).
#[utoipa::path(
    delete,
    path = "/tenants/{tid}/acl/{grant_id}",
    tag = "ACL",
    params(
        ("tid" = Uuid, Path, description = "Tenant ID"),
        ("grant_id" = Uuid, Path, description = "Grant ID"),
        ("X-Tenant-Id" = Uuid, Header, description = "Must match path tid"),
    ),
    security(("bearer_auth" = [])),
    responses(
        (status = 204, description = "Grant revoked"),
        (status = 400, description = "Bad request", body = crate::openapi::schemas::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::openapi::schemas::ErrorResponse),
        (status = 403, description = "Forbidden — owner only", body = crate::openapi::schemas::ErrorResponse),
        (status = 404, description = "Grant not found", body = crate::openapi::schemas::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::openapi::schemas::ErrorResponse),
    )
)]
pub async fn revoke_grant(
    Path((tid, grant_id)): Path<(Uuid, Uuid)>,
    Extension(ctx): Extension<TenantContext>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;

    let mut guard = conn.lock().await;
    // Load the grant (RLS-scoped → cross-tenant rows are invisible → 404).
    let row: Option<(String, Uuid)> =
        sqlx::query_as("SELECT resource_type, resource_id FROM resource_acl WHERE id = $1")
            .bind(grant_id)
            .fetch_optional(&mut *guard)
            .await
            .map_err(|e| ApiError::Internal(format!("load grant: {e}")))?;
    let (resource_type, resource_id) = match row {
        Some(r) => r,
        None => {
            drop(guard);
            return Err(ApiError::NotFound);
        }
    };
    let object = ObjectRef::new(resource_type, resource_id);

    require_owner(&mut guard, &object, auth_user.user_id).await?;

    sqlx::query("DELETE FROM resource_acl WHERE id = $1")
        .bind(grant_id)
        .execute(&mut *guard)
        .await
        .map_err(|e| ApiError::Internal(format!("delete grant: {e}")))?;

    write_audit(
        &mut guard,
        tid,
        auth_user.user_id,
        "acl.revoke",
        &object,
        serde_json::json!({ "grant_id": grant_id }),
    )
    .await?;
    drop(guard);

    Ok(StatusCode::NO_CONTENT)
}

/// Append an immutable audit row for an ACL change.
async fn write_audit(
    guard: &mut sqlx::PgConnection,
    tenant_id: Uuid,
    actor_id: Uuid,
    action: &str,
    object: &ObjectRef,
    metadata: serde_json::Value,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO audit_log (tenant_id, actor_id, action, resource_type, resource_id, metadata)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(tenant_id)
    .bind(actor_id)
    .bind(action)
    .bind(&object.namespace)
    .bind(object.id)
    .bind(metadata)
    .execute(guard)
    .await
    .map_err(|e| ApiError::Internal(format!("write audit: {e}")))?;
    Ok(())
}
