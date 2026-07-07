//! ACL grant routes backed by OpenFGA direct relationship tuples.

use axum::extract::{Extension, Path, Query};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::extractor::AuthUser;
use crate::auth::tenant::TenantContext;
use crate::authz::{
    chat_session_obj, check_or_unavailable, decode_grant_id, document_obj, encode_grant_id,
    parse_grant_principal, tuple, typed_uuid, user_obj, AuthzService, CheckRequest, GrantPrincipal,
    RelationshipTuple, REL_EDITOR, REL_OWNER, REL_VIEWER, TYPE_CHAT_SESSION, TYPE_DOCUMENT,
};
use crate::error::ApiError;
use crate::middleware::rls::SharedConnection;
use crate::routes::tenants::ensure_path_matches_context;

fn is_shareable_namespace(ns: &str) -> bool {
    matches!(ns, TYPE_DOCUMENT | TYPE_CHAT_SESSION)
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

#[derive(Serialize)]
struct GrantRow {
    id: String,
    principal_type: String,
    principal_id: Uuid,
    relation: String,
    created_at: Option<chrono::DateTime<chrono::Utc>>,
}

fn openfga_object(resource_type: &str, resource_id: Uuid) -> Result<String, ApiError> {
    match resource_type {
        TYPE_DOCUMENT => Ok(document_obj(resource_id)),
        TYPE_CHAT_SESSION => Ok(chat_session_obj(resource_id)),
        _ => Err(ApiError::BadRequest(format!(
            "resource_type '{resource_type}' is not shareable"
        ))),
    }
}

fn decode_shareable_object(object: &str) -> Result<(&'static str, Uuid), ApiError> {
    if let Ok(id) = typed_uuid(object, TYPE_DOCUMENT) {
        return Ok((TYPE_DOCUMENT, id));
    }
    if let Ok(id) = typed_uuid(object, TYPE_CHAT_SESSION) {
        return Ok((TYPE_CHAT_SESSION, id));
    }
    Err(ApiError::BadRequest("grant object is not shareable".into()))
}

fn parse_grant(
    resource_type: &str,
    resource_id: Uuid,
    principal_type: &str,
    principal_id: Uuid,
    relation: &str,
) -> Result<(String, GrantPrincipal, String), ApiError> {
    if !is_shareable_namespace(resource_type) {
        return Err(ApiError::BadRequest(format!(
            "resource_type '{resource_type}' is not shareable"
        )));
    }
    if !matches!(relation, REL_EDITOR | REL_VIEWER) {
        return Err(ApiError::BadRequest(format!(
            "relation '{relation}' is not grantable (use 'editor' or 'viewer')"
        )));
    }
    let principal = match principal_type {
        "user" => GrantPrincipal::User(principal_id),
        "workspace" => GrantPrincipal::Workspace(principal_id),
        _ => {
            return Err(ApiError::BadRequest(format!(
                "principal_type '{principal_type}' is invalid"
            )))
        }
    };
    Ok((
        openfga_object(resource_type, resource_id)?,
        principal,
        relation.to_string(),
    ))
}

async fn require_owner(authz: &AuthzService, object: &str, user_id: Uuid) -> Result<(), ApiError> {
    let is_owner = check_or_unavailable(
        authz,
        CheckRequest::new(user_obj(user_id), REL_OWNER, object.to_string()),
    )
    .await?;
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
        ("X-Tenant-ID" = Uuid, Header, description = "Must match path tid"),
        ("resource_type" = String, Query, description = "document or chat_session"),
        ("resource_id" = Uuid, Query, description = "Resource UUID"),
    ),
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Grant list", body = crate::openapi::schemas::GrantsResponse),
        (status = 400, description = "Bad request", body = crate::openapi::schemas::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::openapi::schemas::ErrorResponse),
        (status = 404, description = "Resource not found or no viewer access", body = crate::openapi::schemas::ErrorResponse),
        (status = 503, description = "Authorization unavailable", body = crate::openapi::schemas::ErrorResponse),
    )
)]
pub async fn list_grants(
    Path(tid): Path<Uuid>,
    Extension(ctx): Extension<TenantContext>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
    Extension(authz): Extension<AuthzService>,
    Query(params): Query<AclListParams>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;
    let object = openfga_object(&params.resource_type, params.resource_id)?;

    let mut guard = conn.lock().await;
    ensure_resource_exists(&mut guard, &params.resource_type, params.resource_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    drop(guard);

    let can_view = check_or_unavailable(
        &authz,
        CheckRequest::new(user_obj(auth_user.user_id), REL_VIEWER, object.clone()),
    )
    .await?;
    if !can_view {
        return Err(ApiError::NotFound);
    }

    let tuples = authz
        .read_direct_relationships(&object)
        .await
        .map_err(|e| ApiError::AuthorizationUnavailable(e.to_string()))?;
    let rows = grant_rows_from_tuples(tuples)?;

    Ok(Json(serde_json::json!({ "grants": rows })))
}

/// Create an ACL grant (owner-only).
#[utoipa::path(
    post,
    path = "/tenants/{tid}/acl",
    tag = "ACL",
    params(
        ("tid" = Uuid, Path, description = "Tenant ID"),
        ("X-Tenant-ID" = Uuid, Header, description = "Must match path tid"),
    ),
    security(("bearer_auth" = [])),
    request_body = crate::openapi::schemas::CreateGrantRequest,
    responses(
        (status = 201, description = "Grant created", body = crate::openapi::schemas::CreateGrantResponse),
        (status = 400, description = "Bad request", body = crate::openapi::schemas::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::openapi::schemas::ErrorResponse),
        (status = 403, description = "Forbidden - owner only", body = crate::openapi::schemas::ErrorResponse),
        (status = 404, description = "Resource not found", body = crate::openapi::schemas::ErrorResponse),
        (status = 503, description = "Authorization unavailable", body = crate::openapi::schemas::ErrorResponse),
    )
)]
pub async fn create_grant(
    Path(tid): Path<Uuid>,
    Extension(ctx): Extension<TenantContext>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
    Extension(authz): Extension<AuthzService>,
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
    ensure_resource_exists(&mut guard, &body.resource_type, body.resource_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    validate_principal_in_tenant(&mut guard, &principal).await?;
    drop(guard);

    require_owner(&authz, &object, auth_user.user_id).await?;

    let tuple_key = tuple(principal.to_openfga_user(), &relation, object.clone());
    crate::authz::write_or_unavailable(&authz, vec![tuple_key.clone()], Vec::new()).await?;
    let grant_id = encode_grant_id(&tuple_key);

    let mut guard = conn.lock().await;
    write_audit(
        &mut guard,
        tid,
        auth_user.user_id,
        "acl.grant",
        &body.resource_type,
        body.resource_id,
        serde_json::json!({
            "principal_type": principal.principal_type(),
            "principal_id": principal.id(),
            "relation": relation,
        }),
    )
    .await?;
    drop(guard);

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": grant_id,
            "resource_type": body.resource_type,
            "resource_id": body.resource_id,
            "principal_type": principal.principal_type(),
            "principal_id": principal.id(),
            "relation": relation,
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
        ("grant_id" = String, Path, description = "Opaque grant ID"),
        ("X-Tenant-ID" = Uuid, Header, description = "Must match path tid"),
    ),
    security(("bearer_auth" = [])),
    responses(
        (status = 204, description = "Grant revoked"),
        (status = 400, description = "Bad request", body = crate::openapi::schemas::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::openapi::schemas::ErrorResponse),
        (status = 403, description = "Forbidden - owner only", body = crate::openapi::schemas::ErrorResponse),
        (status = 404, description = "Grant not found", body = crate::openapi::schemas::ErrorResponse),
        (status = 503, description = "Authorization unavailable", body = crate::openapi::schemas::ErrorResponse),
    )
)]
pub async fn revoke_grant(
    Path((tid, grant_id)): Path<(Uuid, String)>,
    Extension(ctx): Extension<TenantContext>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
    Extension(authz): Extension<AuthzService>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;

    let decoded = decode_grant_id(&grant_id)
        .map_err(|e| ApiError::BadRequest(format!("invalid grant id: {e}")))?;
    if !matches!(decoded.relation.as_str(), REL_EDITOR | REL_VIEWER) {
        return Err(ApiError::BadRequest(
            "grant relation is not revocable".into(),
        ));
    }
    let (resource_type, resource_id) = decode_shareable_object(&decoded.object)?;
    let principal = parse_grant_principal(&decoded.user)
        .map_err(|e| ApiError::BadRequest(format!("invalid grant principal: {e}")))?;

    let mut guard = conn.lock().await;
    ensure_resource_exists(&mut guard, resource_type, resource_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    validate_principal_in_tenant(&mut guard, &principal).await?;
    drop(guard);

    require_owner(&authz, &decoded.object, auth_user.user_id).await?;
    let tuple_key = RelationshipTuple::new(decoded.user, decoded.relation, decoded.object);
    crate::authz::write_or_unavailable(&authz, Vec::new(), vec![tuple_key]).await?;

    let mut guard = conn.lock().await;
    write_audit(
        &mut guard,
        tid,
        auth_user.user_id,
        "acl.revoke",
        resource_type,
        resource_id,
        serde_json::json!({ "grant_id": grant_id }),
    )
    .await?;
    drop(guard);

    Ok(StatusCode::NO_CONTENT)
}

async fn write_audit(
    guard: &mut sqlx::PgConnection,
    tenant_id: Uuid,
    actor_id: Uuid,
    action: &str,
    resource_type: &str,
    resource_id: Uuid,
    metadata: serde_json::Value,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO audit_log (tenant_id, actor_id, action, resource_type, resource_id, metadata)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(tenant_id)
    .bind(actor_id)
    .bind(action)
    .bind(resource_type)
    .bind(resource_id)
    .bind(metadata)
    .execute(guard)
    .await
    .map_err(|e| ApiError::Internal(format!("write audit: {e}")))?;
    Ok(())
}

async fn ensure_resource_exists(
    guard: &mut sqlx::PgConnection,
    resource_type: &str,
    resource_id: Uuid,
) -> Result<Option<Uuid>, ApiError> {
    let exists = match resource_type {
        TYPE_DOCUMENT => {
            sqlx::query_scalar("SELECT id FROM documents WHERE id = $1")
                .bind(resource_id)
                .fetch_optional(guard)
                .await
        }
        TYPE_CHAT_SESSION => {
            sqlx::query_scalar("SELECT id FROM chat_sessions WHERE id = $1")
                .bind(resource_id)
                .fetch_optional(guard)
                .await
        }
        _ => {
            return Err(ApiError::BadRequest(
                "resource_type is not shareable".into(),
            ))
        }
    }
    .map_err(|e| ApiError::Internal(format!("load ACL resource: {e}")))?;
    Ok(exists)
}

async fn validate_principal_in_tenant(
    guard: &mut sqlx::PgConnection,
    principal: &GrantPrincipal,
) -> Result<(), ApiError> {
    let exists: bool = match principal {
        GrantPrincipal::User(user_id) => {
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tenant_members WHERE user_id = $1)")
                .bind(user_id)
                .fetch_one(guard)
                .await
        }
        GrantPrincipal::Workspace(workspace_id) => {
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workspaces WHERE id = $1)")
                .bind(workspace_id)
                .fetch_one(guard)
                .await
        }
    }
    .map_err(|e| ApiError::Internal(format!("validate ACL principal: {e}")))?;
    if exists {
        Ok(())
    } else {
        Err(ApiError::BadRequest(
            "principal must belong to the current tenant".into(),
        ))
    }
}

fn grant_rows_from_tuples(tuples: Vec<RelationshipTuple>) -> Result<Vec<GrantRow>, ApiError> {
    let mut rows = Vec::new();
    for tuple_key in tuples {
        if !matches!(tuple_key.relation.as_str(), REL_EDITOR | REL_VIEWER) {
            continue;
        }
        let principal = parse_grant_principal(&tuple_key.user)
            .map_err(|e| ApiError::Internal(format!("stored OpenFGA grant is malformed: {e}")))?;
        rows.push(GrantRow {
            id: encode_grant_id(&tuple_key),
            principal_type: principal.principal_type().to_string(),
            principal_id: principal.id(),
            relation: tuple_key.relation,
            created_at: None,
        });
    }
    rows.sort_by(|a, b| {
        a.principal_type
            .cmp(&b.principal_type)
            .then(a.principal_id.cmp(&b.principal_id))
            .then(a.relation.cmp(&b.relation))
    });
    Ok(rows)
}
