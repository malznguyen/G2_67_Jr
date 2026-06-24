//! Document list route (T57).
//!
//! Tenant-scoped: runs inside the `tenant_middleware` + `rls_middleware` chain
//! and executes on the per-request [`SharedConnection`], so PostgreSQL RLS
//! confines every query to the current tenant. On top of tenant isolation, the
//! list applies a per-user visibility + ACL filter:
//!
//! A document is returned iff the caller holds the `viewer` relation on it
//! (ReBAC, T83): `visibility = 'shared'`, owner, a member of the document's
//! workspace (inheritance), or the recipient of a `resource_acl` grant
//! (directly or via a workspace-group share). This is the set-compiled form of
//! [`crate::rbac::check::check_relation`]`(document, viewer, user)` so the
//! listing stays a single indexed query instead of an N+1 per-row Check.
//!
//! An optional `workspace_id` query parameter further narrows the result.

use std::sync::Arc;

use axum::extract::{Extension, Multipart, Path, Query};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::extractor::AuthUser;
use crate::auth::tenant::TenantContext;
use crate::error::ApiError;
use crate::middleware::rls::SharedConnection;
use crate::queue::{IngestJobPayload, JobEnqueuer};
use crate::rbac::check::check_relation;
use crate::rbac::model::{ObjectRef, Principal, Relation, NS_DOCUMENT};
use crate::routes::tenants::ensure_path_matches_context;
use crate::storage::ObjectStore;
use crate::vector::VectorCleaner;

/// Marks a document as readable by anyone in the tenant.
const VISIBILITY_SHARED: &str = "shared";

/// Document visibility, enforced to the literals `shared`/`private` through
/// Serde so an invalid form value is rejected before it ever reaches the DB
/// (T58 validation requirement).
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Visibility {
    Shared,
    Private,
}

impl Visibility {
    fn as_str(self) -> &'static str {
        match self {
            Visibility::Shared => "shared",
            Visibility::Private => "private",
        }
    }
}

/// Parse a raw form string into [`Visibility`] via Serde (rejects anything
/// other than `shared`/`private`).
fn parse_visibility(raw: &str) -> Result<Visibility, ApiError> {
    serde_json::from_value::<Visibility>(serde_json::Value::String(raw.to_string())).map_err(|_| {
        ApiError::BadRequest(format!(
            "invalid visibility '{raw}'; must be 'shared' or 'private'"
        ))
    })
}

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

/// List documents visible to the caller.
#[utoipa::path(
    get,
    path = "/tenants/{tid}/documents",
    tag = "Documents",
    params(
        ("tid" = Uuid, Path, description = "Tenant ID"),
        ("X-Tenant-Id" = Uuid, Header, description = "Must match path tid"),
        ("workspace_id" = Option<Uuid>, Query, description = "Optional workspace filter"),
    ),
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Document list (unpaginated)", body = crate::openapi::schemas::DocumentsResponse),
        (status = 400, description = "Bad request", body = crate::openapi::schemas::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::openapi::schemas::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::openapi::schemas::ErrorResponse),
    )
)]
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
                 OR EXISTS (
                       SELECT 1 FROM resource_acl ra
                       WHERE ra.resource_type = 'document'
                         AND ra.resource_id = documents.id
                         AND ra.permission IN ('owner', 'editor', 'viewer')
                         AND (
                               (ra.principal_type = 'user' AND ra.principal_id = $2)
                               OR (ra.principal_type = 'workspace' AND EXISTS (
                                     SELECT 1 FROM workspace_members wmg
                                     WHERE wmg.workspace_id = ra.principal_id
                                       AND wmg.user_id = $2))
                             ))
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

/// Upload a document (multipart form).
#[utoipa::path(
    post,
    path = "/tenants/{tid}/documents",
    tag = "Documents",
    params(
        ("tid" = Uuid, Path, description = "Tenant ID"),
        ("X-Tenant-Id" = Uuid, Header, description = "Must match path tid"),
    ),
    security(("bearer_auth" = [])),
    request_body(
        content = crate::openapi::schemas::UploadDocumentForm,
        content_type = "multipart/form-data",
        description = "Fields: file (binary), visibility (shared|private), workspace_id, optional title"
    ),
    responses(
        (status = 201, description = "Document uploaded", body = crate::openapi::schemas::CreateDocumentResponse),
        (status = 400, description = "Invalid form", body = crate::openapi::schemas::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::openapi::schemas::ErrorResponse),
        (status = 429, description = "Quota exceeded", body = crate::openapi::schemas::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::openapi::schemas::ErrorResponse),
    )
)]
pub async fn upload_document(
    Path(tid): Path<Uuid>,
    Extension(ctx): Extension<TenantContext>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
    Extension(store): Extension<Arc<dyn ObjectStore>>,
    Extension(enqueuer): Extension<Arc<dyn JobEnqueuer>>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;
    let owner = auth_user.user_id;

    // 1. Parse multipart parts.
    let mut file_bytes: Option<Vec<u8>> = None;
    let mut filename: Option<String> = None;
    let mut content_type = "application/octet-stream".to_string();
    let mut visibility_raw: Option<String> = None;
    let mut workspace_id: Option<Uuid> = None;
    let mut title: Option<String> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(format!("invalid multipart: {e}")))?
    {
        match field.name() {
            Some("file") => {
                filename = field.file_name().map(|s| s.to_string());
                if let Some(ct) = field.content_type() {
                    content_type = ct.to_string();
                }
                let data = field
                    .bytes()
                    .await
                    .map_err(|e| ApiError::BadRequest(format!("reading file part: {e}")))?;
                file_bytes = Some(data.to_vec());
            }
            Some("visibility") => {
                visibility_raw = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| ApiError::BadRequest(format!("reading visibility: {e}")))?,
                );
            }
            Some("workspace_id") => {
                let raw = field
                    .text()
                    .await
                    .map_err(|e| ApiError::BadRequest(format!("reading workspace_id: {e}")))?;
                let id = Uuid::parse_str(raw.trim()).map_err(|_| {
                    ApiError::BadRequest("workspace_id is not a valid UUID".into())
                })?;
                workspace_id = Some(id);
            }
            Some("title") => {
                title = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| ApiError::BadRequest(format!("reading title: {e}")))?,
                );
            }
            _ => {
                // Drain unknown fields so the stream advances.
                let _ = field.bytes().await;
            }
        }
    }

    // 2. Validate (visibility strictly before any S3 write).
    let bytes = file_bytes.ok_or_else(|| ApiError::BadRequest("missing 'file' part".into()))?;
    let visibility = parse_visibility(
        &visibility_raw.ok_or_else(|| ApiError::BadRequest("missing 'visibility' field".into()))?,
    )?;
    let workspace_id =
        workspace_id.ok_or_else(|| ApiError::BadRequest("missing 'workspace_id' field".into()))?;
    let filename = filename.unwrap_or_else(|| "upload.bin".to_string());
    let title = title.unwrap_or_else(|| filename.clone());
    let byte_size = bytes.len() as i64;

    // 3. Quota check (RLS-scoped). Missing quota row → no enforcement.
    {
        let mut guard = conn.lock().await;
        let quota: Option<(i64, i32)> = sqlx::query_as(
            "SELECT max_storage_bytes, max_documents FROM tenant_quotas WHERE tenant_id = $1",
        )
        .bind(tid)
        .fetch_optional(&mut *guard)
        .await
        .map_err(|e| ApiError::Internal(format!("quota lookup: {e}")))?;
        if let Some((max_bytes, max_docs)) = quota {
            let usage: (i64, i64) = sqlx::query_as(
                "SELECT COALESCE(SUM(byte_size), 0)::bigint, COUNT(*)::bigint FROM documents",
            )
            .fetch_one(&mut *guard)
            .await
            .map_err(|e| ApiError::Internal(format!("usage lookup: {e}")))?;
            let (used_bytes, doc_count) = usage;
            if used_bytes + byte_size > max_bytes {
                return Err(ApiError::QuotaExceeded(format!(
                    "storage quota exceeded: {used_bytes} + {byte_size} > {max_bytes} bytes"
                )));
            }
            if doc_count + 1 > i64::from(max_docs) {
                return Err(ApiError::QuotaExceeded(format!(
                    "document quota exceeded: {doc_count} of {max_docs} used"
                )));
            }
        }
    }

    // 4. Upload to S3 first (so a failed DB/queue step can delete it).
    let document_id = Uuid::new_v4();
    let s3_key = format!("{tid}/{workspace_id}/{document_id}.pdf");
    store
        .put(&s3_key, bytes, &content_type)
        .await
        .map_err(|e| ApiError::Internal(format!("s3 upload: {e}")))?;

    // 5. DB inserts inside a SAVEPOINT so we can undo them on later failure.
    let mut guard = conn.lock().await;
    if let Err(e) = sqlx::Executor::execute(&mut *guard, "SAVEPOINT sp_upload").await {
        drop(guard);
        let _ = store.delete(&s3_key).await;
        return Err(ApiError::Internal(format!("savepoint: {e}")));
    }

    let insert_result: Result<Uuid, ApiError> = async {
        sqlx::query(
            "INSERT INTO documents
               (id, tenant_id, workspace_id, owner_id, title, status, visibility, mime_type, byte_size, s3_key)
             VALUES ($1, $2, $3, $4, $5, 'uploaded', $6, $7, $8, $9)",
        )
        .bind(document_id)
        .bind(tid)
        .bind(workspace_id)
        .bind(owner)
        .bind(&title)
        .bind(visibility.as_str())
        .bind(&content_type)
        .bind(byte_size)
        .bind(&s3_key)
        .execute(&mut *guard)
        .await
        .map_err(|e| ApiError::Internal(format!("insert document: {e}")))?;

        let job_id: (Uuid,) = sqlx::query_as(
            "INSERT INTO ingest_jobs (tenant_id, document_id, status)
             VALUES ($1, $2, 'pending')
             RETURNING id",
        )
        .bind(tid)
        .bind(document_id)
        .fetch_one(&mut *guard)
        .await
        .map_err(|e| ApiError::Internal(format!("insert ingest_job: {e}")))?;

        // T84D Phase 1.1: instead of LPUSHing to Redis here (which is
        // post-commit unsafe because the RLS middleware owns COMMIT),
        // the handler writes one `ingest_outbox` row inside the same
        // transaction. A worker relay drains pending rows and LPUSHes
        // them onto `gmrag:ingest_jobs` after COMMIT — atomic with the
        // documents/ingest_jobs inserts, so a DB failure rolls the whole
        // thing back (no orphan Redis job) and a relay failure leaves a
        // pending row the sweeper can flip.
        let payload = IngestJobPayload {
            id: job_id.0,
            tenant_id: tid,
            workspace_id,
            document_id,
            s3_key: s3_key.clone(),
            filename: filename.clone(),
            owner_id: owner,
            visibility: visibility.as_str().to_string(),
            attempts: 0,
        };
        let payload_json = serde_json::to_value(&payload)
            .map_err(|e| ApiError::Internal(format!("serialize ingest payload: {e}")))?;
        sqlx::query(
            "INSERT INTO ingest_outbox (tenant_id, document_id, payload)
             VALUES ($1, $2, $3)",
        )
        .bind(tid)
        .bind(document_id)
        .bind(payload_json)
        .execute(&mut *guard)
        .await
        .map_err(|e| ApiError::Internal(format!("insert ingest_outbox: {e}")))?;
        Ok(job_id.0)
    }
    .await;

    let job_id = match insert_result {
        Ok(id) => id,
        Err(e) => {
            let _ =
                sqlx::Executor::execute(&mut *guard, "ROLLBACK TO SAVEPOINT sp_upload").await;
            drop(guard);
            let _ = store.delete(&s3_key).await;
            return Err(e);
        }
    };

    // The JobEnqueuer extension is retained for tests/legacy callers but is
    // no longer used at runtime — `enqueuer` is intentionally unused here so
    // the outbox insert is the sole enqueue path. Drop the unused reference
    // explicitly to make the invariant obvious.
    drop(enqueuer);

    let _ = job_id; // job_id is recorded in ingest_outbox; no further use.
    let _ = sqlx::Executor::execute(&mut *guard, "RELEASE SAVEPOINT sp_upload").await;
    drop(guard);

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "id": document_id })),
    ))
}

/// Delete a document (owner-only).
#[utoipa::path(
    delete,
    path = "/tenants/{tid}/documents/{did}",
    tag = "Documents",
    params(
        ("tid" = Uuid, Path, description = "Tenant ID"),
        ("did" = Uuid, Path, description = "Document ID"),
        ("X-Tenant-Id" = Uuid, Header, description = "Must match path tid"),
    ),
    security(("bearer_auth" = [])),
    responses(
        (status = 204, description = "Document deleted"),
        (status = 400, description = "Bad request", body = crate::openapi::schemas::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::openapi::schemas::ErrorResponse),
        (status = 403, description = "Forbidden — owner only", body = crate::openapi::schemas::ErrorResponse),
        (status = 404, description = "Document not found", body = crate::openapi::schemas::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::openapi::schemas::ErrorResponse),
    )
)]
pub async fn delete_document(
    Path((tid, did)): Path<(Uuid, Uuid)>,
    Extension(ctx): Extension<TenantContext>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
    Extension(store): Extension<Arc<dyn ObjectStore>>,
    Extension(cleaner): Extension<Arc<dyn VectorCleaner>>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;

    // 1. Load the document under RLS (cross-tenant → no row → 404), then apply
    //    the ReBAC owner guard via the Check engine (T83).
    let s3_key = {
        let mut guard = conn.lock().await;
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT s3_key FROM documents WHERE id = $1")
                .bind(did)
                .fetch_optional(&mut *guard)
                .await
                .map_err(|e| ApiError::Internal(format!("load document: {e}")))?;
        let s3_key = row.ok_or(ApiError::NotFound)?.0;

        // 2. Owner-only guard (delete is an owner action in the ResourceBAC matrix).
        let is_owner = check_relation(
            &mut guard,
            &ObjectRef::new(NS_DOCUMENT, did),
            Relation::Owner,
            Principal::User(auth_user.user_id),
        )
        .await
        .map_err(|e| ApiError::Internal(format!("owner check: {e}")))?;
        if !is_owner {
            drop(guard);
            return Err(ApiError::Forbidden(
                "only the document owner may delete it".into(),
            ));
        }
        s3_key
    };

    // 3. Best-effort external cleanup: S3 object, then Qdrant chunk vectors.
    if let Some(key) = &s3_key {
        if let Err(e) = store.delete(key).await {
            tracing::warn!(error = %e, document_id = %did, "s3 delete failed during document delete");
        }
    }
    if let Err(e) = cleaner.delete_document_chunks(tid, did).await {
        tracing::warn!(error = %e, document_id = %did, "qdrant cleanup failed during document delete");
    }

    // 4. Postgres delete (cascade: document_chunks + ingest_jobs).
    {
        let mut guard = conn.lock().await;
        sqlx::query("DELETE FROM documents WHERE id = $1")
            .bind(did)
            .execute(&mut *guard)
            .await
            .map_err(|e| ApiError::Internal(format!("delete document: {e}")))?;
    }

    Ok(StatusCode::NO_CONTENT)
}

/// Document metadata returned by the preview endpoint.
#[derive(Serialize, sqlx::FromRow)]
struct DocumentPreview {
    id: Uuid,
    title: String,
    status: String,
    visibility: String,
    owner_id: Uuid,
    workspace_id: Option<Uuid>,
    mime_type: Option<String>,
    byte_size: i64,
    created_at: chrono::DateTime<chrono::Utc>,
}

/// One chunk row in the preview response.
#[derive(Serialize, sqlx::FromRow)]
struct ChunkPreview {
    chunk_index: i32,
    content: String,
    token_count: Option<i32>,
}

/// Max chunks returned in a single preview.
const PREVIEW_CHUNK_LIMIT: i64 = 50;

/// Document metadata and first chunks (viewer-gated, max 50 chunks).
#[utoipa::path(
    get,
    path = "/tenants/{tid}/documents/{did}/preview",
    tag = "Documents",
    params(
        ("tid" = Uuid, Path, description = "Tenant ID"),
        ("did" = Uuid, Path, description = "Document ID"),
        ("X-Tenant-Id" = Uuid, Header, description = "Must match path tid"),
    ),
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Document preview", body = crate::openapi::schemas::DocumentPreviewResponse),
        (status = 400, description = "Bad request", body = crate::openapi::schemas::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::openapi::schemas::ErrorResponse),
        (status = 404, description = "Not found or no viewer access", body = crate::openapi::schemas::ErrorResponse),
        (status = 500, description = "Internal error", body = crate::openapi::schemas::ErrorResponse),
    )
)]
pub async fn preview_document(
    Path((tid, did)): Path<(Uuid, Uuid)>,
    Extension(ctx): Extension<TenantContext>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;

    let mut guard = conn.lock().await;
    // Load the row under RLS, then gate on the viewer relation. A missing row
    // and a denied check both yield 404 (no existence leak).
    let document = sqlx::query_as::<_, DocumentPreview>(
        "SELECT id, title, status, visibility, owner_id, workspace_id, mime_type, byte_size, created_at
         FROM documents
         WHERE id = $1",
    )
    .bind(did)
    .fetch_optional(&mut *guard)
    .await
    .map_err(|e| ApiError::Internal(format!("load document: {e}")))?
    .ok_or(ApiError::NotFound)?;

    let can_view = check_relation(
        &mut guard,
        &ObjectRef::new(NS_DOCUMENT, did),
        Relation::Viewer,
        Principal::User(auth_user.user_id),
    )
    .await
    .map_err(|e| ApiError::Internal(format!("viewer check: {e}")))?;
    if !can_view {
        drop(guard);
        return Err(ApiError::NotFound);
    }

    let chunks = sqlx::query_as::<_, ChunkPreview>(
        "SELECT chunk_index, content, token_count
         FROM document_chunks
         WHERE document_id = $1
         ORDER BY chunk_index ASC
         LIMIT $2",
    )
    .bind(did)
    .bind(PREVIEW_CHUNK_LIMIT)
    .fetch_all(&mut *guard)
    .await
    .map_err(|e| ApiError::Internal(format!("load chunks: {e}")))?;
    drop(guard);

    Ok(Json(serde_json::json!({
        "document": document,
        "chunks": chunks,
    })))
}
