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

/// `POST /tenants/{tid}/documents` — upload a document (T58).
///
/// All-or-nothing across S3 + Postgres + Redis. Because `rls_middleware`
/// owns the request transaction and COMMITs it unconditionally after the
/// handler returns (even on error), atomicity is achieved at the handler
/// level: S3 is written first, the DB inserts run inside a `SAVEPOINT`, and
/// on any post-upload failure we `ROLLBACK TO SAVEPOINT` (undoing both
/// inserts within the outer tx) and delete the S3 object before returning an
/// error.
///
/// Multipart form fields:
/// - `file`        — the document bytes (required; filename + content-type read)
/// - `visibility`  — `shared` | `private` (required, Serde-validated)
/// - `workspace_id`— owning workspace UUID (required; the worker job needs it)
/// - `title`       — optional display title (defaults to the filename)
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

    // 6. Enqueue the fully-populated ingest job (owner_id + visibility per T43).
    let payload = IngestJobPayload {
        id: job_id,
        tenant_id: tid,
        workspace_id,
        document_id,
        s3_key: s3_key.clone(),
        filename: filename.clone(),
        owner_id: owner,
        visibility: visibility.as_str().to_string(),
        attempts: 0,
    };
    if let Err(e) = enqueuer.enqueue(&payload).await {
        let _ = sqlx::Executor::execute(&mut *guard, "ROLLBACK TO SAVEPOINT sp_upload").await;
        drop(guard);
        let _ = store.delete(&s3_key).await;
        return Err(ApiError::Internal(format!("enqueue ingest job: {e}")));
    }

    let _ = sqlx::Executor::execute(&mut *guard, "RELEASE SAVEPOINT sp_upload").await;
    drop(guard);

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "id": document_id })),
    ))
}

/// `DELETE /tenants/{tid}/documents/{did}` — delete a document (T59).
///
/// Owner-only. Order: load (RLS-scoped) → owner guard → S3 object delete →
/// Qdrant chunk-vector cleanup (orphan removal) → Postgres `DELETE` (cascade
/// removes `document_chunks` + `ingest_jobs`). Cross-tenant rows are hidden by
/// RLS, so they surface as `404`.
///
/// Graph nodes/edges are intentionally NOT cleaned per-document: they are
/// deduplicated per `(tenant, workspace, label, kind)` and shared across
/// documents, with no `document_id` link in the schema (see
/// `QdrantStore::delete_chunks_by_document` and the T59 progress notes).
///
/// S3 / Qdrant cleanup is best-effort (failures are logged, not fatal) so a
/// transient object-store hiccup cannot strand the document row; the Postgres
/// delete is the authoritative step.
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

/// `GET /tenants/{tid}/documents/{did}/preview` — metadata + first chunks (T60).
///
/// Authorization is the ReBAC `viewer` relation, evaluated by
/// [`crate::rbac::check::check_relation`] (T83): the document is returned iff
/// `visibility = 'shared'`, the caller is the owner, a member of the
/// document's workspace, or the recipient of a `resource_acl` grant (directly
/// or via a workspace group). RLS scopes to the tenant first, so cross-tenant
/// or unauthorized access both surface as `404` (no information leak). Returns
/// up to [`PREVIEW_CHUNK_LIMIT`] chunks ordered by `chunk_index`.
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
