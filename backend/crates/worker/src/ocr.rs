//! OCR fallback: Ollama vision model for image-only / scanned PDF pages.
//!
//! T37: replaces v1's stub `vision_ocr_fallback` (which returned
//! `"mock_ocr_text"`) with a real Ollama vision client. The trait
//! abstraction allows tests to inject `MockOcr` without touching the
//! network.
//!
//! `OllamaVisionOcr` sends a base64-encoded image to Ollama's `/api/chat`
//! with a vision model (e.g. `moondream:1.8b`) and parses the text from
//! the response `message.content`.

use std::time::Duration;

use base64::Engine;
use thiserror::Error;

const DEFAULT_VISION_MODEL: &str = "moondream:1.8b";
const DEFAULT_VISION_TIMEOUT_SECS: u64 = 60;
const DEFAULT_VISION_RETRIES: usize = 1;
const DEFAULT_VISION_BACKOFF_MS: u64 = 250;
const BACKOFF_CAP_POWER: u32 = 6;

/// OCR prompt — instruct the vision model to extract all text.
const OCR_PROMPT: &str = "Extract all text from this image. Output only the text, nothing else.";

/// Trait abstracting OCR clients so the PDF parser can swap implementations.
pub trait OcrClient: Send + Sync {
    /// OCR an image (PNG bytes) and return the extracted text.
    fn ocr_image<'a>(
        &'a self,
        image_bytes: &'a [u8],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, OcrError>> + Send + 'a>>;
}

/// Errors emitted by OCR clients.
#[derive(Debug, Error)]
pub enum OcrError {
    #[error("ocr HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("ocr request timed out after {0}s")]
    Timeout(u64),
    #[error("ocr response missing message content")]
    Empty,
    #[error("ocr base64 encode error: {0}")]
    Base64(String),
}

/// Ollama vision OCR client. Sends base64-encoded images to
/// `POST {host}/api/chat` with a vision model and parses `message.content`.
pub struct OllamaVisionOcr {
    client: reqwest::Client,
    url: String,
    model: String,
    timeout: Duration,
    retries: usize,
    backoff_ms: u64,
}

impl OllamaVisionOcr {
    /// Build from app config. Uses `cfg.host` + `OLLAMA_VISION_MODEL`
    /// (falls back to `moondream:1.8b`).
    pub fn new(cfg: &gmrag_core::config::OllamaConfig) -> Self {
        Self::new_with_url(&cfg.host, DEFAULT_VISION_MODEL)
    }

    /// Build with an explicit host + model (used by tests to point at
    /// a wiremock server).
    pub fn new_with_url(host: &str, model: &str) -> Self {
        let url = format!("{}/api/chat", host.trim_end_matches('/'));
        let model = if model.is_empty() {
            DEFAULT_VISION_MODEL.to_string()
        } else {
            model.to_string()
        };
        Self {
            client: reqwest::Client::new(),
            url,
            model,
            timeout: Duration::from_secs(DEFAULT_VISION_TIMEOUT_SECS),
            retries: DEFAULT_VISION_RETRIES,
            backoff_ms: DEFAULT_VISION_BACKOFF_MS,
        }
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

    async fn ocr_with_retry(&self, image_bytes: &[u8]) -> Result<String, OcrError> {
        let b64 = base64::engine::general_purpose::STANDARD.encode(image_bytes);
        let body = serde_json::json!({
            "model": self.model,
            "messages": [
                {
                    "role": "user",
                    "content": OCR_PROMPT,
                    "images": [b64],
                }
            ],
            "stream": false,
        });
        let timeout_secs = self.timeout.as_secs();

        let mut last_error: Option<OcrError> = None;
        for attempt in 0..=self.retries {
            let result =
                tokio::time::timeout(self.timeout, self.client.post(&self.url).json(&body).send())
                    .await;

            let outcome: Result<String, OcrError> = match result {
                Ok(Ok(resp)) => match resp.error_for_status() {
                    Ok(ok_resp) => match ok_resp.json::<OllamaChatResponse>().await {
                        Ok(parsed) => {
                            let content = parsed.message.map(|m| m.content).unwrap_or_default();
                            if content.trim().is_empty() {
                                Err(OcrError::Empty)
                            } else {
                                Ok(content)
                            }
                        }
                        Err(e) => Err(OcrError::Http(e)),
                    },
                    Err(e) => Err(OcrError::Http(e)),
                },
                Ok(Err(e)) => Err(OcrError::Http(e)),
                Err(_) => Err(OcrError::Timeout(timeout_secs)),
            };

            match outcome {
                Ok(text) => return Ok(text),
                Err(e) => last_error = Some(e),
            }

            if attempt < self.retries {
                let pow = attempt.min(BACKOFF_CAP_POWER as usize) as u32;
                let delay_ms = self.backoff_ms.saturating_mul(2_u64.saturating_pow(pow));
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
        }

        Err(last_error.unwrap_or(OcrError::Timeout(timeout_secs)))
    }
}

impl OcrClient for OllamaVisionOcr {
    fn ocr_image<'a>(
        &'a self,
        image_bytes: &'a [u8],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, OcrError>> + Send + 'a>>
    {
        Box::pin(async move { self.ocr_with_retry(image_bytes).await })
    }
}

#[derive(serde::Deserialize)]
struct OllamaChatResponse {
    message: Option<OllamaChatMessage>,
}

#[derive(serde::Deserialize)]
struct OllamaChatMessage {
    content: String,
}

/// Mock OCR client for tests. Returns a canned string regardless of input.
pub struct MockOcr {
    text: String,
}

impl MockOcr {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

impl OcrClient for MockOcr {
    fn ocr_image<'a>(
        &'a self,
        _image_bytes: &'a [u8],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, OcrError>> + Send + 'a>>
    {
        let text = self.text.clone();
        Box::pin(async move { Ok(text) })
    }
}

/// Mock OCR client that panics if called. Used to verify OCR is NOT
/// invoked for text-based PDFs.
pub struct NoOcr;

impl OcrClient for NoOcr {
    fn ocr_image<'a>(
        &'a self,
        _image_bytes: &'a [u8],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, OcrError>> + Send + 'a>>
    {
        Box::pin(async {
            panic!("NoOcr::ocr_image called — OCR should not have been invoked for this page")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ocr_at(server: &MockServer) -> OllamaVisionOcr {
        OllamaVisionOcr::new_with_url(&server.uri(), "moondream:1.8b")
            .with_timeout_secs(5)
            .with_retries(2)
            .with_backoff_ms(5)
    }

    #[tokio::test]
    async fn ollama_vision_ocr_extracts_text_from_image() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "moondream:1.8b",
                "message": {
                    "role": "assistant",
                    "content": "This is extracted OCR text."
                }
            })))
            .mount(&server)
            .await;

        let ocr = ocr_at(&server).with_retries(0);
        let text = ocr
            .ocr_image(b"fake-png-bytes")
            .await
            .expect("must succeed");
        assert_eq!(text, "This is extracted OCR text.");
    }

    #[tokio::test]
    async fn ollama_vision_ocr_sends_base64_image() {
        let server = MockServer::start().await;
        // Match that the request body contains an "images" array with
        // a base64 string (the encoded fake-png-bytes).
        let expected_b64 = base64::engine::general_purpose::STANDARD.encode(b"fake-png-bytes");
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .and(body_partial_json(json!({
                "messages": [{"images": [expected_b64]}]
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "message": {"role": "assistant", "content": "ok"}
            })))
            .mount(&server)
            .await;

        let ocr = ocr_at(&server).with_retries(0);
        let text = ocr
            .ocr_image(b"fake-png-bytes")
            .await
            .expect("must succeed");
        assert_eq!(text, "ok");
    }

    #[tokio::test]
    async fn ollama_vision_ocr_retries_on_5xx() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(500))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "message": {"role": "assistant", "content": "recovered"}
            })))
            .mount(&server)
            .await;

        let ocr = ocr_at(&server);
        let text = ocr.ocr_image(b"img").await.expect("retry must succeed");
        assert_eq!(text, "recovered");
    }

    #[tokio::test]
    async fn ollama_vision_ocr_times_out() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_secs(3))
                    .set_body_json(json!({
                        "message": {"content": "late"}
                    })),
            )
            .mount(&server)
            .await;

        let ocr = ocr_at(&server).with_retries(0).with_timeout_secs(1);
        let err = ocr.ocr_image(b"img").await.expect_err("must time out");
        assert!(matches!(err, OcrError::Timeout(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn ollama_vision_ocr_empty_response_returns_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "message": {"content": ""}
            })))
            .mount(&server)
            .await;

        let ocr = ocr_at(&server).with_retries(0);
        let err = ocr
            .ocr_image(b"img")
            .await
            .expect_err("empty content must error");
        assert!(matches!(err, OcrError::Empty), "got {err:?}");
    }

    #[tokio::test]
    async fn mock_ocr_returns_canned_text() {
        let ocr = MockOcr::new("canned ocr result");
        let text = ocr.ocr_image(b"anything").await.expect("must succeed");
        assert_eq!(text, "canned ocr result");
    }
}
