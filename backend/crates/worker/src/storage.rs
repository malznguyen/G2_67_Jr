//! S3 / MinIO object storage client.
//!
//! T35: thin wrapper over `aws-sdk-s3` v1 configured for MinIO
//! (custom endpoint, static credentials, `force_path_style = true`).
//! Tests use `wiremock` to mock the S3 REST API — no live MinIO required.

use aws_sdk_s3::config::Region;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client as S3SdkClient;
use aws_sdk_s3::Config;

use gmrag_core::config::S3Config;

/// S3-compatible object storage client (MinIO in dev, any S3 in prod).
pub struct S3Client {
    client: S3SdkClient,
    bucket: String,
}

impl S3Client {
    /// Build a client from the application's [`S3Config`].
    ///
    /// Uses static credentials and a custom endpoint URL — no AWS environment
    /// discovery. `force_path_style` is set from config (MinIO requires `true`).
    pub fn new(cfg: &S3Config) -> Self {
        let creds = aws_sdk_s3::config::Credentials::new(
            &cfg.access_key,
            &cfg.secret_key,
            None,
            None,
            "static",
        );
        let config = Config::builder()
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

    /// Download an object as raw bytes.
    ///
    /// Returns `Err` if the key does not exist (404) or the request fails.
    pub async fn download(&self, key: &str) -> anyhow::Result<Vec<u8>> {
        let resp = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("S3 get_object '{key}' failed: {e}"))?;
        let bytes = resp
            .body
            .collect()
            .await
            .map_err(|e| anyhow::anyhow!("S3 body read '{key}' failed: {e}"))?;
        Ok(bytes.into_bytes().to_vec())
    }

    /// Upload raw bytes with a content type.
    pub async fn upload(&self, key: &str, data: Vec<u8>, content_type: &str) -> anyhow::Result<()> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(ByteStream::from(data))
            .content_type(content_type)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("S3 put_object '{key}' failed: {e}"))?;
        Ok(())
    }

    /// Delete an object. Idempotent — S3 returns 204 even if the key was
    /// already absent.
    pub async fn delete(&self, key: &str) -> anyhow::Result<()> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("S3 delete_object '{key}' failed: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;

    fn test_s3_config(endpoint: String) -> S3Config {
        S3Config {
            endpoint,
            public_endpoint: "http://localhost:9000".into(),
            region: "us-east-1".into(),
            access_key: "test-ak".into(),
            secret_key: "test-sk".into(),
            bucket: "gmrag-uploads".into(),
            force_path_style: true,
        }
    }

    #[tokio::test]
    async fn storage_upload_then_download_roundtrip() {
        let server = MockServer::start().await;
        let cfg = test_s3_config(server.uri());
        let client = S3Client::new(&cfg);

        Mock::given(method("PUT"))
            .and(path("/gmrag-uploads/roundtrip-test"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/gmrag-uploads/roundtrip-test"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hello s3 roundtrip".to_vec()))
            .mount(&server)
            .await;

        client
            .upload(
                "roundtrip-test",
                b"hello s3 roundtrip".to_vec(),
                "text/plain",
            )
            .await
            .expect("upload should succeed");

        let downloaded = client
            .download("roundtrip-test")
            .await
            .expect("download should succeed");

        assert_eq!(downloaded, b"hello s3 roundtrip");
    }

    #[tokio::test]
    async fn storage_download_nonexistent_key_returns_error() {
        let server = MockServer::start().await;
        let cfg = test_s3_config(server.uri());
        let client = S3Client::new(&cfg);

        Mock::given(method("GET"))
            .and(path("/gmrag-uploads/nonexistent"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let result = client.download("nonexistent").await;
        assert!(result.is_err(), "404 must return Err");
    }

    #[tokio::test]
    async fn storage_delete_succeeds() {
        let server = MockServer::start().await;
        let cfg = test_s3_config(server.uri());
        let client = S3Client::new(&cfg);

        Mock::given(method("DELETE"))
            .and(path("/gmrag-uploads/to-delete"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;

        client
            .delete("to-delete")
            .await
            .expect("delete should succeed");
    }
}
