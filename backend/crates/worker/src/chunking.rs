//! Text chunking: tiktoken cl100k tokenizer, 1200-token chunks, 100-token overlap.
//!
//! T38: port of v1 `chunking.rs`. Pure function — no I/O, no async.
//! Joins per-page text with `\n\n`, splits into token-bounded chunks
//! via `text-splitter` with `cl100k_base` as the sizer, trims + filters
//! empty chunks.
//!
//! T84D Phase 3.1: introduces [`Chunk`] carrying `page_start` / `page_end`
//! (1-based, mapped from each input page's substring offset in the joined
//! buffer). The legacy `Vec<String>` shape is still exposed via
//! [`chunk_page_texts`] for callers that don't need page metadata; the new
//! [`chunk_page_texts_with_pages`] returns `Vec<Chunk>`.
//!
//! Page mapping algorithm (pure-string, no token math): when pages are
//! joined, we record each page's `(start_byte, end_byte, page_number)` in
//! the joined buffer. For each emitted chunk we find the chunk's byte
//! range inside the joined buffer (locate its substring), then compute
//! `page_start` = the smallest page number whose byte range overlaps the
//! chunk, `page_end` = the largest. The splitter's `chunks()` returns
//! substrings of the joined text so byte offsets inside the joined text
//! are stable.

use text_splitter::{ChunkConfig, TextSplitter};
use tiktoken_rs::cl100k_base;
use thiserror::Error;

const CHUNK_SIZE_TOKENS: usize = 1200;
const CHUNK_OVERLAP_TOKENS: usize = 100;

/// Errors emitted by [`chunk_page_texts`].
#[derive(Debug, Error)]
pub enum ChunkError {
    #[error("tokenizer init failed: {0}")]
    Tokenizer(String),
    #[error("invalid chunk config: {0}")]
    Config(String),
}

/// T84D Phase 3.1 — a chunk with page-range metadata.
///
/// `page_start` / `page_end` are 1-based page numbers (matching the
/// `page_number` emitted by [`crate::pdf_parser::PageText`]). A chunk
/// that spans pages 2..4 has `page_start=2, page_end=4`. A chunk from a
/// legacy caller (no page info) keeps both fields as `0` — the qdrant
/// writer persists `NULL` in that case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub text: String,
    pub page_start: i32,
    pub page_end: i32,
}

impl Chunk {
    /// Construct a chunk with no page metadata (used by the legacy
    /// `chunk_page_texts` shim).
    fn no_pages(text: String) -> Self {
        Self {
            text,
            page_start: 0,
            page_end: 0,
        }
    }
}

/// Per-page byte range in the joined buffer: `(start, end, page_number)`.
struct PageSpan {
    start: usize,
    end: usize,
    page_number: u32,
}

/// Chunk a list of per-page text strings into token-bounded chunks.
///
/// Pages are joined with `\n\n` (blank lines); empty/whitespace-only
/// pages are skipped. The joined text is split into chunks of at most
/// `CHUNK_SIZE_TOKENS` (1200) tokens with `CHUNK_OVERLAP_TOKENS` (100)
/// overlap, using `cl100k_base` as the tokenizer. Each chunk is trimmed;
/// empty chunks are filtered out. Returns `Ok(vec![])` when all pages
/// are empty.
///
/// This is the legacy shape — no page metadata. New callers should
/// prefer [`chunk_page_texts_with_pages`].
pub fn chunk_page_texts(page_texts: &[String]) -> Result<Vec<String>, ChunkError> {
    let chunks = chunk_page_texts_with_pages(page_texts)?;
    Ok(chunks.into_iter().map(|c| c.text).collect())
}

/// Same as [`chunk_page_texts`] but returns [`Chunk`]s carrying page
/// metadata. Pages are 1-based and assigned in input order; blank pages
/// are still skipped (their page numbers are NOT used as `page_start` /
/// `page_end` sources).
pub fn chunk_page_texts_with_pages(page_texts: &[String]) -> Result<Vec<Chunk>, ChunkError> {
    // Build the joined buffer while tracking each non-blank page's byte
    // range in it. Pages are numbered 1-based in input order (matches
    // `pdf_parser::PageText::page_number`).
    let mut joined = String::new();
    let mut spans: Vec<PageSpan> = Vec::new();
    let mut first = true;
    for (idx, page) in page_texts.iter().enumerate() {
        let trimmed = page.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !first {
            joined.push_str("\n\n");
        }
        first = false;
        let start = joined.len();
        joined.push_str(trimmed);
        let end = joined.len();
        spans.push(PageSpan {
            start,
            end,
            page_number: (idx + 1) as u32,
        });
    }

    if joined.is_empty() {
        return Ok(Vec::new());
    }

    let tokenizer = cl100k_base().map_err(|err| ChunkError::Tokenizer(err.to_string()))?;
    let config = ChunkConfig::new(CHUNK_SIZE_TOKENS)
        .with_sizer(tokenizer)
        .with_overlap(CHUNK_OVERLAP_TOKENS)
        .map_err(|err| ChunkError::Config(err.to_string()))?;

    let splitter = TextSplitter::new(config);
    let mut out = Vec::new();
    let mut search_from = 0usize;
    for chunk_str in splitter.chunks(&joined) {
        let trimmed = chunk_str.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Locate this chunk inside `joined` starting after the prior
        // match so repeated page text does not map to the wrong page.
        let chunk_start = match joined[search_from..].find(trimmed) {
            Some(rel) => search_from + rel,
            None => match joined.find(trimmed) {
                Some(p) => p,
                None => {
                    out.push(Chunk::no_pages(trimmed.to_string()));
                    continue;
                }
            },
        };
        search_from = chunk_start.saturating_add(1);
        let chunk_end = chunk_start + trimmed.len();

        let (page_start, page_end) = page_range_for(&spans, chunk_start, chunk_end);
        out.push(Chunk {
            text: trimmed.to_string(),
            page_start,
            page_end,
        });
    }
    Ok(out)
}

/// Map a chunk's byte range `[start, end)` to its (page_start, page_end)
/// pair over the page spans. Returns `(0, 0)` when no page overlaps
/// (defensive — shouldn't happen given how the buffer is built).
fn page_range_for(spans: &[PageSpan], chunk_start: usize, chunk_end: usize) -> (i32, i32) {
    let mut page_start: Option<u32> = None;
    let mut page_end: u32 = 0;
    for span in spans {
        // Overlap iff span.start < chunk_end && span.end > chunk_start.
        if span.start < chunk_end && span.end > chunk_start {
            let pn = span.page_number;
            page_start = Some(match page_start {
                Some(prev) => prev.min(pn),
                None => pn,
            });
            page_end = page_end.max(pn);
        }
    }
    match page_start {
        Some(ps) => (ps as i32, page_end as i32),
        None => (0, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_empty_pages_returns_empty() {
        let out = chunk_page_texts(&[]).expect("must succeed");
        assert!(out.is_empty(), "no pages -> no chunks");
    }

    #[test]
    fn chunk_all_blank_pages_returns_empty() {
        let pages = vec!["   ".to_string(), "".to_string(), "\n\n".to_string()];
        let out = chunk_page_texts(&pages).expect("must succeed");
        assert!(out.is_empty(), "all-blank pages -> no chunks");
    }

    #[test]
    fn chunk_single_small_page_returns_single_chunk() {
        let pages = vec!["Hello world, this is a short page.".to_string()];
        let out = chunk_page_texts(&pages).expect("must succeed");
        assert_eq!(out.len(), 1, "short text -> 1 chunk");
        assert!(out[0].contains("Hello world"), "chunk must contain text");
    }

    #[test]
    fn chunk_long_text_splits_into_multiple_chunks() {
        // ~3000+ tokens: repeat a substantial paragraph many times.
        let paragraph = "The quick brown fox jumps over the lazy dog. \
            This sentence is used to build a long text that exceeds the \
            1200-token chunk size limit, forcing the splitter to produce \
            multiple chunks. ";
        let long_text = paragraph.repeat(200);
        let pages = vec![long_text];
        let out = chunk_page_texts(&pages).expect("must succeed");
        assert!(
            out.len() >= 3,
            "long text must split into >=3 chunks, got {}",
            out.len()
        );
    }

    #[test]
    fn chunk_respects_max_size_tokens() {
        let tokenizer = cl100k_base().expect("tokenizer");
        let paragraph = "The quick brown fox jumps over the lazy dog. \
            This sentence is used to build a long text that exceeds the \
            1200-token chunk size limit, forcing the splitter to produce \
            multiple chunks. ";
        let long_text = paragraph.repeat(200);
        let pages = vec![long_text];
        let out = chunk_page_texts(&pages).expect("must succeed");
        for (i, chunk) in out.iter().enumerate() {
            let tokens = tokenizer.encode_with_special_tokens(chunk).len();
            assert!(
                tokens <= CHUNK_SIZE_TOKENS,
                "chunk {i} has {tokens} tokens, max is {CHUNK_SIZE_TOKENS}"
            );
        }
    }

    #[test]
    fn chunk_trims_and_skips_blank_pages() {
        let pages = vec![
            "First page content here.".to_string(),
            "   ".to_string(),
            "".to_string(),
            "Third page content here.".to_string(),
        ];
        let out = chunk_page_texts(&pages).expect("must succeed");
        // Blank pages skipped; remaining text is short -> 1 chunk.
        assert_eq!(out.len(), 1, "non-blank pages joined -> 1 chunk");
        assert!(out[0].contains("First page"), "must contain page 1");
        assert!(out[0].contains("Third page"), "must contain page 3");
        // Blank page content must not appear.
        assert!(!out[0].starts_with("   "), "leading whitespace trimmed");
    }

    #[test]
    fn chunk_preserves_order() {
        let pages = vec![
            "Alice".to_string(),
            "Bob".to_string(),
            "Carol".to_string(),
        ];
        let out = chunk_page_texts(&pages).expect("must succeed");
        assert_eq!(out.len(), 1, "short text -> 1 chunk");
        // Order: Alice before Bob before Carol in the joined text.
        let combined = &out[0];
        let alice_pos = combined.find("Alice").expect("Alice");
        let bob_pos = combined.find("Bob").expect("Bob");
        let carol_pos = combined.find("Carol").expect("Carol");
        assert!(alice_pos < bob_pos, "Alice before Bob");
        assert!(bob_pos < carol_pos, "Bob before Carol");
    }

    #[test]
    fn chunk_multiple_pages_joined_with_double_newline() {
        let pages = vec!["Page one".to_string(), "Page two".to_string()];
        let out = chunk_page_texts(&pages).expect("must succeed");
        assert_eq!(out.len(), 1);
        assert!(
            out[0].contains("Page one\n\nPage two"),
            "pages must be joined with \\n\\n, got: {:?}",
            out[0]
        );
    }

    // ---------- T84D Phase 3.1: Chunk page metadata ----------

    #[test]
    fn chunk_with_pages_single_page_carries_page_metadata() {
        let pages = vec!["Hello world, this is a short page.".to_string()];
        let out = chunk_page_texts_with_pages(&pages).expect("must succeed");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].page_start, 1);
        assert_eq!(out[0].page_end, 1);
    }

    #[test]
    fn chunk_with_pages_skips_blank_pages_in_numbering() {
        // Page 2 is blank — it must NOT show up as page_start or page_end
        // of the resulting single chunk (the chunk spans only pages 1+3).
        let pages = vec![
            "First page content here.".to_string(),
            "   ".to_string(),
            "Third page content here.".to_string(),
        ];
        let out = chunk_page_texts_with_pages(&pages).expect("must succeed");
        assert_eq!(out.len(), 1, "joined text -> 1 chunk");
        assert_eq!(out[0].page_start, 1, "page_start must be 1");
        assert_eq!(out[0].page_end, 3, "page_end must be 3");
    }

    #[test]
    fn chunk_with_pages_assigns_correct_ranges_for_multi_page_long_text() {
        // Build three large pages so chunks do NOT span page boundaries
        // often. Use one paragraph big enough to produce multiple chunks
        // per page so each chunk's page range is a single page.
        let paragraph = "The quick brown fox jumps over the lazy dog. \
            This sentence is used to build a long text that exceeds the \
            1200-token chunk size limit, forcing the splitter to produce \
            multiple chunks. ";
        let pages = vec![
            paragraph.repeat(60),
            "MIDDLE PAGE MARKER ".repeat(200),
            paragraph.repeat(60),
        ];
        let out = chunk_page_texts_with_pages(&pages).expect("must succeed");
        assert!(out.len() >= 3, "need >=3 chunks for page mapping");
        // Every chunk must have non-zero page metadata.
        assert!(out.iter().all(|c| c.page_start >= 1 && c.page_end >= c.page_start));
        // At least one chunk's page_start must equal 1 (the first chunk
        // came from page 1) — robust against overlap spillover.
        assert_eq!(out[0].page_start, 1, "first chunk must start on page 1");
        // Some chunk's page_end must reach page 3 (last page text).
        assert!(
            out.iter().any(|c| c.page_end == 3),
            "at least one chunk must end on page 3, got: {:?}",
            out.iter().map(|c| (c.page_start, c.page_end)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn chunk_with_pages_returns_empty_for_all_blank_input() {
        let pages = vec!["".to_string(), "   ".to_string()];
        let out = chunk_page_texts_with_pages(&pages).expect("must succeed");
        assert!(out.is_empty());
    }
}
