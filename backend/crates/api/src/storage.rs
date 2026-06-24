//! Object storage abstraction for the API (T58).
//!
//! The document upload endpoint must push the raw file to S3/MinIO *before*
//! touching Postgres/Redis, and must delete it again if any later step fails
//! (all-or-nothing rollback). Handlers depend on the [`ObjectStore`] trait
//! (injected as `Extension<Arc<dyn ObjectStore>>`) so tests can substitute a
//! mock that records calls and forces failures without a live MinIO.
//!
//! The real [`S3ObjectStore`] mirrors the worker's `S3Client`
//! (`backend/crates/worker/src/storage.rs`): a thin `aws-sdk-s3` v1 wrapper
//! configured for MinIO (custom endpoint, static credentials,
//! `force_path_style`).

use aws_sdk_s3::config::Region;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::{Client as S3SdkClient, Config as S3SdkConfig};

use gmrag_core::config::S3Config;

/// Object storage operations used by the document endpoints.
///
/// Errors are returned as `String` so the trait stays free of SDK-specific
/// error types (keeps mocks trivial). The handler maps them to `ApiError`.
#[async_trait::async_trait]
pub trait ObjectStore: Send + Sync {
    /// Upload raw bytes under `key` with the given content type.
    async fn put(&self, key: &str, data: Vec<u8>, content_type: &str) -> Result<(), String>;

    /// Delete the object at `key`. Idempotent (S3 returns success even if the
    /// key is already absent) — used for rollback of a partial upload.
    async fn delete(&self, key: &str) -> Result<(), String>;

    /// T84D Phase 2.2 (SEC-4): best-effort delete of every object under
    /// `prefix` (e.g. `{tenant_id}/`). Used by the tenant-teardown path.
    /// The default impl returns `Err("not implemented")` so the mock in
    /// tests stays trivial. Implementations paginate `list_objects_v2`
    /// and call `delete_objects` in batches of 1000.
    async fn delete_prefix(&self, prefix: &str) -> Result<(), String> {
        let _ = prefix;
        Err("delete_prefix not implemented for this ObjectStore".into())
    }
}

/// S3-compatible object storage (MinIO in dev, any S3 in prod).
pub struct S3ObjectStore {
    client: S3SdkClient,
    bucket: String,
}

impl S3ObjectStore {
    /// Build a client from the application's [`S3Config`] (static credentials,
    /// custom endpoint, `force_path_style` from config).
    pub fn new(cfg: &S3Config) -> Self {
        let creds = aws_sdk_s3::config::Credentials::new(
            &cfg.access_key,
            &cfg.secret_key,
            None,
            None,
            "static",
        );
        let config = S3SdkConfig::builder()
            .behavior_version_latest()
            .region(Region::new(cfg.region.clone()))
            .endpoint_url(&cfg.endpoint)
            .credentials_provider(creds)
            .force_path_style(cfg.force_path_style)
            .build();
        Self {
            client: S3SdkClient::from_conf(config),
            bucket: cfg.bucket.clone(),
        }
    }
}

#[async_trait::async_trait]
impl ObjectStore for S3ObjectStore {
    async fn put(&self, key: &str, data: Vec<u8>, content_type: &str) -> Result<(), String> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(ByteStream::from(data))
            .content_type(content_type)
            .send()
            .await
            .map_err(|e| format!("S3 put_object '{key}' failed: {e}"))?;
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<(), String> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| format!("S3 delete_object '{key}' failed: {e}"))?;
        Ok(())
    }

    /// T84D Phase 2.2 (SEC-4): paginated S3 prefix delete used by the
    /// tenant teardown path. Lists objects under `prefix` and deletes
    /// them in batches of 1000 (the AWS `delete_objects` cap).
    async fn delete_prefix(&self, prefix: &str) -> Result<(), String> {
        use aws_sdk_s3::types::ObjectIdentifier;

        let mut continuation: Option<String> = None;
        loop {
            let mut list = self.client.list_objects_v2().bucket(&self.bucket).prefix(prefix);
            if let Some(token) = continuation.as_deref() {
                list = list.continuation_token(token);
            }
            let resp = list
                .send()
                .await
                .map_err(|e| format!("S3 list_objects_v2 '{prefix}' failed: {e}"))?;

            let keys: Vec<ObjectIdentifier> = resp
                .contents()
                .iter()
                .filter_map(|o| o.key().map(|k| k.to_string()))
                .map(|k| ObjectIdentifier::builder().key(k).build().map_err(|e| e.to_string()))
                .collect::<Result<Vec<_>, _>>()?;

            if !keys.is_empty() {
                self.client
                    .delete_objects()
                    .bucket(&self.bucket)
                    .set_delete(Some(
                        aws_sdk_s3::types::Delete::builder()
                            .set_objects(Some(keys))
                            .build()
                            .map_err(|e| format!("S3 delete batch build: {e}"))?,
                    ))
                    .send()
                    .await
                    .map_err(|e| format!("S3 delete_objects '{prefix}' failed: {e}"))?;
            }

            if resp.is_truncated().unwrap_or(false) {
                continuation = resp.next_continuation_token().map(|s| s.to_string());
            } else {
                break;
            }
        }
        Ok(())
    }
}
