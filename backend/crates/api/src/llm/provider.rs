//! API-side LLM provider abstraction.
//!
//! Sprint 6 uses this module as the common surface for retrieval and chat:
//! query embedding, chat deltas from OpenAI-compatible SSE, and graph
//! extraction from OpenAI-compatible chat completions.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use futures::{Stream, StreamExt};
use gmrag_core::config::{DeepSeekConfig, OllamaConfig};
use thiserror::Error;

pub const EMBED_DIM: usize = 768;
pub const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
pub const DEFAULT_OPENAI_EMBED_MODEL: &str = "text-embedding-3-small";

const GRAPH_SYSTEM_PROMPT: &str = "You are a knowledge-graph extractor. Read the user text and return ONLY a JSON object with the exact shape {\"nodes\":[{\"kind\":string,\"label\":string,\"description\":string}],\"edges\":[{\"source\":string,\"target\":string,\"kind\":string}]}. Do not wrap the JSON in markdown fences. Do not add commentary. If no entities are found, return {\"nodes\":[],\"edges\":[]}.";

pub type ProviderFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, LlmError>> + Send + 'a>>;
pub type ChatStream = Pin<Box<dyn Stream<Item = Result<ChatDelta, LlmError>> + Send>>;
pub type ChatStreamFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ChatStream, LlmError>> + Send + 'a>>;

pub trait LlmProvider: Send + Sync {
    fn embed_query<'a>(&'a self, query: &'a str) -> ProviderFuture<'a, Vec<f32>>;
    fn chat_stream<'a>(&'a self, messages: &'a [ChatMessage]) -> ChatStreamFuture<'a>;
    fn graph_extract<'a>(&'a self, text: &'a str) -> ProviderFuture<'a, GraphExtraction>;
    fn provider(&self) -> &str;
    fn chat_model(&self) -> &str;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbeddingProviderConfig {
    Ollama {
        host: String,
        model: String,
    },
    OpenAi {
        api_key: String,
        base_url: String,
        model: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderConfig {
    pub chat_base_url: String,
    pub chat_model: String,
    pub chat_api_key: Option<String>,
    pub timeout_s: u64,
    pub embedding: EmbeddingProviderConfig,
}

impl ProviderConfig {
    pub fn from_global(deepseek: &DeepSeekConfig, ollama: &OllamaConfig) -> Self {
        Self {
            chat_base_url: deepseek.base_url.clone(),
            chat_model: deepseek.model.clone(),
            chat_api_key: deepseek.api_key.clone(),
            timeout_s: deepseek.timeout_s,
            embedding: EmbeddingProviderConfig::Ollama {
                host: ollama.host.clone(),
                model: ollama.embed_model.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatDelta {
    pub content: String,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct ExtractedNode {
    pub kind: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct ExtractedEdge {
    pub source: String,
    pub target: String,
    pub kind: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
pub struct GraphExtraction {
    #[serde(default)]
    pub nodes: Vec<ExtractedNode>,
    #[serde(default)]
    pub edges: Vec<ExtractedEdge>,
}

#[derive(Debug, Error)]
pub enum LlmError {
    #[error("llm HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("llm request timed out after {0}s")]
    Timeout(u64),
    #[error("llm provider returned an empty response")]
    Empty,
    #[error("llm json parse error: {0}")]
    Parse(String),
    #[error("embedding provider returned {actual} embeddings for {expected} requested texts")]
    CountMismatch { expected: usize, actual: usize },
}

pub struct DeepSeekProvider {
    client: reqwest::Client,
    chat_url: String,
    chat_api_key: Option<String>,
    chat_model: String,
    timeout: Duration,
    embedding: EmbeddingProviderConfig,
}

impl DeepSeekProvider {
    pub fn new(config: ProviderConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            chat_url: format!(
                "{}/chat/completions",
                config.chat_base_url.trim_end_matches('/')
            ),
            chat_api_key: config.chat_api_key,
            chat_model: config.chat_model,
            timeout: Duration::from_secs(config.timeout_s.max(1)),
            embedding: config.embedding,
        }
    }

    pub fn from_global(deepseek: &DeepSeekConfig, ollama: &OllamaConfig) -> Self {
        Self::new(ProviderConfig::from_global(deepseek, ollama))
    }

    pub fn chat_url(&self) -> &str {
        &self.chat_url
    }

    async fn embed_query_inner(&self, query: &str) -> Result<Vec<f32>, LlmError> {
        match &self.embedding {
            EmbeddingProviderConfig::Ollama { host, model } => {
                let url = format!("{}/api/embed", host.trim_end_matches('/'));
                let body = OllamaEmbedRequest {
                    model,
                    input: &[query],
                };
                let resp =
                    tokio::time::timeout(self.timeout, self.client.post(url).json(&body).send())
                        .await
                        .map_err(|_| LlmError::Timeout(self.timeout.as_secs()))??;
                let parsed = resp
                    .error_for_status()?
                    .json::<OllamaEmbedResponse>()
                    .await?;
                one_embedding(parsed.embeddings)
            }
            EmbeddingProviderConfig::OpenAi {
                api_key,
                base_url,
                model,
            } => {
                let url = format!("{}/embeddings", base_url.trim_end_matches('/'));
                let body = OpenAiEmbedRequest {
                    model,
                    input: &[query],
                    dimensions: EMBED_DIM,
                };
                let resp = tokio::time::timeout(
                    self.timeout,
                    self.client
                        .post(url)
                        .bearer_auth(api_key)
                        .json(&body)
                        .send(),
                )
                .await
                .map_err(|_| LlmError::Timeout(self.timeout.as_secs()))??;
                let parsed = resp
                    .error_for_status()?
                    .json::<OpenAiEmbedResponse>()
                    .await?;
                let embeddings = parsed.data.into_iter().map(|d| d.embedding).collect();
                one_embedding(embeddings)
            }
        }
    }

    async fn chat_stream_inner(&self, messages: &[ChatMessage]) -> Result<ChatStream, LlmError> {
        let body = ChatRequest {
            model: &self.chat_model,
            messages,
            stream: true,
            response_format: None,
        };
        let mut req = self.client.post(&self.chat_url).json(&body);
        if let Some(key) = &self.chat_api_key {
            req = req.bearer_auth(key);
        }
        let resp = tokio::time::timeout(self.timeout, req.send())
            .await
            .map_err(|_| LlmError::Timeout(self.timeout.as_secs()))??;
        let mut bytes = resp.error_for_status()?.bytes_stream();

        let stream = async_stream::try_stream! {
            let mut buffer = String::new();
            while let Some(chunk) = bytes.next().await {
                let chunk = chunk?;
                buffer.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(event) = pop_next_sse_event(&mut buffer) {
                    if let Some(delta) = parse_sse_event(&event)? {
                        yield delta;
                    }
                }
            }
            if !buffer.trim().is_empty() {
                if let Some(delta) = parse_sse_event(&buffer)? {
                    yield delta;
                }
            }
        };

        Ok(Box::pin(stream))
    }

    async fn graph_extract_inner(&self, text: &str) -> Result<GraphExtraction, LlmError> {
        let body = ChatRequest {
            model: &self.chat_model,
            messages: &[
                ChatMessage::new("system", GRAPH_SYSTEM_PROMPT),
                ChatMessage::new("user", text),
            ],
            stream: false,
            response_format: Some(ResponseFormat { fmt: "json_object" }),
        };
        let mut req = self.client.post(&self.chat_url).json(&body);
        if let Some(key) = &self.chat_api_key {
            req = req.bearer_auth(key);
        }
        let resp = tokio::time::timeout(self.timeout, req.send())
            .await
            .map_err(|_| LlmError::Timeout(self.timeout.as_secs()))??;
        let parsed = resp.error_for_status()?.json::<ChatResponse>().await?;
        let content = parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .filter(|c| !c.trim().is_empty())
            .ok_or(LlmError::Empty)?;
        parse_graph_json(&content)
    }
}

impl LlmProvider for DeepSeekProvider {
    fn embed_query<'a>(&'a self, query: &'a str) -> ProviderFuture<'a, Vec<f32>> {
        Box::pin(async move { self.embed_query_inner(query).await })
    }

    fn chat_stream<'a>(&'a self, messages: &'a [ChatMessage]) -> ChatStreamFuture<'a> {
        Box::pin(async move { self.chat_stream_inner(messages).await })
    }

    fn graph_extract<'a>(&'a self, text: &'a str) -> ProviderFuture<'a, GraphExtraction> {
        Box::pin(async move { self.graph_extract_inner(text).await })
    }

    fn provider(&self) -> &str {
        "deepseek"
    }

    fn chat_model(&self) -> &str {
        &self.chat_model
    }
}

fn one_embedding(embeddings: Vec<Vec<f32>>) -> Result<Vec<f32>, LlmError> {
    if embeddings.len() != 1 {
        return Err(LlmError::CountMismatch {
            expected: 1,
            actual: embeddings.len(),
        });
    }
    embeddings
        .into_iter()
        .next()
        .filter(|v| !v.is_empty())
        .ok_or(LlmError::Empty)
}

pub fn parse_graph_json(raw: &str) -> Result<GraphExtraction, LlmError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(LlmError::Parse("empty completion".into()));
    }
    let candidate = extract_json_object(trimmed)
        .ok_or_else(|| LlmError::Parse("no JSON object found in completion".into()))?;
    let parsed: GraphExtraction =
        serde_json::from_str(candidate).map_err(|e| LlmError::Parse(e.to_string()))?;
    Ok(parsed)
}

fn extract_json_object(s: &str) -> Option<&str> {
    let after_fence = if let Some(start) = s.find("```") {
        let rest = &s[start + 3..];
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
    (end >= start).then(|| &after_fence[start..=end])
}

fn pop_next_sse_event(buffer: &mut String) -> Option<String> {
    let lf = buffer.find("\n\n").map(|i| (i, 2));
    let crlf = buffer.find("\r\n\r\n").map(|i| (i, 4));
    let (idx, delimiter_len) = match (lf, crlf) {
        (Some(a), Some(b)) => {
            if a.0 <= b.0 {
                a
            } else {
                b
            }
        }
        (Some(a), None) => a,
        (None, Some(b)) => b,
        (None, None) => return None,
    };
    let event = buffer[..idx].to_string();
    buffer.drain(..idx + delimiter_len);
    Some(event)
}

pub(crate) fn parse_sse_event(event: &str) -> Result<Option<ChatDelta>, LlmError> {
    let mut data = Vec::new();
    for line in event.lines() {
        let line = line.trim_end_matches('\r').trim();
        if let Some(rest) = line.strip_prefix("data:") {
            data.push(rest.trim());
        }
    }
    if data.is_empty() {
        return Ok(None);
    }
    let payload = data.join("\n");
    if payload == "[DONE]" {
        return Ok(None);
    }

    let parsed: StreamChunk =
        serde_json::from_str(&payload).map_err(|e| LlmError::Parse(e.to_string()))?;
    let Some(choice) = parsed.choices.into_iter().next() else {
        return Ok(None);
    };
    let content = choice.delta.content.unwrap_or_default();
    if content.is_empty() && choice.finish_reason.is_none() {
        return Ok(None);
    }
    Ok(Some(ChatDelta {
        content,
        finish_reason: choice.finish_reason,
    }))
}

#[derive(serde::Serialize)]
struct OllamaEmbedRequest<'a> {
    model: &'a str,
    input: &'a [&'a str],
}

#[derive(serde::Deserialize)]
struct OllamaEmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

#[derive(serde::Serialize)]
struct OpenAiEmbedRequest<'a> {
    model: &'a str,
    input: &'a [&'a str],
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

#[derive(serde::Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormat>,
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
struct StreamChunk {
    choices: Vec<StreamChoice>,
}

#[derive(serde::Deserialize)]
struct StreamChoice {
    delta: StreamDelta,
    finish_reason: Option<String>,
}

#[derive(serde::Deserialize)]
struct StreamDelta {
    content: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn provider_at(server: &MockServer) -> DeepSeekProvider {
        DeepSeekProvider::new(ProviderConfig {
            chat_base_url: server.uri(),
            chat_model: "deepseek-v4-flash".into(),
            chat_api_key: Some("sk-test".into()),
            timeout_s: 5,
            embedding: EmbeddingProviderConfig::Ollama {
                host: server.uri(),
                model: "nomic-embed-text".into(),
            },
        })
    }

    #[tokio::test]
    async fn embed_query_uses_ollama_default() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/embed"))
            .and(body_partial_json(json!({
                "model": "nomic-embed-text",
                "input": ["hello"]
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "embeddings": [vec![0.25; EMBED_DIM]]
            })))
            .mount(&server)
            .await;

        let provider = provider_at(&server);
        let out = provider.embed_query("hello").await.expect("embed query");
        assert_eq!(out.len(), EMBED_DIM);
        assert_eq!(out[0], 0.25);
    }

    #[tokio::test]
    async fn graph_extract_sends_bearer_auth_and_parses_json() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(header("authorization", "Bearer sk-test"))
            .and(body_partial_json(json!({
                "model": "deepseek-v4-flash",
                "stream": false,
                "response_format": {"type": "json_object"}
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{
                    "message": {
                        "content": "```json\n{\"nodes\":[{\"kind\":\"Person\",\"label\":\"Alice\",\"description\":\"Engineer\"}],\"edges\":[]}\n```"
                    }
                }]
            })))
            .mount(&server)
            .await;

        let provider = provider_at(&server);
        let out = provider
            .graph_extract("Alice is an engineer.")
            .await
            .expect("graph extract");
        assert_eq!(out.nodes.len(), 1);
        assert_eq!(out.nodes[0].label, "Alice");
        assert!(out.edges.is_empty());
    }

    #[tokio::test]
    async fn chat_stream_parses_openai_compatible_sse_deltas() {
        let server = MockServer::start().await;
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(body_partial_json(json!({
                "model": "deepseek-v4-flash",
                "stream": true
            })))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(body),
            )
            .mount(&server)
            .await;

        let provider = provider_at(&server);
        let mut stream = provider
            .chat_stream(&[ChatMessage::new("user", "hello")])
            .await
            .expect("chat stream");

        let mut deltas = Vec::new();
        while let Some(delta) = stream.next().await {
            deltas.push(delta.expect("delta"));
        }

        assert_eq!(deltas.len(), 3);
        assert_eq!(deltas[0].content, "Hel");
        assert_eq!(deltas[1].content, "lo");
        assert_eq!(deltas[2].finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn parse_sse_event_ignores_done() {
        let parsed = parse_sse_event("data: [DONE]").expect("parse");
        assert!(parsed.is_none());
    }
}
