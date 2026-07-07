//! Embedding clients: Ollama (default) + OpenAI (BYOK), behind a shared trait.
//!
//! T39: `OllamaEmbedder` — `POST {host}/api/embed` with batch + retry/backoff.
//! T40: `Embedder` trait + `OpenAiEmbedder` (BYOK, `dimensions=768` pinned) +
//!      `select_embedder` factory that reads `tenant_llm_config` (RLS-scoped)
//!      to pick the right embedder per tenant.

use std::time::Duration;

use futures::stream::{self, StreamExt};
use gmrag_core::config::OllamaConfig;
use thiserror::Error;

const DEFAULT_BATCH_SIZE: usize = 32;
const DEFAULT_CONCURRENCY: usize = 2;
const DEFAULT_TIMEOUT_SECS: u64 = 120;
const DEFAULT_RETRIES: usize = 1;
const DEFAULT_BACKOFF_MS: u64 = 250;
const BACKOFF_CAP_POWER: u32 = 6;

/// Embedding dimension shared by all embedders.
///
/// Pinned to 768 to match `QdrantStore::EMBED_DIM` (core/src/qdrant/store.rs).
/// `OpenAiEmbedder` requests `dimensions=768` from `text-embedding-3-small`
/// (OpenAI supports shortening) so BYOK vectors fit existing collections
/// without re-embedding or per-tenant dimension tracking.
pub const EMBED_DIM: usize = 768;

/// Boxed future returned by [`Embedder::embed_batch`]. The `Send` bound
/// lets it be awaited from any tokio task; the `'a` tie lets the embedder
/// borrow `&[String]` without cloning.
pub type EmbedFuture<'a> = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<Vec<Vec<f32>>, EmbedError>> + Send + 'a>,
>;

/// Trait abstracting embedding providers so the worker can swap Ollama vs
/// OpenAI per-tenant (BYOK) without the call site caring.
pub trait Embedder: Send + Sync {
    /// Embed a slice of texts, returning vectors in input order.
    fn embed_batch<'a>(&'a self, texts: &'a [String]) -> EmbedFuture<'a>;

    /// Output vector dimension (always 768 in this project).
    fn dimension(&self) -> usize {
        EMBED_DIM
    }

    /// Provider name for logging/metrics ("ollama" | "openai").
    fn provider(&self) -> &str;
}

/// Ollama embedding client backed by `POST {host}/api/embed`.
///
/// Batches input texts (`batch_size`), runs batches concurrently
/// (`concurrency`), retries transient failures with exponential backoff
/// (`retries`, `backoff_ms * 2^attempt` capped at `2^6`), and enforces a
/// per-batch timeout. Output order matches input order regardless of
/// concurrency.
pub struct OllamaEmbedder {
    client: reqwest::Client,
    url: String,
    model: String,
    batch_size: usize,
    concurrency: usize,
    timeout: Duration,
    retries: usize,
    backoff_ms: u64,
}

/// Errors emitted by embedding clients.
#[derive(Debug, Error)]
pub enum EmbedError {
    #[error("embedding HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("embedding request timed out after {0}s")]
    Timeout(u64),
    #[error("embedding provider returned an empty embedding")]
    Empty,
    #[error("embedding provider returned {actual} embeddings for {expected} requested texts")]
    CountMismatch { expected: usize, actual: usize },
    #[error("embedding database error: {0}")]
    Db(String),
    #[error("BYOK api_key decrypt failed: {0}")]
    Decrypt(String),
}

impl OllamaEmbedder {
    /// Build an embedder from app config (`OllamaConfig.host` + `.embed_model`).
    pub fn new(cfg: &OllamaConfig) -> Self {
        Self::new_with_url(&cfg.host, &cfg.embed_model)
    }

    /// Build an embedder pointing at an explicit host URL (used by tests
    /// to point at a `wiremock` server). The `/api/embed` suffix is
    /// appended; a trailing slash on `host` is trimmed first.
    pub fn new_with_url(host: &str, model: &str) -> Self {
        let url = format!("{}/api/embed", host.trim_end_matches('/'));
        Self {
            client: reqwest::Client::new(),
            url,
            model: model.to_string(),
            batch_size: DEFAULT_BATCH_SIZE,
            concurrency: DEFAULT_CONCURRENCY,
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
            retries: DEFAULT_RETRIES,
            backoff_ms: DEFAULT_BACKOFF_MS,
        }
    }

    pub fn with_batch_size(mut self, n: usize) -> Self {
        self.batch_size = n.max(1);
        self
    }
    pub fn with_concurrency(mut self, n: usize) -> Self {
        self.concurrency = n.max(1);
        self
    }
    pub fn with_timeout_secs(mut self, secs: u64) -> Self {
        self.timeout = Duration::from_secs(secs.max(1));
        self
    }
    pub fn with_retries(mut self, n: usize) -> Self {
        self.retries = n;
        self
    }
    pub fn with_backoff_ms(mut self, ms: u64) -> Self {
        self.backoff_ms = ms;
        self
    }

    pub fn url(&self) -> &str {
        &self.url
    }
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Embed a single text. Convenience wrapper around [`embed_batch`].
    pub async fn embed_one(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
        let mut out = self.embed_batch(&[text.to_string()]).await?;
        out.pop().ok_or(EmbedError::Empty)
    }

    /// Embed a slice of texts, returning vectors in input order.
    ///
    /// Splits `texts` into batches of `batch_size`, runs `concurrency`
    /// batches concurrently via `buffer_unordered`, and stitches results
    /// back into input order. Empty input short-circuits to `Ok(vec![])`
    /// without touching the network.
    pub async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let batch_size = self.batch_size;
        let batches: Vec<(usize, Vec<String>)> = texts
            .chunks(batch_size)
            .enumerate()
            .map(|(i, chunk)| (i * batch_size, chunk.to_vec()))
            .collect();

        let results = stream::iter(batches.into_iter().map(|(start, batch)| async move {
            let embeddings = self.embed_batch_with_retry(&batch).await?;
            Ok::<_, EmbedError>((start, embeddings))
        }))
        .buffer_unordered(self.concurrency)
        .collect::<Vec<_>>()
        .await;

        let mut ordered = vec![None; texts.len()];
        for result in results {
            let (start, embeddings) = result?;
            for (offset, emb) in embeddings.into_iter().enumerate() {
                let idx = start + offset;
                if idx < ordered.len() {
                    ordered[idx] = Some(emb);
                }
            }
        }
        ordered
            .into_iter()
            .map(|emb| emb.ok_or(EmbedError::Empty))
            .collect()
    }

    /// Send one batch to `/api/embed` with retry/backoff + per-attempt timeout.
    ///
    /// Retries `self.retries` times (so up to `retries + 1` total attempts)
    /// on any error (HTTP non-2xx, network error, or timeout). Backoff is
    /// `backoff_ms * 2^attempt` capped at `2^BACKOFF_CAP_POWER` to avoid
    /// multi-minute waits on long retry chains.
    async fn embed_batch_with_retry(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        let body = OllamaEmbedRequest {
            model: self.model.clone(),
            input: texts,
        };
        let timeout_secs = self.timeout.as_secs();

        let mut last_error: Option<EmbedError> = None;
        for attempt in 0..=self.retries {
            let result =
                tokio::time::timeout(self.timeout, self.client.post(&self.url).json(&body).send())
                    .await;

            let outcome: Result<Vec<Vec<f32>>, EmbedError> = match result {
                Ok(Ok(resp)) => match resp.error_for_status() {
                    Ok(ok_resp) => match ok_resp.json::<OllamaEmbedResponse>().await {
                        Ok(parsed) => {
                            if parsed.embeddings.len() != texts.len() {
                                Err(EmbedError::CountMismatch {
                                    expected: texts.len(),
                                    actual: parsed.embeddings.len(),
                                })
                            } else if parsed.embeddings.iter().any(Vec::is_empty) {
                                Err(EmbedError::Empty)
                            } else {
                                Ok(parsed.embeddings)
                            }
                        }
                        Err(e) => Err(EmbedError::Http(e)),
                    },
                    Err(e) => Err(EmbedError::Http(e)),
                },
                Ok(Err(e)) => Err(EmbedError::Http(e)),
                Err(_) => Err(EmbedError::Timeout(timeout_secs)),
            };

            match outcome {
                Ok(embeddings) => return Ok(embeddings),
                Err(e) => last_error = Some(e),
            }

            if attempt < self.retries {
                let pow = attempt.min(BACKOFF_CAP_POWER as usize) as u32;
                let delay_ms = self.backoff_ms.saturating_mul(2_u64.saturating_pow(pow));
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
        }

        Err(last_error.unwrap_or(EmbedError::Timeout(timeout_secs)))
    }
}

impl Embedder for OllamaEmbedder {
    fn embed_batch<'a>(
        &'a self,
        texts: &'a [String],
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<Vec<f32>>, EmbedError>> + Send + 'a>,
    > {
        Box::pin(async move { OllamaEmbedder::embed_batch(self, texts).await })
    }

    fn provider(&self) -> &str {
        "ollama"
    }
}

// =========================================================
// T40: OpenAI BYOK embedder (text-embedding-3-small, dim 768)
// =========================================================

const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
const DEFAULT_OPENAI_TIMEOUT_SECS: u64 = 60;
const OPENAI_MODEL: &str = "text-embedding-3-small";

/// OpenAI embedding client (BYOK). Uses `text-embedding-3-small` with
/// `dimensions=768` pinned so output vectors fit the shared Qdrant
/// collection schema (768-dim Cosine) without re-embedding or per-tenant
/// dimension tracking.
pub struct OpenAiEmbedder {
    client: reqwest::Client,
    url: String,
    api_key: String,
    model: String,
    batch_size: usize,
    concurrency: usize,
    timeout: Duration,
    retries: usize,
    backoff_ms: u64,
}

impl OpenAiEmbedder {
    /// Build from a tenant's BYOK config. `base_url` falls back to the
    /// OpenAI default when `None`; `model` falls back to
    /// `text-embedding-3-small` when empty.
    pub fn new(api_key: String, model: &str, base_url: Option<&str>) -> Self {
        let base = base_url
            .map(|b| b.trim_end_matches('/').to_string())
            .filter(|b| !b.is_empty())
            .unwrap_or_else(|| DEFAULT_OPENAI_BASE_URL.to_string());
        let model = if model.is_empty() {
            OPENAI_MODEL.to_string()
        } else {
            model.to_string()
        };
        Self {
            client: reqwest::Client::new(),
            url: format!("{base}/embeddings"),
            api_key,
            model,
            batch_size: DEFAULT_BATCH_SIZE,
            concurrency: DEFAULT_CONCURRENCY,
            timeout: Duration::from_secs(DEFAULT_OPENAI_TIMEOUT_SECS),
            retries: DEFAULT_RETRIES,
            backoff_ms: DEFAULT_BACKOFF_MS,
        }
    }

    pub fn with_batch_size(mut self, n: usize) -> Self {
        self.batch_size = n.max(1);
        self
    }
    pub fn with_concurrency(mut self, n: usize) -> Self {
        self.concurrency = n.max(1);
        self
    }
    pub fn with_timeout_secs(mut self, secs: u64) -> Self {
        self.timeout = Duration::from_secs(secs.max(1));
        self
    }
    pub fn with_retries(mut self, n: usize) -> Self {
        self.retries = n;
        self
    }
    pub fn with_backoff_ms(mut self, ms: u64) -> Self {
        self.backoff_ms = ms;
        self
    }

    pub fn url(&self) -> &str {
        &self.url
    }
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Embed a single text. Convenience wrapper around the trait's
    /// `embed_batch`.
    pub async fn embed_one(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
        let out = self.embed_batch(&[text.to_string()]).await?;
        out.into_iter().next().ok_or(EmbedError::Empty)
    }

    async fn embed_batch_with_retry(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        let body = OpenAiEmbedRequest {
            model: self.model.clone(),
            input: texts,
            dimensions: EMBED_DIM,
        };
        let timeout_secs = self.timeout.as_secs();

        let mut last_error: Option<EmbedError> = None;
        for attempt in 0..=self.retries {
            let result = tokio::time::timeout(
                self.timeout,
                self.client
                    .post(&self.url)
                    .bearer_auth(&self.api_key)
                    .json(&body)
                    .send(),
            )
            .await;

            let outcome: Result<Vec<Vec<f32>>, EmbedError> = match result {
                Ok(Ok(resp)) => match resp.error_for_status() {
                    Ok(ok_resp) => match ok_resp.json::<OpenAiEmbedResponse>().await {
                        Ok(parsed) => {
                            let embeddings: Vec<Vec<f32>> =
                                parsed.data.into_iter().map(|d| d.embedding).collect();
                            if embeddings.len() != texts.len() {
                                Err(EmbedError::CountMismatch {
                                    expected: texts.len(),
                                    actual: embeddings.len(),
                                })
                            } else if embeddings.iter().any(Vec::is_empty) {
                                Err(EmbedError::Empty)
                            } else {
                                Ok(embeddings)
                            }
                        }
                        Err(e) => Err(EmbedError::Http(e)),
                    },
                    Err(e) => Err(EmbedError::Http(e)),
                },
                Ok(Err(e)) => Err(EmbedError::Http(e)),
                Err(_) => Err(EmbedError::Timeout(timeout_secs)),
            };

            match outcome {
                Ok(embeddings) => return Ok(embeddings),
                Err(e) => last_error = Some(e),
            }

            if attempt < self.retries {
                let pow = attempt.min(BACKOFF_CAP_POWER as usize) as u32;
                let delay_ms = self.backoff_ms.saturating_mul(2_u64.saturating_pow(pow));
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
        }

        Err(last_error.unwrap_or(EmbedError::Timeout(timeout_secs)))
    }
}

impl Embedder for OpenAiEmbedder {
    fn embed_batch<'a>(
        &'a self,
        texts: &'a [String],
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<Vec<f32>>, EmbedError>> + Send + 'a>,
    > {
        Box::pin(async move {
            if texts.is_empty() {
                return Ok(Vec::new());
            }
            let batch_size = self.batch_size;
            let batches: Vec<(usize, Vec<String>)> = texts
                .chunks(batch_size)
                .enumerate()
                .map(|(i, chunk)| (i * batch_size, chunk.to_vec()))
                .collect();

            let results = stream::iter(batches.into_iter().map(|(start, batch)| async move {
                let embeddings = self.embed_batch_with_retry(&batch).await?;
                Ok::<_, EmbedError>((start, embeddings))
            }))
            .buffer_unordered(self.concurrency)
            .collect::<Vec<_>>()
            .await;

            let mut ordered = vec![None; texts.len()];
            for result in results {
                let (start, embeddings) = result?;
                for (offset, emb) in embeddings.into_iter().enumerate() {
                    let idx = start + offset;
                    if idx < ordered.len() {
                        ordered[idx] = Some(emb);
                    }
                }
            }
            ordered
                .into_iter()
                .map(|emb| emb.ok_or(EmbedError::Empty))
                .collect()
        })
    }

    fn provider(&self) -> &str {
        "openai"
    }
}

// =========================================================
// T40: tenant_llm_config row + select_embedder factory
// =========================================================

/// Row from `tenant_llm_config` (RLS-scoped per tenant).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TenantLlmConfig {
    pub provider: String,
    pub api_key: Option<String>,
    pub api_key_ciphertext: Option<Vec<u8>>,
    pub api_key_nonce: Option<Vec<u8>>,
    pub model: String,
    pub base_url: Option<String>,
    pub dimensions: i32,
    pub enabled: bool,
}

/// Select the embedder for a tenant.
///
/// Reads `tenant_llm_config` inside an RLS-enforced transaction
/// (`SET LOCAL ROLE gmrag_app` + `SET LOCAL app.tenant_id`). If the
/// tenant has an enabled row with `provider='openai'` and a usable
/// API key (encrypted or plaintext), returns an `OpenAiEmbedder`;
/// otherwise falls back to the platform default `OllamaEmbedder`.
///
/// **Key resolution order** (mirrors `api/src/llm/byok.rs`):
/// 1. Encrypted fields present → decrypt via `core::crypto::decrypt_with_aad`
///    (AAD = `tenant_id` bytes). Requires `enc_key` (from
///    `GMRAG_TENANT_KEY_ENCRYPTION_KEY`). Fails if key is missing or
///    decryption fails — does NOT silently fall back.
/// 2. No encrypted fields → use plaintext `api_key` if non-empty (legacy).
/// 3. Neither → fall back to `OllamaEmbedder`.
///
/// This is the entry point the worker calls per ingest job (T42). The
/// pool passed in can be either `init_app_pool` (production — role
/// already `gmrag_app`) or a superuser test pool — `SET LOCAL ROLE
/// gmrag_app` handles both.
pub async fn select_embedder(
    pool: &sqlx::PgPool,
    tenant_id: uuid::Uuid,
    ollama_cfg: &OllamaConfig,
    enc_key: Option<&[u8; 32]>,
) -> Result<Box<dyn Embedder>, EmbedError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| EmbedError::Db(e.to_string()))?;

    // RLS: downgrade to app role + set tenant context for this tx.
    sqlx::Executor::execute(&mut *tx, "SET LOCAL ROLE gmrag_app")
        .await
        .map_err(|e| EmbedError::Db(e.to_string()))?;
    sqlx::query(&format!("SET LOCAL app.tenant_id = '{tenant_id}'"))
        .execute(&mut *tx)
        .await
        .map_err(|e| EmbedError::Db(e.to_string()))?;

    let row = sqlx::query_as::<_, TenantLlmConfig>(
        r#"
        SELECT provider, api_key, api_key_ciphertext, api_key_nonce,
               model, base_url, dimensions, enabled
        FROM tenant_llm_config
        WHERE enabled = true
        "#,
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| EmbedError::Db(e.to_string()))?;

    tx.commit()
        .await
        .map_err(|e| EmbedError::Db(e.to_string()))?;

    match row {
        Some(cfg) if cfg.provider == "openai" => {
            let api_key = resolve_byok_api_key(
                cfg.api_key_ciphertext.as_deref(),
                cfg.api_key_nonce.as_deref(),
                cfg.api_key.as_deref(),
                enc_key,
                tenant_id,
            )?;
            match api_key {
                Some(key) => Ok(Box::new(OpenAiEmbedder::new(
                    key,
                    &cfg.model,
                    cfg.base_url.as_deref(),
                ))),
                None => Ok(Box::new(OllamaEmbedder::new(ollama_cfg))),
            }
        }
        _ => Ok(Box::new(OllamaEmbedder::new(ollama_cfg))),
    }
}

/// Resolve a tenant's BYOK API key from `tenant_llm_config` columns.
///
/// Encrypted fields take priority over plaintext. If encrypted fields
/// are present but `enc_key` is `None` or decryption fails, returns an
/// error — does NOT silently fall back to plaintext or global defaults.
/// This matches the security invariant in `api/src/llm/byok.rs`.
fn resolve_byok_api_key(
    ciphertext: Option<&[u8]>,
    nonce: Option<&[u8]>,
    plaintext_key: Option<&str>,
    enc_key: Option<&[u8; 32]>,
    tenant_id: uuid::Uuid,
) -> Result<Option<String>, EmbedError> {
    match (ciphertext, nonce) {
        (Some(ct), Some(n)) => {
            let key = enc_key.ok_or_else(|| {
                EmbedError::Decrypt(
                    "encrypted BYOK key present but GMRAG_TENANT_KEY_ENCRYPTION_KEY not configured"
                        .into(),
                )
            })?;
            let decrypted = gmrag_core::crypto::decrypt_with_aad(ct, n, key, tenant_id.as_bytes())
                .map_err(|e| EmbedError::Decrypt(e.to_string()))?;
            Ok(Some(decrypted))
        }
        (None, None) => Ok(plaintext_key
            .filter(|v| !v.trim().is_empty())
            .map(|s| s.to_string())),
        _ => Err(EmbedError::Decrypt(
            "encrypted key pair is incomplete (one field NULL)".into(),
        )),
    }
}

#[derive(serde::Serialize)]
struct OllamaEmbedRequest<'a> {
    model: String,
    input: &'a [String],
}

#[derive(serde::Deserialize)]
struct OllamaEmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

#[derive(serde::Serialize)]
struct OpenAiEmbedRequest<'a> {
    model: String,
    input: &'a [String],
    dimensions: usize,
}

#[derive(serde::Deserialize)]
struct OpenAiEmbedResponse {
    data: Vec<OpenAiEmbedDatum>,
}

#[derive(serde::Deserialize)]
struct OpenAiEmbedDatum {
    embedding: Vec<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn embedder_at(server: &MockServer) -> OllamaEmbedder {
        OllamaEmbedder::new_with_url(&server.uri(), "nomic-embed-text")
            .with_batch_size(2)
            .with_concurrency(2)
            .with_timeout_secs(5)
            .with_retries(2)
            .with_backoff_ms(5)
    }

    fn vec768(seed: f32) -> Vec<f32> {
        vec![seed; 768]
    }

    #[tokio::test]
    async fn ollama_embed_batch_returns_vectors_in_order() {
        let server = MockServer::start().await;
        let resp = ResponseTemplate::new(200).set_body_json(json!({
            "model": "nomic-embed-text",
            "embeddings": [vec768(0.1), vec768(0.2), vec768(0.3)],
        }));
        Mock::given(method("POST"))
            .and(path("/api/embed"))
            .respond_with(resp)
            .mount(&server)
            .await;

        // batch_size=3 so all 3 texts go in one batch; the mock returns
        // exactly 3 embeddings (count must match per-batch).
        let embedder = embedder_at(&server).with_batch_size(3);
        let texts = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let out = embedder.embed_batch(&texts).await.expect("must succeed");

        assert_eq!(out.len(), 3, "must return one vector per input");
        assert_eq!(out[0].len(), 768, "dimension must be 768");
        // Order preserved: each vector tagged with its seed via value.
        assert_eq!(out[0][0], 0.1);
        assert_eq!(out[1][0], 0.2);
        assert_eq!(out[2][0], 0.3);
    }

    #[tokio::test]
    async fn ollama_embed_batch_multi_batch_preserves_order() {
        // Two batches of 1 text each; separate mocks return distinct
        // single-embedding responses so we can assert order stitching.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/embed"))
            .and(body_partial_json(json!({ "input": ["first"] })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "nomic-embed-text",
                "embeddings": [vec768(0.1)],
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/embed"))
            .and(body_partial_json(json!({ "input": ["second"] })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "nomic-embed-text",
                "embeddings": [vec768(0.2)],
            })))
            .mount(&server)
            .await;

        let embedder = embedder_at(&server).with_batch_size(1).with_retries(0);
        let texts = vec!["first".to_string(), "second".to_string()];
        let out = embedder.embed_batch(&texts).await.expect("must succeed");

        assert_eq!(out.len(), 2);
        // Order preserved despite buffer_unordered concurrency.
        assert_eq!(out[0][0], 0.1, "first input -> first vector");
        assert_eq!(out[1][0], 0.2, "second input -> second vector");
    }

    #[tokio::test]
    async fn ollama_embed_batch_retries_on_500_then_succeeds() {
        let server = MockServer::start().await;
        // First call -> 500. Second call -> 200.
        Mock::given(method("POST"))
            .and(path("/api/embed"))
            .respond_with(ResponseTemplate::new(500))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/embed"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "nomic-embed-text",
                "embeddings": [vec768(0.9)],
            })))
            .mount(&server)
            .await;

        let embedder = embedder_at(&server).with_batch_size(1);
        let out = embedder
            .embed_batch(&["x".to_string()])
            .await
            .expect("retry must succeed");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0][0], 0.9);
    }

    #[tokio::test]
    async fn ollama_embed_batch_fails_after_max_retries() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/embed"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let embedder = embedder_at(&server).with_batch_size(1).with_retries(1);
        let err = embedder
            .embed_batch(&["x".to_string()])
            .await
            .expect_err("must fail after retries exhausted");
        // Either Http (500 status) — the retry path surfaces last error.
        assert!(matches!(err, EmbedError::Http(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn ollama_embed_batch_times_out() {
        let server = MockServer::start().await;
        // Respond after 3s, but timeout is 1s.
        Mock::given(method("POST"))
            .and(path("/api/embed"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_secs(3))
                    .set_body_json(json!({
                        "model": "nomic-embed-text",
                        "embeddings": [vec768(0.0)],
                    })),
            )
            .mount(&server)
            .await;

        let embedder = embedder_at(&server)
            .with_batch_size(1)
            .with_retries(0)
            .with_timeout_secs(1);
        let err = embedder
            .embed_batch(&["x".to_string()])
            .await
            .expect_err("must time out");
        assert!(matches!(err, EmbedError::Timeout(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn ollama_embed_batch_count_mismatch() {
        let server = MockServer::start().await;
        // Request 2 inputs, server returns only 1 embedding.
        Mock::given(method("POST"))
            .and(path("/api/embed"))
            .and(body_partial_json(json!({ "input": ["a", "b"] })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "nomic-embed-text",
                "embeddings": [vec768(0.1)],
            })))
            .mount(&server)
            .await;

        let embedder = embedder_at(&server).with_batch_size(2).with_retries(0);
        let err = embedder
            .embed_batch(&["a".to_string(), "b".to_string()])
            .await
            .expect_err("must detect count mismatch");
        assert!(
            matches!(
                err,
                EmbedError::CountMismatch {
                    expected: 2,
                    actual: 1
                }
            ),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn ollama_embed_batch_empty_input_returns_empty() {
        let server = MockServer::start().await;
        // No mock mounted — any call would fail. Empty input must short-circuit.
        let embedder = embedder_at(&server);
        let out = embedder
            .embed_batch(&[])
            .await
            .expect("empty -> Ok(vec![])");
        assert!(out.is_empty(), "empty input must produce empty output");
    }

    #[tokio::test]
    async fn ollama_embed_one_returns_single_vector() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/embed"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "nomic-embed-text",
                "embeddings": [vec768(0.5)],
            })))
            .mount(&server)
            .await;

        let embedder = embedder_at(&server).with_batch_size(1).with_retries(0);
        let v = embedder.embed_one("hello").await.expect("must succeed");
        assert_eq!(v.len(), 768);
        assert_eq!(v[0], 0.5);
    }

    // ---------- T40: OpenAiEmbedder (BYOK) ----------

    fn openai_embedder_at(server: &MockServer) -> OpenAiEmbedder {
        OpenAiEmbedder::new("sk-test-key".into(), "", Some(&server.uri()))
            .with_batch_size(2)
            .with_concurrency(2)
            .with_timeout_secs(5)
            .with_retries(2)
            .with_backoff_ms(5)
    }

    #[tokio::test]
    async fn openai_embedder_returns_768_dim_vectors() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/embeddings"))
            .and(body_partial_json(json!({ "dimensions": 768 })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [
                    { "embedding": vec768(0.1), "index": 0 },
                    { "embedding": vec768(0.2), "index": 1 },
                ]
            })))
            .mount(&server)
            .await;

        let embedder = openai_embedder_at(&server).with_batch_size(2);
        let out = embedder
            .embed_batch(&["a".to_string(), "b".to_string()])
            .await
            .expect("must succeed");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].len(), 768, "dimension must be pinned to 768");
        assert_eq!(out[0][0], 0.1);
        assert_eq!(out[1][0], 0.2);
    }

    #[tokio::test]
    async fn openai_embedder_sends_bearer_auth() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/embeddings"))
            .and(wiremock::matchers::header(
                "Authorization",
                "Bearer sk-test-key",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{ "embedding": vec768(0.7), "index": 0 }]
            })))
            .mount(&server)
            .await;

        let embedder = openai_embedder_at(&server)
            .with_batch_size(1)
            .with_retries(0);
        let v = embedder.embed_one("hello").await.expect("must succeed");
        assert_eq!(v[0], 0.7);
    }

    #[tokio::test]
    async fn openai_embedder_retries_on_5xx() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/embeddings"))
            .respond_with(ResponseTemplate::new(500))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{ "embedding": vec768(0.3), "index": 0 }]
            })))
            .mount(&server)
            .await;

        let embedder = openai_embedder_at(&server).with_batch_size(1);
        let v = embedder.embed_one("x").await.expect("retry must succeed");
        assert_eq!(v[0], 0.3);
    }

    #[tokio::test]
    async fn openai_embedder_trait_provider_and_dimension() {
        let server = MockServer::start().await;
        let embedder = openai_embedder_at(&server);
        assert_eq!(embedder.provider(), "openai");
        assert_eq!(embedder.dimension(), 768);
    }

    #[tokio::test]
    async fn ollama_embedder_trait_provider_and_dimension() {
        let server = MockServer::start().await;
        let embedder = embedder_at(&server);
        assert_eq!(embedder.provider(), "ollama");
        assert_eq!(embedder.dimension(), 768);
    }
}
