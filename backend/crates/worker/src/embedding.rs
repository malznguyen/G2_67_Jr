//! Ollama embedding client: batched `/api/embed` with retry/backoff.
//!
//! T39: port of v1 `embedding.rs` + retry/backoff logic from v1 `processor.rs`.
//! The embedder is a concrete type (`OllamaEmbedder`); T40 introduces the
//! `Embedder` trait + `OpenAiEmbedder` + factory for BYOK.

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

/// Errors emitted by the Ollama embedder.
#[derive(Debug, Error)]
pub enum EmbedError {
    #[error("ollama embedding request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("ollama embedding timed out after {0}s")]
    Timeout(u64),
    #[error("ollama returned an empty embedding")]
    Empty,
    #[error("ollama returned {actual} embeddings for {expected} requested texts")]
    CountMismatch { expected: usize, actual: usize },
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

        let results = stream::iter(batches.into_iter().map(|(start, batch)| {
            async move {
                let embeddings = self.embed_batch_with_retry(&batch).await?;
                Ok::<_, EmbedError>((start, embeddings))
            }
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
    async fn embed_batch_with_retry(
        &self,
        texts: &[String],
    ) -> Result<Vec<Vec<f32>>, EmbedError> {
        let body = OllamaEmbedRequest {
            model: self.model.clone(),
            input: texts,
        };
        let timeout_secs = self.timeout.as_secs();

        let mut last_error: Option<EmbedError> = None;
        for attempt in 0..=self.retries {
            let result = tokio::time::timeout(
                self.timeout,
                self.client.post(&self.url).json(&body).send(),
            )
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

#[derive(serde::Serialize)]
struct OllamaEmbedRequest<'a> {
    model: String,
    input: &'a [String],
}

#[derive(serde::Deserialize)]
struct OllamaEmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    use serde_json::json;

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
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({
                    "model": "nomic-embed-text",
                    "embeddings": [vec768(0.1)],
                })),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/embed"))
            .and(body_partial_json(json!({ "input": ["second"] })))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({
                    "model": "nomic-embed-text",
                    "embeddings": [vec768(0.2)],
                })),
            )
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
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({
                    "model": "nomic-embed-text",
                    "embeddings": [vec768(0.9)],
                })),
            )
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
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(3)).set_body_json(json!({
                "model": "nomic-embed-text",
                "embeddings": [vec768(0.0)],
            })))
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
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({
                    "model": "nomic-embed-text",
                    "embeddings": [vec768(0.1)],
                })),
            )
            .mount(&server)
            .await;

        let embedder = embedder_at(&server).with_batch_size(2).with_retries(0);
        let err = embedder
            .embed_batch(&["a".to_string(), "b".to_string()])
            .await
            .expect_err("must detect count mismatch");
        assert!(
            matches!(err, EmbedError::CountMismatch { expected: 2, actual: 1 }),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn ollama_embed_batch_empty_input_returns_empty() {
        let server = MockServer::start().await;
        // No mock mounted — any call would fail. Empty input must short-circuit.
        let embedder = embedder_at(&server);
        let out = embedder.embed_batch(&[]).await.expect("empty -> Ok(vec![])");
        assert!(out.is_empty(), "empty input must produce empty output");
    }

    #[tokio::test]
    async fn ollama_embed_one_returns_single_vector() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/embed"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({
                    "model": "nomic-embed-text",
                    "embeddings": [vec768(0.5)],
                })),
            )
            .mount(&server)
            .await;

        let embedder = embedder_at(&server).with_batch_size(1).with_retries(0);
        let v = embedder.embed_one("hello").await.expect("must succeed");
        assert_eq!(v.len(), 768);
        assert_eq!(v[0], 0.5);
    }
}
