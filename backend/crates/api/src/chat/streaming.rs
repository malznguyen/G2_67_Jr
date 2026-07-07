//! RAG chat streaming: provider SSE → parsed text/citation events (T49).

use std::pin::Pin;

use futures::{Stream, StreamExt};
use sqlx::PgConnection;
use thiserror::Error;
use uuid::Uuid;

use crate::llm::provider::{ChatMessage, LlmError, LlmProvider};
use crate::metering::{self, MeteringError};

use super::{assemble_system_prompt, ChunkHit, GraphContext};

const CHUNK_TAG_PREFIX: &str = "[chunk:";

/// Parsed stream event for downstream SSE mapping (T61).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatStreamEvent {
    Text { content: String },
    Citation { index: u32 },
    Done { finish_reason: Option<String> },
}

#[derive(Debug, Error)]
pub enum StreamingError {
    #[error(transparent)]
    Llm(#[from] LlmError),
    #[error(transparent)]
    Metering(#[from] MeteringError),
}

pub type ParsedChatStream =
    Pin<Box<dyn Stream<Item = Result<ChatStreamEvent, StreamingError>> + Send>>;

/// Stateful parser for model output containing `[chunk:N]` citation tags.
///
/// Handles tags split across provider deltas (R8 mitigation).
#[derive(Debug, Default)]
pub struct DeepseekTokenParser {
    buffer: String,
}

impl DeepseekTokenParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a text delta and return any complete text/citation events.
    pub fn push(&mut self, delta: &str) -> Vec<ChatStreamEvent> {
        if delta.is_empty() {
            return Vec::new();
        }
        self.buffer.push_str(delta);
        self.drain_complete()
    }

    /// Flush remaining buffer at end of stream.
    pub fn finish(&mut self) -> Vec<ChatStreamEvent> {
        if self.buffer.is_empty() {
            return Vec::new();
        }
        let remaining = std::mem::take(&mut self.buffer);
        vec![ChatStreamEvent::Text { content: remaining }]
    }

    fn drain_complete(&mut self) -> Vec<ChatStreamEvent> {
        let mut events = Vec::new();
        let mut cursor = 0;

        while let Some(rel) = self.buffer[cursor..].find(CHUNK_TAG_PREFIX) {
            let tag_start = cursor + rel;

            if tag_start > cursor {
                events.push(ChatStreamEvent::Text {
                    content: self.buffer[cursor..tag_start].to_string(),
                });
            }

            let Some(rel_end) = self.buffer[tag_start..].find(']') else {
                cursor = tag_start;
                break;
            };
            let tag_end = tag_start + rel_end + 1;
            let tag = &self.buffer[tag_start..tag_end];

            if let Some(index) = parse_chunk_tag(tag) {
                events.push(ChatStreamEvent::Citation { index });
                cursor = tag_end;
            } else {
                events.push(ChatStreamEvent::Text {
                    content: tag.to_string(),
                });
                cursor = tag_end;
            }
        }

        let holdback = holdback_len(&self.buffer[cursor..]);
        let emit_end = self.buffer.len().saturating_sub(holdback);
        if emit_end > cursor {
            events.push(ChatStreamEvent::Text {
                content: self.buffer[cursor..emit_end].to_string(),
            });
        }
        self.buffer = self.buffer[emit_end..].to_string();
        events
    }
}

fn parse_chunk_tag(tag: &str) -> Option<u32> {
    let rest = tag.strip_prefix(CHUNK_TAG_PREFIX)?.strip_suffix(']')?;
    rest.parse().ok()
}

/// Bytes at the end of `s` that may be an incomplete `[chunk:N]` tag.
fn holdback_len(s: &str) -> usize {
    for i in 1..=s.len() {
        let suffix = &s[s.len() - i..];
        if CHUNK_TAG_PREFIX.starts_with(suffix) {
            return i;
        }
        if suffix.starts_with(CHUNK_TAG_PREFIX) && !suffix.ends_with(']') {
            return i;
        }
    }
    0
}

/// Stream a RAG answer: system prompt + chat history + user query → parsed
/// text/citation events.
///
/// T84D Phase 3.3: the `history` slice is prepended between the system
/// message and the current user message in the `messages` array passed to
/// the provider, so the model can reason over the prior turns of this
/// session. Pass `&[]` for the no-history behaviour (preserved by older
/// callers).
pub async fn stream_rag_response(
    provider: &dyn LlmProvider,
    chunks: &[ChunkHit],
    graph: &GraphContext,
    user_query: &str,
    history: &[ChatMessage],
) -> Result<ParsedChatStream, StreamingError> {
    let system = assemble_system_prompt(chunks, graph);
    let mut messages = Vec::with_capacity(2 + history.len());
    messages.push(ChatMessage::new("system", system));
    messages.extend(history.iter().cloned());
    messages.push(ChatMessage::new("user", user_query));
    let upstream = provider.chat_stream(&messages).await?;

    let stream = async_stream::try_stream! {
        let mut parser = DeepseekTokenParser::new();
        let mut upstream = upstream;
        while let Some(delta) = upstream.next().await {
            let delta = delta?;
            for event in parser.push(&delta.content) {
                yield event;
            }
            if let Some(reason) = delta.finish_reason {
                for event in parser.finish() {
                    yield event;
                }
                yield ChatStreamEvent::Done {
                    finish_reason: Some(reason),
                };
                return;
            }
        }
        for event in parser.finish() {
            yield event;
        }
        yield ChatStreamEvent::Done { finish_reason: None };
    };

    Ok(Box::pin(stream))
}

/// Accumulate assistant text from parsed stream events (for metering / persistence).
pub fn assistant_text_from_events(events: &[ChatStreamEvent]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            ChatStreamEvent::Text { content } => Some(content.as_str()),
            _ => None,
        })
        .collect()
}

/// Record `llm_tokens` after a completed RAG chat stream (T51).
pub async fn meter_rag_chat_completion(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    provider: &dyn LlmProvider,
    chunks: &[ChunkHit],
    graph: &GraphContext,
    user_query: &str,
    events: &[ChatStreamEvent],
) -> Result<u32, StreamingError> {
    let system = assemble_system_prompt(chunks, graph);
    let input_text = format!("{system}\n{user_query}");
    let output_text = assistant_text_from_events(events);
    Ok(metering::record_llm_usage(
        conn,
        tenant_id,
        &input_text,
        &output_text,
        provider.chat_model(),
    )
    .await?)
}

/// Collect all events from a parsed chat stream (testing helper).
pub async fn collect_stream_events(
    stream: &mut ParsedChatStream,
) -> Result<Vec<ChatStreamEvent>, StreamingError> {
    let mut out = Vec::new();
    while let Some(item) = stream.next().await {
        out.push(item?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use futures::stream;
    use serde_json::json;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::llm::provider::{
        ChatDelta, ChatMessage, ChatStream, ChatStreamFuture, DeepSeekProvider,
        EmbeddingProviderConfig, GraphExtraction, LlmProvider, ProviderConfig, ProviderFuture,
    };

    fn push_all(parser: &mut DeepseekTokenParser, deltas: &[&str]) -> Vec<ChatStreamEvent> {
        deltas.iter().flat_map(|d| parser.push(d)).collect()
    }

    #[test]
    fn parser_emits_text_and_citation_for_complete_tag() {
        let mut parser = DeepseekTokenParser::new();
        let events = push_all(&mut parser, &["See [chunk:2] here"]);
        assert_eq!(
            events,
            vec![
                ChatStreamEvent::Text {
                    content: "See ".into()
                },
                ChatStreamEvent::Citation { index: 2 },
                ChatStreamEvent::Text {
                    content: " here".into()
                },
            ]
        );
    }

    #[test]
    fn parser_handles_tag_split_across_deltas() {
        let mut parser = DeepseekTokenParser::new();
        let mut events = push_all(&mut parser, &["Hel", "lo [chunk:", "1] world"]);
        events.extend(parser.finish());
        assert_eq!(
            events,
            vec![
                ChatStreamEvent::Text {
                    content: "Hel".into()
                },
                ChatStreamEvent::Text {
                    content: "lo ".into()
                },
                ChatStreamEvent::Citation { index: 1 },
                ChatStreamEvent::Text {
                    content: " world".into()
                },
            ]
        );
    }

    #[test]
    fn parser_emits_unknown_index_as_citation() {
        let mut parser = DeepseekTokenParser::new();
        let events = push_all(&mut parser, &["Ref [chunk:99]"]);
        assert!(events.contains(&ChatStreamEvent::Citation { index: 99 }));
    }

    #[test]
    fn parser_finish_emits_trailing_text() {
        let mut parser = DeepseekTokenParser::new();
        let events = parser.push("tail");
        assert_eq!(
            events,
            vec![ChatStreamEvent::Text {
                content: "tail".into()
            }]
        );
        assert!(parser.finish().is_empty());
    }

    #[test]
    fn parser_finish_emits_held_partial_suffix() {
        let mut parser = DeepseekTokenParser::new();
        let _ = parser.push("See [chunk:");
        assert_eq!(
            parser.finish(),
            vec![ChatStreamEvent::Text {
                content: "[chunk:".into()
            }]
        );
    }

    struct StaticChatProvider {
        body: &'static str,
        calls: AtomicUsize,
    }

    impl LlmProvider for StaticChatProvider {
        fn embed_query<'a>(&'a self, _query: &'a str) -> ProviderFuture<'a, Vec<f32>> {
            Box::pin(async { Ok(vec![0.1; 768]) })
        }

        fn chat_stream<'a>(&'a self, _messages: &'a [ChatMessage]) -> ChatStreamFuture<'a> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let body = self.body;
            Box::pin(async move {
                let stream: ChatStream = Box::pin(stream::iter(
                    body.lines().map(|line| Ok(parse_test_sse_line(line))),
                ));
                Ok(stream)
            })
        }

        fn graph_extract<'a>(&'a self, _text: &'a str) -> ProviderFuture<'a, GraphExtraction> {
            Box::pin(async { Ok(GraphExtraction::default()) })
        }

        fn provider(&self) -> &str {
            "static-mock"
        }

        fn chat_model(&self) -> &str {
            "mock-model"
        }
    }

    fn parse_test_sse_line(line: &str) -> ChatDelta {
        let payload = line.strip_prefix("data:").unwrap_or(line).trim();
        if payload == "[DONE]" {
            return ChatDelta {
                content: String::new(),
                finish_reason: Some("stop".into()),
            };
        }
        let v: serde_json::Value = serde_json::from_str(payload).unwrap();
        ChatDelta {
            content: v["choices"][0]["delta"]["content"]
                .as_str()
                .unwrap_or("")
                .to_string(),
            finish_reason: v["choices"][0]["finish_reason"]
                .as_str()
                .map(str::to_string),
        }
    }

    #[test]
    fn assistant_text_from_events_joins_text_deltas() {
        let events = vec![
            ChatStreamEvent::Text {
                content: "Hello".into(),
            },
            ChatStreamEvent::Citation { index: 1 },
            ChatStreamEvent::Text {
                content: " world".into(),
            },
        ];
        assert_eq!(assistant_text_from_events(&events), "Hello world");
    }

    #[tokio::test]
    async fn stream_rag_response_parses_provider_sse() {
        let provider = StaticChatProvider {
            body: concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"See [chunk:1]\"},\"finish_reason\":null}]}\n",
                "data: {\"choices\":[{\"delta\":{\"content\":\" answer\"},\"finish_reason\":null}]}\n",
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n",
            ),
            calls: AtomicUsize::new(0),
        };

        let mut stream =
            stream_rag_response(&provider, &[], &GraphContext::default(), "question?", &[])
                .await
                .expect("stream");

        let events = collect_stream_events(&mut stream).await.expect("events");
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        assert!(events.contains(&ChatStreamEvent::Citation { index: 1 }));
        assert!(events.contains(&ChatStreamEvent::Text {
            content: " answer".into()
        }));
        assert!(events.iter().any(|e| matches!(
            e,
            ChatStreamEvent::Done {
                finish_reason: Some(r),
                ..
            } if r == "stop"
        )));
    }

    #[tokio::test]
    async fn stream_rag_response_wiremock_end_to_end() {
        let server = MockServer::start().await;
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"lo [chunk:\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"1]!\"},\"finish_reason\":null}]}\n\n",
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

        let provider = DeepSeekProvider::new(ProviderConfig {
            chat_base_url: server.uri(),
            chat_model: "deepseek-v4-flash".into(),
            chat_api_key: Some("sk-test".into()),
            timeout_s: 5,
            embedding: EmbeddingProviderConfig::Ollama {
                host: server.uri(),
                model: "nomic-embed-text".into(),
            },
        });

        let mut stream = stream_rag_response(&provider, &[], &GraphContext::default(), "hi", &[])
            .await
            .expect("stream");

        let events = collect_stream_events(&mut stream).await.expect("events");
        assert!(events.contains(&ChatStreamEvent::Citation { index: 1 }));
        assert!(events.contains(&ChatStreamEvent::Text {
            content: "Hel".into()
        }));
    }
}
