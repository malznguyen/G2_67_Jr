//! Graph extraction — turn document text into typed nodes + edges via an
//! OpenAI-compatible LLM (DeepSeek by default, OpenAI BYOK per tenant).
//!
//! T41: `DeepSeekGraphExtractor` calls `{base_url}/chat/completions` with a
//! system prompt that forces JSON output, then a tolerant parser extracts
//! `{"nodes":[...],"edges":[...]}` even when the model wraps it in a
//! markdown fence or omits the `edges` array.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use gmrag_core::config::DeepSeekConfig;
use thiserror::Error;

const DEFAULT_TIMEOUT_SECS: u64 = 60;
const DEFAULT_RETRIES: usize = 1;
const DEFAULT_BACKOFF_MS: u64 = 250;
const BACKOFF_CAP_POWER: u32 = 6;

const SYSTEM_PROMPT: &str = "You are a knowledge-graph extractor. Read the user text and return ONLY a JSON object with the exact shape {\"nodes\":[{\"kind\":string,\"label\":string,\"description\":string}],\"edges\":[{\"source\":string,\"target\":string,\"kind\":string}]}. Do not wrap the JSON in markdown fences. Do not add commentary. If no entities are found, return {\"nodes\":[],\"edges\":[]}.";

/// A single extracted entity (graph node candidate).
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct ExtractedNode {
    pub kind: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
}

/// A single extracted relationship (graph edge candidate). `source`/`target`
/// reference node labels produced in the same extraction.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct ExtractedEdge {
    pub source: String,
    pub target: String,
    pub kind: String,
}

/// Output of a single graph extraction call.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct GraphExtraction {
    #[serde(default)]
    pub nodes: Vec<ExtractedNode>,
    #[serde(default)]
    pub edges: Vec<ExtractedEdge>,
}

/// Errors emitted by graph extraction.
#[derive(Debug, Error)]
pub enum GraphExtractError {
    #[error("graph extraction HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("graph extraction request timed out after {0}s")]
    Timeout(u64),
    #[error("llm returned an empty completion")]
    Empty,
    #[error("graph extraction json parse error: {0}")]
    Parse(String),
    #[error("graph extraction database error: {0}")]
    Db(String),
    #[error("BYOK api_key decrypt failed: {0}")]
    Decrypt(String),
}

type ExFuture<'a> =
    Pin<Box<dyn Future<Output = Result<GraphExtraction, GraphExtractError>> + Send + 'a>>;

/// Trait abstracting graph extractors so the worker can swap LLM backends.
pub trait GraphExtractor: Send + Sync {
    fn extract<'a>(&'a self, text: &'a str) -> ExFuture<'a>;
    /// The chat completions endpoint URL the extractor calls (for
    /// diagnostics / tests).
    fn url(&self) -> &str;
    /// The model name used for completion.
    fn model(&self) -> &str;
}

/// DeepSeek (OpenAI-compatible) graph extractor.
pub struct DeepSeekGraphExtractor {
    client: reqwest::Client,
    url: String,
    api_key: Option<String>,
    model: String,
    timeout: Duration,
    retries: usize,
    backoff_ms: u64,
}

impl DeepSeekGraphExtractor {
    /// Build from the global `DeepSeekConfig`.
    pub fn new(cfg: &DeepSeekConfig) -> Self {
        Self::new_with(&cfg.base_url, cfg.api_key.as_deref(), &cfg.model)
    }

    /// Build pointing at an explicit base URL (used by tests → wiremock).
    pub fn new_with(base_url: &str, api_key: Option<&str>, model: &str) -> Self {
        Self {
            client: reqwest::Client::new(),
            url: format!("{}/chat/completions", base_url.trim_end_matches('/')),
            api_key: api_key.map(|s| s.to_string()),
            model: model.to_string(),
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
            retries: DEFAULT_RETRIES,
            backoff_ms: DEFAULT_BACKOFF_MS,
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

    /// Call the LLM with retry/backoff, returning the raw completion text.
    ///
    /// Retries `self.retries` times (up to `retries + 1` total attempts) on
    /// any error. Backoff is `backoff_ms * 2^attempt` capped at
    /// `2^BACKOFF_CAP_POWER` (16s with default 250ms) — mirrors T39 embedding.
    async fn complete(&self, user_text: &str) -> Result<String, GraphExtractError> {
        let body = ChatRequest {
            model: &self.model,
            messages: vec![
                ChatMessage {
                    role: "system",
                    content: SYSTEM_PROMPT,
                },
                ChatMessage {
                    role: "user",
                    content: user_text,
                },
            ],
            stream: false,
            response_format: ResponseFormat { fmt: "json_object" },
        };
        let timeout_secs = self.timeout.as_secs();
        let mut last_error: Option<GraphExtractError> = None;

        for attempt in 0..=self.retries {
            let mut req = self.client.post(&self.url).json(&body);
            if let Some(key) = &self.api_key {
                req = req.bearer_auth(key);
            }
            let result = tokio::time::timeout(self.timeout, req.send()).await;
            let outcome: Result<String, GraphExtractError> = match result {
                Ok(Ok(resp)) => match resp.error_for_status() {
                    Ok(ok_resp) => match ok_resp.json::<ChatResponse>().await {
                        Ok(parsed) => parsed
                            .choices
                            .into_iter()
                            .next()
                            .map(|c| c.message.content)
                            .filter(|c| !c.trim().is_empty())
                            .ok_or(GraphExtractError::Empty),
                        Err(e) => Err(GraphExtractError::Http(e)),
                    },
                    Err(e) => Err(GraphExtractError::Http(e)),
                },
                Ok(Err(e)) => Err(GraphExtractError::Http(e)),
                Err(_) => Err(GraphExtractError::Timeout(timeout_secs)),
            };

            match outcome {
                Ok(content) => return Ok(content),
                Err(e) => last_error = Some(e),
            }

            if attempt < self.retries {
                let pow = attempt.min(BACKOFF_CAP_POWER as usize) as u32;
                let delay_ms = self.backoff_ms.saturating_mul(2_u64.saturating_pow(pow));
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
        }
        Err(last_error.unwrap_or(GraphExtractError::Timeout(timeout_secs)))
    }
}

impl GraphExtractor for DeepSeekGraphExtractor {
    fn extract<'a>(&'a self, text: &'a str) -> ExFuture<'a> {
        Box::pin(async move { self.extract_inner(text).await })
    }

    fn url(&self) -> &str {
        self.url.as_str()
    }

    fn model(&self) -> &str {
        self.model.as_str()
    }
}

impl DeepSeekGraphExtractor {
    async fn extract_inner(&self, text: &str) -> Result<GraphExtraction, GraphExtractError> {
        let raw = self.complete(text).await?;
        parse_graph_json(&raw)
    }
}

/// Tolerant JSON parser: strips markdown fences and parses the first JSON
/// object found in `raw`. Missing `edges` defaults to empty.
///
/// DeepSeek occasionally wraps the JSON object in a ```json ... ``` fence or
/// appends trailing prose despite the system prompt. This parser:
/// 1. Trims surrounding whitespace.
/// 2. If a ``` fence is present, extracts the fenced segment.
/// 3. Otherwise locates the first `{` and the matching last `}`.
/// 4. Deserializes into [`RawExtraction`] (both arrays default to empty).
pub fn parse_graph_json(raw: &str) -> Result<GraphExtraction, GraphExtractError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(GraphExtractError::Parse("empty completion".into()));
    }

    let candidate = extract_json_object(trimmed)
        .ok_or_else(|| GraphExtractError::Parse("no JSON object found in completion".into()))?;

    let parsed: RawExtraction =
        serde_json::from_str(candidate).map_err(|e| GraphExtractError::Parse(format!("{e}")))?;

    Ok(GraphExtraction {
        nodes: parsed.nodes,
        edges: parsed.edges,
    })
}

/// Locate the first balanced JSON object in `s`. Handles markdown fences by
/// stripping a leading ```json fence and trailing ```. Falls back to the
/// substring between the first `{` and the last `}`.
fn extract_json_object(s: &str) -> Option<&str> {
    // Strip a markdown code fence if present.
    let after_fence = if let Some(start) = s.find("```") {
        let rest = &s[start + 3..];
        // skip optional language tag (e.g. `json`) up to newline
        let nl = rest.find('\n').map(|n| n + 1).unwrap_or(0);
        let body = &rest[nl..];
        if let Some(end) = body.rfind("```") {
            &body[..end]
        } else {
            body
        }
    } else {
        s
    };

    let start = after_fence.find('{')?;
    let end = after_fence.rfind('}')?;
    if end < start {
        return None;
    }
    Some(&after_fence[start..=end])
}

// =========================================================
// T41: BYOK graph extractor factory (2-layer fallback)
// =========================================================

const DEFAULT_OPENAI_CHAT_BASE_URL: &str = "https://api.openai.com/v1";

/// Row from `tenant_llm_config` used to pick the LLM for graph extraction.
#[derive(Debug, Clone, sqlx::FromRow)]
struct TenantLlmRow {
    provider: String,
    api_key: Option<String>,
    api_key_ciphertext: Option<Vec<u8>>,
    api_key_nonce: Option<Vec<u8>>,
    llm_model: Option<String>,
    llm_base_url: Option<String>,
}

/// Select a graph extractor for a tenant using the 2-layer fallback:
/// 1. If the tenant has an enabled `tenant_llm_config` row with a usable
///    API key (encrypted or plaintext) AND an `llm_model` override, build
///    a BYOK extractor pointed at `llm_base_url` with `llm_model`.
/// 2. Otherwise fall back to the global `DeepSeekConfig`.
///
/// **Key resolution** mirrors `select_embedder`: encrypted fields take
/// priority, then plaintext `api_key` legacy fallback. If encrypted
/// fields are present but `enc_key` is `None` or decrypt fails, returns
/// an error — does NOT silently fall back.
///
/// RLS is enforced inside an explicit transaction (`SET LOCAL ROLE
/// gmrag_app` + `SET LOCAL app.tenant_id`), mirroring `select_embedder`.
pub async fn select_graph_extractor(
    pool: &sqlx::PgPool,
    tenant_id: uuid::Uuid,
    global_cfg: &DeepSeekConfig,
    enc_key: Option<&[u8; 32]>,
) -> Result<Box<dyn GraphExtractor>, GraphExtractError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| GraphExtractError::Db(e.to_string()))?;
    sqlx::Executor::execute(&mut *tx, "SET LOCAL ROLE gmrag_app")
        .await
        .map_err(|e| GraphExtractError::Db(e.to_string()))?;
    sqlx::query(&format!("SET LOCAL app.tenant_id = '{tenant_id}'"))
        .execute(&mut *tx)
        .await
        .map_err(|e| GraphExtractError::Db(e.to_string()))?;

    let row = sqlx::query_as::<_, TenantLlmRow>(
        r#"
        SELECT provider, api_key, api_key_ciphertext, api_key_nonce,
               llm_model, llm_base_url
        FROM tenant_llm_config
        WHERE enabled = true
        "#,
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| GraphExtractError::Db(e.to_string()))?;
    tx.commit()
        .await
        .map_err(|e| GraphExtractError::Db(e.to_string()))?;

    match row {
        Some(r) => {
            let api_key = resolve_graph_api_key(
                r.api_key_ciphertext.as_deref(),
                r.api_key_nonce.as_deref(),
                r.api_key.as_deref(),
                enc_key,
                tenant_id,
            )?;
            match (api_key, r.llm_model.as_deref().filter(|m| !m.is_empty())) {
                (Some(key), Some(model)) => {
                    let base_url = r
                        .llm_base_url
                        .filter(|b| !b.is_empty())
                        .or_else(|| match r.provider.as_str() {
                            "openai" => Some(DEFAULT_OPENAI_CHAT_BASE_URL.to_string()),
                            _ => Some(global_cfg.base_url.clone()),
                        })
                        .unwrap_or_else(|| global_cfg.base_url.clone());
                    Ok(Box::new(DeepSeekGraphExtractor::new_with(
                        &base_url,
                        Some(&key),
                        model,
                    )))
                }
                _ => {
                    // No llm_model override or no usable API key → fall back
                    // to global DeepSeek (don't call OpenAI chat with an
                    // embedding model name).
                    Ok(Box::new(DeepSeekGraphExtractor::new(global_cfg)))
                }
            }
        }
        None => Ok(Box::new(DeepSeekGraphExtractor::new(global_cfg))),
    }
}

/// Resolve a tenant's BYOK API key for graph extraction.
///
/// Same priority and error semantics as `embedding::resolve_byok_api_key`:
/// encrypted fields take priority; missing key or decrypt failure is an
/// error, not a silent fallback.
fn resolve_graph_api_key(
    ciphertext: Option<&[u8]>,
    nonce: Option<&[u8]>,
    plaintext_key: Option<&str>,
    enc_key: Option<&[u8; 32]>,
    tenant_id: uuid::Uuid,
) -> Result<Option<String>, GraphExtractError> {
    match (ciphertext, nonce) {
        (Some(ct), Some(n)) => {
            let key = enc_key.ok_or_else(|| {
                GraphExtractError::Decrypt(
                    "encrypted BYOK key present but GMRAG_TENANT_KEY_ENCRYPTION_KEY not configured"
                        .into(),
                )
            })?;
            let decrypted = gmrag_core::crypto::decrypt_with_aad(ct, n, key, tenant_id.as_bytes())
                .map_err(|e| GraphExtractError::Decrypt(e.to_string()))?;
            Ok(Some(decrypted))
        }
        (None, None) => Ok(plaintext_key
            .filter(|v| !v.trim().is_empty())
            .map(|s| s.to_string())),
        _ => Err(GraphExtractError::Decrypt(
            "encrypted key pair is incomplete (one field NULL)".into(),
        )),
    }
}

#[derive(serde::Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    stream: bool,
    response_format: ResponseFormat,
}

#[derive(serde::Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(serde::Serialize)]
struct ResponseFormat {
    #[serde(rename = "type")]
    fmt: &'static str,
}

#[derive(serde::Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(serde::Deserialize)]
struct ChatChoice {
    message: ChatRespMessage,
}

#[derive(serde::Deserialize)]
struct ChatRespMessage {
    content: String,
}

#[derive(serde::Deserialize)]
struct RawExtraction {
    #[serde(default)]
    nodes: Vec<ExtractedNode>,
    #[serde(default)]
    edges: Vec<ExtractedEdge>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn extractor_at(server: &MockServer) -> DeepSeekGraphExtractor {
        DeepSeekGraphExtractor::new_with(&server.uri(), Some("sk-test"), "deepseek-v4-flash")
            .with_timeout_secs(5)
            .with_retries(2)
            .with_backoff_ms(5)
    }

    fn completion_body(content: &str) -> serde_json::Value {
        json!({
            "id": "chatcmpl-test",
            "object": "chat.completion",
            "model": "deepseek-v4-flash",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": content},
                "finish_reason": "stop"
            }]
        })
    }

    #[tokio::test]
    async fn deepseek_extractor_parses_nodes_and_edges() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(body_partial_json(json!({"model": "deepseek-v4-flash"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(completion_body(
                r#"{"nodes":[{"kind":"Person","label":"Alice","description":"Engineer"},{"kind":"Company","label":"Acme","description":"Tech company"}],"edges":[{"source":"Alice","target":"Acme","kind":"works_at"}]}"#,
            )))
            .mount(&server)
            .await;

        let ext = extractor_at(&server);
        let out = ext
            .extract("Alice works at Acme.")
            .await
            .expect("extract ok");
        assert_eq!(out.nodes.len(), 2);
        assert_eq!(out.nodes[0].label, "Alice");
        assert_eq!(out.nodes[0].kind, "Person");
        assert_eq!(out.nodes[1].label, "Acme");
        assert_eq!(out.edges.len(), 1);
        assert_eq!(out.edges[0].source, "Alice");
        assert_eq!(out.edges[0].target, "Acme");
        assert_eq!(out.edges[0].kind, "works_at");
    }

    #[tokio::test]
    async fn deepseek_extractor_tolerant_of_markdown_fenced_json() {
        let server = MockServer::start().await;
        let fenced = "```json\n{\"nodes\":[{\"kind\":\"Concept\",\"label\":\"Rust\",\"description\":\"Language\"}],\"edges\":[]}\n```";
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(completion_body(fenced)))
            .mount(&server)
            .await;

        let ext = extractor_at(&server);
        let out = ext
            .extract("Rust is a language.")
            .await
            .expect("extract ok");
        assert_eq!(out.nodes.len(), 1);
        assert_eq!(out.nodes[0].label, "Rust");
        assert!(out.edges.is_empty());
    }

    #[tokio::test]
    async fn deepseek_extractor_defaults_missing_edges_to_empty() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(completion_body(
                r#"{"nodes":[{"kind":"Person","label":"Bob","description":""}]}"#,
            )))
            .mount(&server)
            .await;

        let ext = extractor_at(&server);
        let out = ext.extract("Bob.").await.expect("extract ok");
        assert_eq!(out.nodes.len(), 1);
        assert!(out.edges.is_empty(), "missing edges must default to empty");
    }

    #[tokio::test]
    async fn deepseek_extractor_returns_empty_for_empty_text() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(completion_body(r#"{"nodes":[],"edges":[]}"#)),
            )
            .mount(&server)
            .await;

        let ext = extractor_at(&server);
        let out = ext.extract("").await.expect("extract ok");
        assert!(out.nodes.is_empty());
        assert!(out.edges.is_empty());
    }

    #[tokio::test]
    async fn deepseek_extractor_retries_on_5xx_then_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(500))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(completion_body(
                r#"{"nodes":[{"kind":"X","label":"Y","description":""}],"edges":[]}"#,
            )))
            .mount(&server)
            .await;

        let ext = extractor_at(&server);
        let out = ext.extract("text").await.expect("extract ok after retry");
        assert_eq!(out.nodes.len(), 1);
    }

    #[tokio::test]
    async fn deepseek_extractor_fails_after_max_retries() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let ext = extractor_at(&server).with_retries(1);
        let res = ext.extract("text").await;
        assert!(res.is_err(), "must error after exhausting retries");
    }

    #[test]
    fn parse_graph_json_handles_plain_object() {
        let raw = r#"{"nodes":[{"kind":"A","label":"B","description":"C"}],"edges":[]}"#;
        let out = parse_graph_json(raw).expect("parse ok");
        assert_eq!(out.nodes.len(), 1);
    }

    #[test]
    fn parse_graph_json_handles_fenced_block() {
        let raw = "Some prose\n```json\n{\"nodes\":[],\"edges\":[]}\n```\nmore";
        let out = parse_graph_json(raw).expect("parse ok");
        assert!(out.nodes.is_empty());
    }

    #[test]
    fn parse_graph_json_returns_err_on_garbage() {
        let out = parse_graph_json("not json at all");
        assert!(out.is_err());
    }
}
