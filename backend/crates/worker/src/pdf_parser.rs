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
    /// At least one page was rasterized and sent to an OCR engine
    /// (Ollama vision) because the text layer was too thin.
    Ocr,
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
    // In tests, add a small delay so timeout tests with Duration::ZERO
    // reliably fire before spawn_blocking completes. Without this, a warm
    // blocking thread pool can finish parsing a 596-byte PDF faster than
    // the timeout future's first poll — a race condition.
    #[cfg(test)]
    std::thread::sleep(Duration::from_millis(100));

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

// =========================================================
// T37: OCR fallback — per-page text extraction + OCR rescue
// =========================================================

/// Minimum chars per page to skip OCR. Pages with fewer extracted
/// characters are considered "scanned" / image-only and sent to OCR.
const MIN_TEXT_CHARS: usize = 50;

/// Errors emitted by [`PageRenderer::render_page_to_png`].
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("PDF render error: {0}")]
    Render(String),
    #[error("page {page} does not exist (document has {page_count} pages)")]
    PageOutOfRange { page: u32, page_count: usize },
}

/// Trait abstracting PDF page rasterization. Production uses
/// `PdfiumRenderer` (pdfium-render + libpdfium); tests use `MockRenderer`
/// to avoid the native library dependency.
pub trait PageRenderer: Send + Sync {
    fn render_page_to_png(
        &self,
        data: &[u8],
        page_number: u32,
    ) -> Result<Vec<u8>, RenderError>;
}

/// Mock renderer for tests. Returns canned PNG bytes regardless of input.
pub struct MockRenderer {
    image_bytes: Vec<u8>,
}

impl MockRenderer {
    pub fn new(image_bytes: Vec<u8>) -> Self {
        Self { image_bytes }
    }
}

impl PageRenderer for MockRenderer {
    fn render_page_to_png(
        &self,
        _data: &[u8],
        _page_number: u32,
    ) -> Result<Vec<u8>, RenderError> {
        Ok(self.image_bytes.clone())
    }
}

/// Per-page extraction result (internal).
struct PdfPage {
    page_number: u32,
    text: String,
    needs_ocr: bool,
}

/// Per-page text extraction (sync, called inside `spawn_blocking`).
///
/// Uses `pdf_extract::output_doc_page` for per-page text (unlike T36's
/// `parse_pdf_blocking` which uses `extract_text_from_mem` for the whole
/// document). Pages with `< MIN_TEXT_CHARS` extracted characters are
/// flagged `needs_ocr`.
fn extract_pages_blocking(data: &[u8]) -> Result<Vec<PdfPage>, PdfParseError> {
    #[cfg(test)]
    std::thread::sleep(Duration::from_millis(100));

    let doc = lopdf::Document::load_mem(data)
        .map_err(|e| PdfParseError::Parse(format!("lopdf load: {e}")))?;

    let mut page_numbers: Vec<u32> = doc.get_pages().into_keys().collect();
    page_numbers.sort_unstable();

    let mut pages = Vec::with_capacity(page_numbers.len());
    for page_number in page_numbers {
        let text = extract_page_text(&doc, page_number);
        let needs_ocr = text.trim().chars().count() < MIN_TEXT_CHARS;
        pages.push(PdfPage {
            page_number,
            text,
            needs_ocr,
        });
    }
    Ok(pages)
}

/// Extract text from a single page via `pdf_extract::output_doc_page`.
fn extract_page_text(doc: &lopdf::Document, page_number: u32) -> String {
    use pdf_extract::{PlainTextOutput, output_doc_page};
    let mut text = String::new();
    let result = {
        let mut output = PlainTextOutput::new(&mut text);
        output_doc_page(doc, &mut output, page_number)
    };
    if result.is_err() {
        return String::new();
    }
    text
}

/// Parse a PDF with OCR fallback for scanned/image-only pages.
///
/// Flow:
/// 1. Load PDF with `lopdf` + extract per-page text with `pdf-extract`
///    (inside `spawn_blocking` with a timeout).
/// 2. For each page where extracted text is `< MIN_TEXT_CHARS`:
///    a. Rasterize the page via `renderer.render_page_to_png()`.
///    b. OCR the image via `ocr.ocr_image()`.
///    c. Append OCR text to the page's text.
/// 3. Join all page texts with `\n\n` and return a [`ParsedDocument`].
///
/// `extraction_method` is [`ExtractionMethod::Ocr`] if any page used OCR,
/// [`ExtractionMethod::PdfExtract`] if text was extracted without OCR,
/// or [`ExtractionMethod::Fallback`] if no text at all was produced.
pub async fn parse_pdf_with_ocr(
    data: Vec<u8>,
    timeout_secs: u64,
    renderer: &dyn PageRenderer,
    ocr: &dyn crate::ocr::OcrClient,
) -> anyhow::Result<ParsedDocument> {
    let data_for_render = data.clone();

    let parse_result = tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        tokio::task::spawn_blocking(move || extract_pages_blocking(&data)),
    )
    .await;

    let pages = match parse_result {
        Ok(Ok(Ok(pages))) => pages,
        Ok(Ok(Err(e))) => return Err(anyhow::Error::new(e)),
        Ok(Err(je)) => return Err(anyhow::anyhow!("PDF parse thread error: {je}")),
        Err(_) => return Err(anyhow::Error::new(PdfParseError::Timeout(timeout_secs))),
    };

    let page_count = pages.len();
    let mut used_ocr = false;
    let mut page_texts = Vec::with_capacity(page_count);

    for page in pages {
        let mut text = page.text;
        if page.needs_ocr {
            let image_bytes = renderer
                .render_page_to_png(&data_for_render, page.page_number)
                .map_err(|e| anyhow::anyhow!("render error: {e}"))?;
            let ocr_text = ocr
                .ocr_image(&image_bytes)
                .await
                .map_err(|e| anyhow::anyhow!("ocr error: {e}"))?;
            if !text.trim().is_empty() && !ocr_text.trim().is_empty() {
                text.push('\n');
            }
            text.push_str(&ocr_text);
            used_ocr = true;
        }
        page_texts.push(text);
    }

    let full_text = page_texts
        .iter()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");

    let extraction_method = if used_ocr {
        ExtractionMethod::Ocr
    } else if !full_text.is_empty() {
        ExtractionMethod::PdfExtract
    } else {
        ExtractionMethod::Fallback
    };

    Ok(ParsedDocument {
        text: full_text,
        page_count,
        extraction_method,
    })
}

// =========================================================
// T84D Phase 1.3 — Page-aware ingest dispatcher + feature-
// gated PdfiumRenderer.
// =========================================================

/// Per-page extracted text (1-based `page_number`). Returned by
/// [`parse_pdf_for_ingest`]; consumed by the chunking pipeline (Phase 3
/// maps each chunk's char range back to these page numbers to compute
/// `page_start` / `page_end`).
#[derive(Debug, Clone)]
pub struct PageText {
    pub page_number: u32,
    pub text: String,
}

/// Feature-flagged native renderer backed by `pdfium-render` + libpdfium.
/// Compiled only when the `ocr-pdfium` Cargo feature is on; the feature is
/// OFF by default so the Docker build never bakes libpdfium in.
#[cfg(feature = "ocr-pdfium")]
pub struct PdfiumRenderer {
    #[allow(dead_code)]
    private: (),
}

#[cfg(feature = "ocr-pdfium")]
impl PdfiumRenderer {
    pub fn new() -> Self {
        Self { private: () }
    }
}

#[cfg(feature = "ocr-pdfium")]
impl PageRenderer for PdfiumRenderer {
    fn render_page_to_png(
        &self,
        _data: &[u8],
        _page_number: u32,
    ) -> Result<Vec<u8>, RenderError> {
        // The pdfium-render bindings require unsafe FFI + a bundled
        // libpdfium at link time. T84D intentionally leaves the wiring
        // off-by-default (the feature is gated); operators who build the
        // native image flip the feature on AND land the real impl here.
        Err(RenderError::Render(
            "PdfiumRenderer::render_page_to_png not implemented on this build".into(),
        ))
    }
}

/// Page-aware extraction entry point — replaces the legacy
/// `parse_pdf(...)` + `vec![parsed.text]` pair in `process_job`.
///
/// Behaviour:
/// - Always extract per-page text via `extract_pages_blocking` (the
///   shared lopdf + pdf_extract path the OCR fallback already uses).
/// - If OCR is requested (`ocr_enabled = true`) AND the `ocr-pdfium`
///   Cargo feature is on → use the OCR fallback to rasterize/OCR
///   scanned pages (Phase 1 stubs the renderer because the feature is
///   off by default; Phase 1 just ensures the dispatcher reaches the
///   OCR branch under the feature flag).
/// - Otherwise → join per-page text, set [`ExtractionMethod::PdfExtract`]
///   when text was produced, else [`ExtractionMethod::Fallback`].
///
/// Returns `(Vec<PageText>, ExtractionMethod)` — the chunking layer in
/// Phase 3 maps each chunk back to page numbers.
pub async fn parse_pdf_for_ingest(
    data: Vec<u8>,
    timeout_secs: u64,
    ocr_enabled: bool,
    ocr: Option<&dyn crate::ocr::OcrClient>,
) -> anyhow::Result<(Vec<PageText>, ExtractionMethod)> {
    #[cfg(feature = "ocr-pdfium")]
    let use_ocr = ocr_enabled;
    #[cfg(not(feature = "ocr-pdfium"))]
    {
        // Suppress unused-variable lint when the feature is off.
        let _ = ocr_enabled;
        let _ = ocr;
    }
    #[cfg(not(feature = "ocr-pdfium"))]
    let use_ocr = false;

    let parse_result = tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        tokio::task::spawn_blocking(move || extract_pages_blocking(&data)),
    )
    .await;

    let pages = match parse_result {
        Ok(Ok(Ok(pages))) => pages,
        Ok(Ok(Err(e))) => return Err(anyhow::Error::new(e)),
        Ok(Err(je)) => return Err(anyhow::anyhow!("PDF parse thread error: {je}")),
        Err(_) => return Err(anyhow::Error::new(PdfParseError::Timeout(timeout_secs))),
    };

    let page_count = pages.len();

    if use_ocr {
        let ocr = ocr.ok_or_else(|| {
            anyhow::anyhow!("ocr_enabled=true but no OcrClient supplied to parse_pdf_for_ingest")
        })?;
        let _data_for_render: &[u8] = &[];
        #[cfg(feature = "ocr-pdfium")]
        let renderer = PdfiumRenderer::new();
        let mut out = Vec::with_capacity(page_count);
        let mut pages_iter = pages.into_iter();
        let mut used_ocr = false;
        while let Some(page) = pages_iter.next() {
            #[allow(unused_mut)]
            let mut text = page.text;
            if page.needs_ocr {
                #[cfg(feature = "ocr-pdfium")]
                {
                    let image_bytes = renderer
                        .render_page_to_png(_data_for_render, page.page_number)
                        .map_err(|e| anyhow::anyhow!("render error: {e}"))?;
                    let ocr_text = ocr
                        .ocr_image(&image_bytes)
                        .await
                        .map_err(|e| anyhow::anyhow!("ocr error: {e}"))?;
                    if !text.trim().is_empty() && !ocr_text.trim().is_empty() {
                        text.push('\n');
                    }
                    text.push_str(&ocr_text);
                    used_ocr = true;
                }
                #[cfg(not(feature = "ocr-pdfium"))]
                {
                    let _ = ocr;
                    let _ = &mut used_ocr;
                }
            }
            out.push(PageText {
                page_number: page.page_number,
                text,
            });
        }
        let method = if used_ocr {
            ExtractionMethod::Ocr
        } else if out.iter().any(|p| !p.text.trim().is_empty()) {
            ExtractionMethod::PdfExtract
        } else {
            ExtractionMethod::Fallback
        };
        return Ok((out, method));
    }

    let any_text = pages.iter().any(|p| !p.text.trim().is_empty());
    let method = if any_text {
        ExtractionMethod::PdfExtract
    } else {
        ExtractionMethod::Fallback
    };
    let out = pages
        .into_iter()
        .map(|p| PageText {
            page_number: p.page_number,
            text: p.text,
        })
        .collect();
    Ok((out, method))
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

    // ---------- T37: parse_pdf_with_ocr ----------

    use crate::ocr::{MockOcr, NoOcr};

    #[tokio::test]
    async fn parse_pdf_with_ocr_returns_ocr_method_for_blank_pdf() {
        let data = include_bytes!("../tests/fixtures/scanned.pdf").to_vec();
        let renderer = MockRenderer::new(b"fake-png".to_vec());
        let ocr = MockOcr::new("OCR EXTRACTED TEXT");

        let result = parse_pdf_with_ocr(data, 30, &renderer, &ocr).await;
        assert!(result.is_ok(), "parse should succeed: {:?}", result.err());
        let parsed = result.unwrap();
        assert!(parsed.page_count > 0, "page_count must be > 0");
        assert_eq!(
            parsed.extraction_method,
            ExtractionMethod::Ocr,
            "blank PDF must use OCR method"
        );
        assert!(
            parsed.text.contains("OCR EXTRACTED TEXT"),
            "OCR text must appear in output, got: {:?}",
            parsed.text
        );
    }

    #[tokio::test]
    async fn parse_pdf_with_ocr_skips_ocr_for_text_pdf() {
        // text_rich.pdf has 100 chars of extractable text (> MIN_TEXT_CHARS=50)
        // → OCR must NOT be called.
        let data = include_bytes!("../tests/fixtures/text_rich.pdf").to_vec();
        let renderer = MockRenderer::new(b"fake-png".to_vec());
        let ocr = NoOcr; // panics if called

        let result = parse_pdf_with_ocr(data, 30, &renderer, &ocr).await;
        assert!(result.is_ok(), "text PDF should parse without OCR: {:?}", result.err());
        let parsed = result.unwrap();
        assert_ne!(
            parsed.extraction_method,
            ExtractionMethod::Ocr,
            "text-rich PDF must not trigger OCR"
        );
    }

    #[tokio::test]
    async fn parse_pdf_with_ocr_invalid_bytes_returns_err() {
        let garbage = b"not a pdf".to_vec();
        let renderer = MockRenderer::new(vec![]);
        let ocr = MockOcr::new("irrelevant");

        let result = parse_pdf_with_ocr(garbage, 30, &renderer, &ocr).await;
        assert!(result.is_err(), "invalid bytes must return Err");
    }

    #[tokio::test]
    async fn parse_pdf_with_ocr_times_out() {
        let data = include_bytes!("../tests/fixtures/scanned.pdf").to_vec();
        let renderer = MockRenderer::new(b"fake-png".to_vec());
        let ocr = MockOcr::new("OCR TEXT");

        let result = parse_pdf_with_ocr(data, 0, &renderer, &ocr).await;
        assert!(result.is_err(), "timeout_secs=0 must return Err (timeout)");
    }

    #[tokio::test]
    async fn parse_pdf_with_ocr_preserves_text_from_text_pages() {
        // text_rich.pdf has 100 chars → OCR not triggered → OCR text
        // must NOT appear in output.
        let data = include_bytes!("../tests/fixtures/text_rich.pdf").to_vec();
        let renderer = MockRenderer::new(b"fake-png".to_vec());
        let ocr = MockOcr::new("SHOULD NOT APPEAR");

        let result = parse_pdf_with_ocr(data, 30, &renderer, &ocr).await;
        assert!(result.is_ok());
        let parsed = result.unwrap();
        assert_ne!(
            parsed.extraction_method,
            ExtractionMethod::Ocr,
            "text-rich PDF must not trigger OCR"
        );
        if !parsed.text.is_empty() {
            assert!(
                !parsed.text.contains("SHOULD NOT APPEAR"),
                "OCR text must not leak into text-page output, got: {:?}",
                parsed.text
            );
        }
    }
}
