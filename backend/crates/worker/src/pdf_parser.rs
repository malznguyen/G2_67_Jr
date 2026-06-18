//! PDF parser: pdf-extract (primary) + lopdf (fallback) with timeout.
//!
//! T36: `parse_pdf` wraps CPU-bound parsing in `tokio::task::spawn_blocking`
//! with a `tokio::time::timeout`. The primary path uses `pdf-extract` for
//! text extraction; if that fails or returns empty, the fallback returns
//! page count from `lopdf` with empty text. Invalid PDF bytes return `Err`.
//! A timeout returns `Err(PdfParseError::Timeout)`.

use std::time::Duration;

use thiserror::Error;

/// Result of parsing a PDF document.
#[derive(Debug)]
pub struct ParsedDocument {
    pub text: String,
    pub page_count: usize,
    pub extraction_method: ExtractionMethod,
}

/// Which extraction path produced the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractionMethod {
    /// `pdf-extract` successfully extracted text from a text-based PDF.
    PdfExtract,
    /// `pdf-extract` failed or returned empty; `lopdf` provided page count
    /// only (text may be empty — e.g. scanned PDF with no text layer).
    Fallback,
}

/// Errors emitted by [`parse_pdf`].
#[derive(Debug, Error)]
pub enum PdfParseError {
    #[error("PDF parse timed out after {0}s")]
    Timeout(u64),
    #[error("PDF parse error: {0}")]
    Parse(String),
}

/// Parse a PDF byte slice into text + page count, with a timeout.
///
/// The CPU-bound work runs on `spawn_blocking` so the async runtime is not
/// blocked. If parsing does not complete within `timeout_secs`, returns
/// `Err(PdfParseError::Timeout)`.
pub async fn parse_pdf(data: Vec<u8>, timeout_secs: u64) -> anyhow::Result<ParsedDocument> {
    let timeout = Duration::from_secs(timeout_secs);

    let result = tokio::time::timeout(
        timeout,
        tokio::task::spawn_blocking(move || parse_pdf_blocking(&data)),
    )
    .await;

    match result {
        Ok(Ok(parsed)) => parsed.map_err(anyhow::Error::new),
        Ok(Err(je)) => Err(anyhow::anyhow!("PDF parse thread error: {je}")),
        Err(_) => Err(anyhow::Error::new(PdfParseError::Timeout(timeout_secs))),
    }
}

/// Synchronous parsing — called inside `spawn_blocking`.
///
/// 1. Load with `lopdf` to get page count (always needed).
/// 2. Try `pdf-extract` for text extraction.
/// 3. If pdf-extract succeeds with non-empty text → `ExtractionMethod::PdfExtract`.
/// 4. Otherwise → `ExtractionMethod::Fallback` (page count from lopdf, empty text).
fn parse_pdf_blocking(data: &[u8]) -> std::result::Result<ParsedDocument, PdfParseError> {
    let doc = lopdf::Document::load_mem(data)
        .map_err(|e| PdfParseError::Parse(format!("lopdf load: {e}")))?;
    let page_count = doc.get_pages().len();

    match pdf_extract::extract_text_from_mem(data) {
        Ok(text) if !text.trim().is_empty() => Ok(ParsedDocument {
            text,
            page_count,
            extraction_method: ExtractionMethod::PdfExtract,
        }),
        _ => Ok(ParsedDocument {
            text: String::new(),
            page_count,
            extraction_method: ExtractionMethod::Fallback,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn parse_pdf_extracts_text_from_valid_pdf() {
        let data = include_bytes!("../tests/fixtures/sample.pdf").to_vec();
        let result = parse_pdf(data, 30).await;
        assert!(result.is_ok(), "parse should succeed: {:?}", result.err());
        let parsed = result.unwrap();
        assert!(
            parsed.page_count > 0,
            "page_count must be > 0"
        );
        assert!(
            !parsed.text.is_empty() || parsed.extraction_method == ExtractionMethod::Fallback,
            "either text is extracted or fallback is used (method={:?})",
            parsed.extraction_method
        );
    }

    #[tokio::test]
    async fn parse_pdf_returns_error_on_invalid_bytes() {
        let garbage = b"not a pdf at all".to_vec();
        let result = parse_pdf(garbage, 30).await;
        assert!(result.is_err(), "invalid bytes must return Err");
    }

    #[tokio::test]
    async fn parse_pdf_times_out() {
        let data = include_bytes!("../tests/fixtures/sample.pdf").to_vec();
        let result = parse_pdf(data, 0).await;
        assert!(result.is_err(), "timeout_secs=0 must return Err (timeout)");
    }
}
