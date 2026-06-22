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
use crate::routes::tenants::ensure_path_matches_context;
use crate::storage::ObjectStore;

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
