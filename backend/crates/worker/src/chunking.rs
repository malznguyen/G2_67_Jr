//! Text chunking: tiktoken cl100k tokenizer, 1200-token chunks, 100-token overlap.
//!
//! T38: port of v1 `chunking.rs`. Pure function — no I/O, no async.
//! Joins per-page text with `\n\n`, splits into token-bounded chunks
//! via `text-splitter` with `cl100k_base` as the sizer, trims + filters
//! empty chunks.

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

/// Chunk a list of per-page text strings into token-bounded chunks.
///
/// Pages are joined with `\n\n` (blank lines); empty/whitespace-only
/// pages are skipped. The joined text is split into chunks of at most
/// `CHUNK_SIZE_TOKENS` (1200) tokens with `CHUNK_OVERLAP_TOKENS` (100)
/// overlap, using `cl100k_base` as the tokenizer. Each chunk is trimmed;
/// empty chunks are filtered out. Returns `Ok(vec![])` when all pages
/// are empty.
pub fn chunk_page_texts(page_texts: &[String]) -> Result<Vec<String>, ChunkError> {
    let full_text = page_texts
        .iter()
        .map(|page| page.trim())
        .filter(|page| !page.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");

    if full_text.is_empty() {
        return Ok(Vec::new());
    }

    let tokenizer = cl100k_base().map_err(|err| ChunkError::Tokenizer(err.to_string()))?;
    let config = ChunkConfig::new(CHUNK_SIZE_TOKENS)
        .with_sizer(tokenizer)
        .with_overlap(CHUNK_OVERLAP_TOKENS)
        .map_err(|err| ChunkError::Config(err.to_string()))?;

    let splitter = TextSplitter::new(config);
    Ok(splitter
        .chunks(&full_text)
        .map(str::trim)
        .filter(|chunk| !chunk.is_empty())
        .map(str::to_owned)
        .collect())
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
}
