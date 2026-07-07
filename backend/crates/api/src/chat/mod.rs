//! Chat/RAG helpers: retrieval orchestration and system prompt assembly.

pub mod retrieval;
pub mod streaming;

pub use retrieval::{
    accessible_document_ids, retrieve_all, retrieve_all_with_metering, retrieve_all_with_provider,
    retrieve_chunks, retrieve_chunks_with_vector, retrieve_graph_context, ChunkHit, GraphContext,
    GraphEdgeHit, GraphNodeHit, RetrievalError, RetrievalParams, DEFAULT_TOP_K,
};
pub use streaming::{
    assistant_text_from_events, collect_stream_events, meter_rag_chat_completion,
    stream_rag_response, ChatStreamEvent, DeepseekTokenParser, ParsedChatStream, StreamingError,
};

use std::collections::HashMap;

use uuid::Uuid;

/// Resolved chunk citation metadata for client display (T50, T84D page metadata).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ResolvedCitation {
    pub index: u32,
    pub point_id: Uuid,
    pub document_id: Uuid,
    pub chunk_index: i32,
    pub filename: Option<String>,
    /// T84D Phase 3.1: 1-based page range — `None` when no page metadata
    /// is recorded for the chunk (legacy rows / non-PDF ingest).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_start: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_end: Option<i32>,
}

impl From<&ChunkHit> for ResolvedCitation {
    fn from(hit: &ChunkHit) -> Self {
        Self {
            index: hit.citation_index,
            point_id: hit.point_id,
            document_id: hit.document_id,
            chunk_index: hit.chunk_index,
            filename: hit.filename.clone(),
            page_start: hit.page_start,
            page_end: hit.page_end,
        }
    }
}

/// Map a single 1-based citation index to retrieved chunk metadata.
pub fn resolve_citation(chunks: &[ChunkHit], index: u32) -> Option<ResolvedCitation> {
    chunks
        .iter()
        .find(|c| c.citation_index == index)
        .map(ResolvedCitation::from)
}

/// Map citation indices to chunk metadata; unknown indices are skipped.
/// Duplicate indices in the iterator are deduplicated (first occurrence wins).
pub fn resolve_chunk_index_citations(
    chunks: &[ChunkHit],
    indices: impl IntoIterator<Item = u32>,
) -> Vec<ResolvedCitation> {
    let by_index: HashMap<u32, &ChunkHit> = chunks.iter().map(|c| (c.citation_index, c)).collect();
    let mut seen = HashMap::new();
    let mut out = Vec::new();
    for index in indices {
        if seen.insert(index, ()).is_some() {
            continue;
        }
        if let Some(hit) = by_index.get(&index) {
            out.push(ResolvedCitation::from(*hit));
        }
    }
    out
}

/// JSON payload for a single SSE `data:` line (T61, T84D page metadata).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatSsePayload {
    Text {
        content: String,
    },
    Citation {
        index: u32,
        point_id: Uuid,
        document_id: Uuid,
        chunk_index: i32,
        #[serde(skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
        /// T84D Phase 3.1: nullable page range in the citation SSE.
        #[serde(skip_serializing_if = "Option::is_none")]
        page_start: Option<i32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        page_end: Option<i32>,
    },
    CitationUnknown {
        index: u32,
    },
    Done {
        #[serde(skip_serializing_if = "Option::is_none")]
        finish_reason: Option<String>,
    },
    Error {
        code: String,
        message: String,
    },
}

impl From<&EnrichedChatStreamEvent> for ChatSsePayload {
    fn from(ev: &EnrichedChatStreamEvent) -> Self {
        match ev {
            EnrichedChatStreamEvent::Text { content } => ChatSsePayload::Text {
                content: content.clone(),
            },
            EnrichedChatStreamEvent::CitationResolved(c) => ChatSsePayload::Citation {
                index: c.index,
                point_id: c.point_id,
                document_id: c.document_id,
                chunk_index: c.chunk_index,
                filename: c.filename.clone(),
                page_start: c.page_start,
                page_end: c.page_end,
            },
            EnrichedChatStreamEvent::CitationUnknown { index } => {
                ChatSsePayload::CitationUnknown { index: *index }
            }
            EnrichedChatStreamEvent::Done { finish_reason } => ChatSsePayload::Done {
                finish_reason: finish_reason.clone(),
            },
        }
    }
}

/// Enrich parsed stream events: `Citation { index }` → `CitationResolved { .. }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnrichedChatStreamEvent {
    Text { content: String },
    CitationResolved(ResolvedCitation),
    CitationUnknown { index: u32 },
    Done { finish_reason: Option<String> },
}

pub fn enrich_stream_events(
    chunks: &[ChunkHit],
    events: &[ChatStreamEvent],
) -> Vec<EnrichedChatStreamEvent> {
    events
        .iter()
        .map(|event| match event {
            ChatStreamEvent::Text { content } => EnrichedChatStreamEvent::Text {
                content: content.clone(),
            },
            ChatStreamEvent::Citation { index } => match resolve_citation(chunks, *index) {
                Some(resolved) => EnrichedChatStreamEvent::CitationResolved(resolved),
                None => EnrichedChatStreamEvent::CitationUnknown { index: *index },
            },
            ChatStreamEvent::Done { finish_reason } => EnrichedChatStreamEvent::Done {
                finish_reason: finish_reason.clone(),
            },
        })
        .collect()
}

/// Build the RAG system prompt with numbered `[chunk:N]` citation tags (T48).
pub fn assemble_system_prompt(chunks: &[ChunkHit], graph: &GraphContext) -> String {
    let mut out = String::from(
        "You are GMRAG assistant. Answer ONLY from the context below.\n\
         When citing document text, use the exact tag [chunk:N] matching the excerpt number.\n",
    );

    out.push_str("\n## Document excerpts\n");
    if chunks.is_empty() {
        out.push_str("No document context was retrieved.\n");
    } else {
        for chunk in chunks {
            let source = chunk
                .filename
                .clone()
                .unwrap_or_else(|| chunk.document_id.to_string());
            out.push_str(&format!(
                "[chunk:{}] (source: {source})\n{}\n\n",
                chunk.citation_index, chunk.content
            ));
        }
    }

    if !graph.nodes.is_empty() {
        out.push_str("\n## Knowledge graph\n### Entities\n");
        for node in &graph.nodes {
            out.push_str(&format!(
                "- {} ({}): {}\n",
                node.label, node.kind, node.description
            ));
        }

        if !graph.edges.is_empty() {
            out.push_str("\n### Relationships\n");
            for edge in &graph.edges {
                out.push_str(&format!(
                    "- {} --[{}]--> {}\n",
                    edge.src_label, edge.kind, edge.dst_label
                ));
            }
        }
    }

    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn sample_chunk(index: u32, content: &str) -> ChunkHit {
        ChunkHit {
            citation_index: index,
            point_id: Uuid::new_v4(),
            document_id: Uuid::new_v4(),
            chunk_index: (index - 1) as i32,
            content: content.into(),
            filename: Some(format!("doc{index}.pdf")),
            score: 1.0,
            page_start: None,
            page_end: None,
        }
    }

    #[test]
    fn prompt_includes_chunk_tags_in_order() {
        let chunks = vec![
            sample_chunk(1, "first excerpt"),
            sample_chunk(2, "second excerpt"),
        ];
        let prompt = assemble_system_prompt(&chunks, &GraphContext::default());
        assert!(prompt.contains("[chunk:1]"));
        assert!(prompt.contains("[chunk:2]"));
        assert!(prompt.contains("first excerpt"));
        assert!(prompt.contains("second excerpt"));
    }

    #[test]
    fn prompt_omits_graph_section_when_empty() {
        let chunks = vec![sample_chunk(1, "only chunk")];
        let prompt = assemble_system_prompt(&chunks, &GraphContext::default());
        assert!(!prompt.contains("## Knowledge graph"));
        assert!(prompt.contains("## Document excerpts"));
    }

    #[test]
    fn prompt_includes_entities_and_edges() {
        let graph = GraphContext {
            nodes: vec![GraphNodeHit {
                node_id: Uuid::new_v4(),
                label: "Alice".into(),
                kind: "Person".into(),
                description: "Engineer".into(),
                score: Some(0.9),
            }],
            edges: vec![GraphEdgeHit {
                src_node_id: Uuid::new_v4(),
                dst_node_id: Uuid::new_v4(),
                src_label: "Alice".into(),
                dst_label: "Acme".into(),
                kind: "works_at".into(),
            }],
        };
        let prompt = assemble_system_prompt(&[], &graph);
        assert!(prompt.contains("Alice (Person): Engineer"));
        assert!(prompt.contains("Alice --[works_at]--> Acme"));
    }

    #[test]
    fn prompt_handles_zero_chunks() {
        let prompt = assemble_system_prompt(&[], &GraphContext::default());
        assert!(prompt.contains("No document context was retrieved"));
        assert!(prompt.contains("GMRAG assistant"));
    }

    #[test]
    fn resolve_citation_maps_index_to_point_id() {
        let chunks = vec![sample_chunk(1, "a"), sample_chunk(2, "b")];
        let resolved = resolve_citation(&chunks, 2).expect("found");
        assert_eq!(resolved.index, 2);
        assert_eq!(resolved.point_id, chunks[1].point_id);
        assert_eq!(resolved.document_id, chunks[1].document_id);
    }

    #[test]
    fn resolve_citation_unknown_index_returns_none() {
        let chunks = vec![sample_chunk(1, "a")];
        assert!(resolve_citation(&chunks, 99).is_none());
    }

    #[test]
    fn resolve_chunk_index_citations_dedupes_and_skips_unknown() {
        let chunks = vec![sample_chunk(1, "a"), sample_chunk(2, "b")];
        let resolved = resolve_chunk_index_citations(&chunks, [2, 99, 2, 1]);
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].index, 2);
        assert_eq!(resolved[1].index, 1);
    }

    #[test]
    fn resolve_chunk_index_citations_empty_chunks() {
        assert!(resolve_chunk_index_citations(&[], [1]).is_empty());
    }

    #[test]
    fn enrich_stream_events_resolves_citations() {
        let chunks = vec![sample_chunk(1, "a")];
        let events = vec![
            ChatStreamEvent::Text {
                content: "See ".into(),
            },
            ChatStreamEvent::Citation { index: 1 },
            ChatStreamEvent::Citation { index: 99 },
        ];
        let enriched = enrich_stream_events(&chunks, &events);
        assert!(matches!(
            enriched[1],
            EnrichedChatStreamEvent::CitationResolved(_)
        ));
        assert!(matches!(
            enriched[2],
            EnrichedChatStreamEvent::CitationUnknown { index: 99 }
        ));
    }
}
