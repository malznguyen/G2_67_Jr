# BATCH 5B SUMMARY — PDF OCR, Chunking, Embedding, BYOK LLM
# Tasks: T37, T38, T39, T40 | Trạng thái: ✅ Hoàn thành
# Test count khi kết thúc batch: 31 passed, 0 failed (worker: 25 lib + 6 integration)

---

## 1. TRẠNG THÁI TỔNG
| Task | Tên ngắn | Commit |
|------|----------|--------|
| T37 | PDF parser OCR fallback (Ollama vision) + per-page text extraction | (commit T37) |
| T38 | Chunking: tiktoken cl100k 1200/100 via text-splitter | (commit T38) |
| T39 | Embedding: OllamaEmbedder `/api/embed` batch + retry/backoff | (commit T39) |
| T40 | BYOK embed trait + OpenAiEmbedder (768-dim pinned) + tenant_llm_config migration + select_embedder factory | (commit T40) |

---

## 2. FILES ĐÃ TẠO / SỬA
| File | Task | Hành động | Ghi chú |
|------|------|-----------|---------|
| `backend/crates/worker/Cargo.toml` | T37 | Sửa | Thêm dep: `base64 0.22` |
| `backend/crates/worker/src/ocr.rs` | T37 | Tạo | `OcrClient` trait + `OllamaVisionOcr` (POST /api/chat + base64 image + retry/backoff) + `MockOcr` + `NoOcr` + `OcrError` enum + 6 wiremock tests |
| `backend/crates/worker/src/pdf_parser.rs` | T37 | Sửa (mở rộng T36) | Thêm `ExtractionMethod::Ocr` + `PageRenderer` trait + `MockRenderer` + `RenderError` + `PdfPage` + `extract_pages_blocking` + `extract_page_text` (per-page via `output_doc_page`) + `parse_pdf_with_ocr` + 5 tests. T36 `parse_pdf` + 3 tests giữ nguyên |
| `backend/crates/worker/src/lib.rs` | T37 | Sửa | Thêm `pub mod ocr;` + re-exports `OcrClient, OcrError, OllamaVisionOcr, MockOcr, NoOcr, PageRenderer, MockRenderer, RenderError, parse_pdf_with_ocr` |
| `backend/crates/worker/tests/fixtures/scanned.pdf` | T37 | Tạo | 329-byte blank PDF (1 page, no content stream, no text layer) — needs_ocr=true |
| `backend/crates/worker/tests/fixtures/text_rich.pdf` | T37 | Tạo | 725-byte PDF (1 page, Helvetica, 100 chars extractable text "The quick brown fox...") — > MIN_TEXT_CHARS=50 → needs_ocr=false |
| `backend/crates/worker/Cargo.toml` | T38 | Sửa | Thêm deps: `tiktoken-rs 0.11`, `text-splitter 0.30` (feature `tiktoken-rs`) |
| `backend/crates/worker/src/chunking.rs` | T38 | Tạo | `chunk_page_texts` pure function + `ChunkError` enum + 8 unit tests |
| `backend/crates/worker/src/lib.rs` | T38 | Sửa | Thêm `pub mod chunking;` + `pub use chunking::{ChunkError, chunk_page_texts};` |
| `backend/migrations/20260618100000_tenant_llm_config.sql` | T40 | Tạo | `tenant_llm_config` table (PK=tenant_id) + RLS + GRANT. Plaintext api_key (MVP) |
| `backend/crates/worker/src/embedding.rs` | T39 | Tạo | `OllamaEmbedder` struct + builder + `embed_one` + `embed_batch` + `embed_batch_with_retry` + `EmbedError` enum + req/resp serde structs + 8 wiremock tests |
| `backend/crates/worker/src/lib.rs` | T39 | Sửa | Thêm `pub mod embedding;` + `pub use embedding::{EmbedError, OllamaEmbedder};` |
| `backend/crates/worker/Cargo.toml` | T39 | Sửa | Thêm deps: `reqwest 0.12` (json, rustls-tls, default-features=false), `futures 0.3` |
| `backend/crates/worker/src/embedding.rs` | T40 | Sửa (mở rộng T39) | Thêm `EMBED_DIM=768` const + `EmbedFuture` type alias + `Embedder` trait + `impl Embedder for OllamaEmbedder` + `OpenAiEmbedder` struct + impl + `TenantLlmConfig` struct + `select_embedder` factory + `OpenAiEmbedRequest`/`OpenAiEmbedResponse` serde structs + 5 OpenAI/trait tests |
| `backend/crates/worker/src/lib.rs` | T40 | Sửa | Re-exports: `Embedder, OpenAiEmbedder, TenantLlmConfig, select_embedder` |
| `backend/crates/worker/tests/select_embedder.rs` | T40 | Tạo | 6 `#[sqlx::test]` integration tests cho `select_embedder` |

---

## 3. PUBLIC API ĐÃ CÓ
> Để agent batch sau biết không implement lại

```rust
// T37 — gmrag-worker::ocr
pub trait OcrClient: Send + Sync {
    fn ocr_image<'a>(&'a self, image_bytes: &'a [u8]) -> Pin<Box<dyn Future<Output = Result<String, OcrError>> + Send + 'a>>;
}
pub struct OllamaVisionOcr { client, url, model, timeout, retries, backoff_ms }
impl OcrClient for OllamaVisionOcr
pub struct MockOcr { text: String }
pub struct NoOcr;  // panics if called (verifies OCR NOT triggered)
pub enum OcrError { Http, Timeout, ... }

// T37 — pdf_parser additions
pub enum ExtractionMethod { PdfExtract, Fallback, Ocr }  // +Ocr variant
pub trait PageRenderer: Send + Sync {
    fn render_page_to_png(&self, data: &[u8], page_number: u32) -> Result<Vec<u8>, RenderError>;
}
pub struct MockRenderer { image_bytes: Vec<u8> }
pub struct PdfPage { page_number: u32, text: String }
pub async fn parse_pdf_with_ocr(
    data: Vec<u8>, timeout_secs: u64,
    renderer: &dyn PageRenderer, ocr: &dyn OcrClient,
) -> anyhow::Result<ParsedDocument>

// T38 — gmrag-worker::chunking
const CHUNK_SIZE_TOKENS: usize = 1200;
const CHUNK_OVERLAP_TOKENS: usize = 100;
pub fn chunk_page_texts(page_texts: &[String]) -> Result<Vec<String>, ChunkError>
pub enum ChunkError { Tokenizer(String), Config(String) }

// T39 — gmrag-worker::embedding
pub struct OllamaEmbedder {
    client: reqwest::Client,
    url: String, model: String,
    batch_size: usize, concurrency: usize,
    timeout: Duration, retries: usize, backoff_ms: u64,
}
impl OllamaEmbedder {
    pub fn new(cfg: &OllamaConfig) -> Self
    pub fn new_with_url(host: &str, model: &str) -> Self
    pub fn with_batch_size/concurrency/timeout_secs/retries/backoff_ms(self, ...) -> Self
    pub fn url(&self) -> &str
    pub fn model(&self) -> &str
    pub async fn embed_one(&self, text: &str) -> Result<Vec<f32>, EmbedError>
    pub async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError>
    async fn embed_batch_with_retry(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError>
}
pub enum EmbedError { Http, Timeout(u64), Empty, CountMismatch { expected, actual } }

// T40 — Embedder trait
pub const EMBED_DIM: usize = 768;
pub type EmbedFuture<'a> = Pin<Box<dyn Future<Output = Result<Vec<Vec<f32>>, EmbedError>> + Send + 'a>>;
pub trait Embedder: Send + Sync {
    fn embed_batch<'a>(&'a self, texts: &'a [String]) -> EmbedFuture<'a>;
    fn dimension(&self) -> usize { EMBED_DIM }
    fn provider(&self) -> &str;
}
impl Embedder for OllamaEmbedder

// T40 — OpenAiEmbedder
pub struct OpenAiEmbedder { client, url, api_key, model, batch_size, concurrency, timeout, retries, backoff_ms }
impl OpenAiEmbedder {
    pub fn new(api_key: String, model: &str, base_url: Option<&str>) -> Self
    pub fn with_batch_size/concurrency/timeout_secs/retries/backoff_ms(self, ...) -> Self
    pub fn url(&self) -> &str
    pub fn model(&self) -> &str
    pub async fn embed_one(&self, text: &str) -> Result<Vec<f32>, EmbedError>
}
impl Embedder for OpenAiEmbedder { fn provider(&self) -> &str { "openai" } }

// T40 — TenantLlmConfig + factory
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TenantLlmConfig { provider, api_key, model, base_url, dimensions, enabled }

pub async fn select_embedder(
    pool: &PgPool, tenant_id: Uuid, ollama_cfg: &OllamaConfig
) -> Result<Box<dyn Embedder>, EmbedError>
```

---

## 4. MIGRATION STATE
| File migration | Version timestamp | Bảng tạo ra |
|---------------|------------------|-------------|
| (Tất cả migrations từ Batch 1-3 + 4 unchanged) | | |
| `20260618100000_tenant_llm_config.sql` (T40) | `20260618100000` | `tenant_llm_config` (PK=tenant_id) + RLS + GRANT to gmrag_app |

RLS đang enforce trên: 16 bảng cũ + `tenant_llm_config` = 17 bảng
sqlx offline cache: có (2 files từ T26)

---

## 5. ENV VARS / CONFIG
N/A — Batch 5B không thêm env mới (reuse existing).

---

## 6. DEPENDENCIES (Cargo.toml)
| Crate | Version | Thêm vào crate | Task |
|-------|---------|---------------|------|
| `base64` | 0.22 | `gmrag-worker` | T37 |
| `tiktoken-rs` | 0.11 | `gmrag-worker` | T38 |
| `text-splitter` | 0.30 (feature `tiktoken-rs`) | `gmrag-worker` | T38 |
| `reqwest` | 0.12 (features: json, rustls-tls; default-features=false) | `gmrag-worker` | T39 |
| `futures` | 0.3 | `gmrag-worker` | T39 |

---

## 7. QUYẾT ĐỊNH KỸ THUẬT CÓ IMPACT CROSS-TASK
> Chỉ giữ quyết định ảnh hưởng tới batch sau, bỏ quyết định internal

- **[T37]** `PageRenderer` trait thay vì `pdfium-render` trực tiếp — T37 delivery = OCR orchestration logic; `PdfiumRenderer` (production) là follow-up
- **[T37]** `OcrClient` trait + `MockOcr` + `NoOcr` — `NoOcr` panics if called (verifies OCR NOT triggered for text PDFs)
- **[T37]** Per-page text extraction via `output_doc_page` — T36 dùng `extract_text_from_mem` (whole-document); T37 cần per-page để detect thin text
- **[T37]** `MIN_TEXT_CHARS=50` threshold — page <50 chars → `needs_ocr=true`
- **[T37]** `spawn_blocking` + 3-level `Result`: `Result<Result<Result<Vec<PdfPage>, PdfParseError>, JoinError>, Elapsed>` — match 3 `Ok` layers
- **[T37]** `data.clone()` cho render — `data_for_render = data.clone()` trước move vào `spawn_blocking`
- **[T37]** `ExtractionMethod::Ocr` khi bất kỳ page dùng OCR — final method: `Ocr` if used_ocr, `PdfExtract` if text without OCR, `Fallback` if no text
- **[T37]** Ollama `/api/chat` format: `{model, messages:[{role:"user", content:OCR_PROMPT, images:[base64]}], stream:false}` → `{message:{role:"assistant", content:"text"}}`
- **[T37]** `OCR_PROMPT = "Extract all text from this image. Output only the text, nothing else."`
- **[T38]** Port nguyên vẹn v1 `chunking.rs` — logic giống hệt, chỉ thêm `thiserror::Error` derive
- **[T38]** `CHUNK_SIZE_TOKENS=1200` + `CHUNK_OVERLAP_TOKENS=100` — match v1 + sheet "Kế hoạch Task" R43
- **[T38]** Pure function, no async — `cl100k_base()` và `TextSplitter::chunks()` CPU-bound sync
- **[T39]** `new_with_url(host, model)` constructor cho tests — wiremock URI
- **[T39]** Builder pattern cho tuning: `with_batch_size`/`with_concurrency`/`with_timeout_secs`/`with_retries`/`with_backoff_ms` — thay vì read env trực tiếp
- **[T39]** `buffer_unordered` + stitch by index — batch song song → output order = input order
- **[T39]** Retry/backoff: `for attempt in 0..=retries`, backoff `backoff_ms * 2^attempt` capped `2^BACKOFF_CAP_POWER=6` (max 16s với default 250ms)
- **[T39]** Empty input short-circuit — `if texts.is_empty() { return Ok(Vec::new()); }` trước khi touch network
- **[T39]** CountMismatch per-batch — mỗi batch check `parsed.embeddings.len() != texts.len()`
- **[T39]** `reqwest` features match api crate: `default-features=false` + `json` + `rustls-tls`
- **[T40]** `dimensions=768` pinned cho OpenAI: `OpenAiEmbedRequest` gửi `{model, input, dimensions: 768}` — vector 768-dim khớp `QdrantStore::EMBED_DIM=768`
- **[T40]** `Embedder` trait + boxed future — `Box::pin` trong impl cho phép `async move` block
- **[T40]** `select_embedder` RLS-correct: `BEGIN → SET LOCAL ROLE gmrag_app → SET LOCAL app.tenant_id = '{tenant_id}' → SELECT → COMMIT`
- **[T40]** Plaintext api_key (MVP) — `api_key TEXT NULL` trong `tenant_llm_config`. RLS enforced (FORCE + policy) → chỉ tenant owner thấy key. Follow-up: encrypt bằng `pgp_sym_encrypt` (T45 API sẽ làm)
- **[T40]** Fallback logic 3-level: (1) no row → ollama; (2) row `provider='ollama'` → ollama; (3) row `provider='openai'` + `api_key IS NULL` → ollama; (4) row `enabled=false` → ollama
- **[T40]** Integration test file riêng: `tests/select_embedder.rs` (không trong lib unit tests) — vì `#[sqlx::test]` cần DB sống
- **[T40]** No `.sqlx` offline cache needed — `select_embedder` dùng `sqlx::query_as::<_, T>(...)` FUNCTION (runtime), không phải MACRO

---

## 8. INVARIANTS BẮT BUỘC (tích lũy đến hết batch này)
> Đây là luật bất biến — agent vi phạm là toang
> Phải liệt kê ĐẦY ĐỦ kể cả invariant kế thừa từ batch trước

1-94. (Tất cả invariants từ Batch 1+2A+2B+3+4+5A — giữ nguyên)

**MỚI (Batch 5B):**
95. **Vector dim = 768, Distance = Cosine** (const `EMBED_DIM = 768`) — khớp `QdrantStore::EMBED_DIM` + OpenAI `text-embedding-3-small` với `dimensions=768`
96. **`select_embedder` RLS-correct** từ đầu — dùng `SET LOCAL ROLE gmrag_app` + `SET LOCAL app.tenant_id` (mirror middleware pattern)
97. **`tenant_llm_config` table**: PK=tenant_id, RLS FORCE + policy `tenant_id = gmrag_current_tenant()`, GRANT to gmrag_app. Plaintext api_key (MVP) — sẽ upgrade encrypted ở T45
98. **`OllamaEmbedder` reuse qua `select_embedder`** — fallback mặc định khi tenant không có BYOK config
99. **`OpenAiEmbedder` pin `dimensions=768`** — request `{model, input, dimensions: 768}` → 768-dim vector khớp Qdrant collection
100. **Per-page text extraction** cho OCR orchestration — `output_doc_page` thay vì whole-document
101. **`MIN_TEXT_CHARS=50`** threshold cho OCR trigger — page <50 chars → needs_ocr=true
102. **`ExtractionMethod::Ocr` khi bất kỳ page OCR** — final method: `Ocr` if used_ocr, `PdfExtract` if text without OCR, `Fallback` if no text
103. **Chunking config**: `CHUNK_SIZE_TOKENS=1200`, `CHUNK_OVERLAP_TOKENS=100` — match v1
104. **Chunking pure function** — no async; caller (T42) gọi trực tiếp, không cần `spawn_blocking`
105. **Empty input short-circuit** cho embed_batch — tránh wiremock "no matching mock" error
106. **CountMismatch per-batch** — không check total
107. **`reqwest` features match api crate**: `default-features=false` + `json` + `rustls-tls` — giữ TLS backend nhất quán
108. **Builder pattern cho OllamaEmbedder/OpenAiEmbedder** — testable, không phụ thuộc env
109. **`buffer_unordered(concurrency)` + stitch by index** — output order = input order dù batch chạy song song
110. **`embed_batch_with_retry` retry/backoff**: `for attempt in 0..=retries` (retries+1 total), backoff `backoff_ms * 2^attempt` capped `2^6=64x`
111. **Integration test file riêng** cho `#[sqlx::test]` — tách khỏi lib unit tests (wiremock, không cần DB)
112. **No `.sqlx` offline cache needed** cho `sqlx::query_as::<_, T>(...)` FUNCTION — chỉ `query!`/`query_as!` MACRO mới cần
113. **Plaintext api_key MVP** — follow-up T45 API sẽ thêm encryption bằng `pgp_sym_encrypt` + `GMRAG_TENANT_KEY_ENCRYPTION_KEY` env

---

## 9. ⚠️ BLOCKERS & WARNINGS CHO BATCH TIẾP THEO
> QUAN TRỌNG NHẤT — KHÔNG được tóm tắt chung chung

### P0 — Chặn đường găng (phải xử lý trước khi bắt đầu batch sau)
- **[nguồn: T37]** `PdfiumRenderer` chưa implement: T37 define `PageRenderer` trait + `MockRenderer` nhưng chưa implement `PdfiumRenderer` (cần `pdfium-render` crate + libpdfium binary). Follow-up task: add `pdfium-render` dep + download libpdfium (bblanchon/pdfium-binaries) cho Docker build stage + Windows dev.
- **[nguồn: T37]** `infra/backend.Dockerfile` chưa update cho libpdfium — cần thêm libpdfium vào build + runtime stage khi implement `PdfiumRenderer`. Download từ `bblanchon/pdfium-binaries` GitHub release, đặt `libpdfium.so` vào `/usr/lib`, set `LD_LIBRARY_PATH`.
- **[nguồn: T37]** Ollama vision model provisioning: cần `ollama pull moondream:1.8b` (hoặc `llama3.2-vision`) bên cạnh `nomic-embed-text` đã có. Add vào provisioning script/README.
- **[nguồn: T37]** OCR timeout: `parse_pdf_with_ocr` timeout chỉ cover phase 1 (lopdf load + per-page text extract). OCR calls (phase 2) có riêng timeout trong `OllamaVisionOcr` (default 60s). Nếu document có nhiều scanned pages, tổng OCR time = N_pages × 60s. Follow-up: stage-level timeout cho toàn bộ OCR phase.
- **[nguồn: T38]** OCR/PDF text feed: T41 `extract(text)` nhận raw text. T43 sẽ truyền `parsed.text` (từ `parse_pdf_with_ocr`) hoặc join chunks. Document rất dài → cần chunk-level extraction hoặc truncate input trong future perf task (ngoài scope T41).
- **[nguồn: T40]** DB password: Test cần `DATABASE_URL=postgres://gmrag:7d52bde8138028a77dde2eb1574c33b6@localhost:5432/gmrag` (override `postgres16`→`localhost` khi chạy từ host, per HOTFIX_PRE_BATCH5.md:95).
- **[nguồn: T40]** `dimensions=768` mất một nửa thông tin embedding (768 thay vì 1536 native OpenAI) — acceptable cho MVP, có thể mở rộng sau

### P1 — Lưu ý khi implement
- **[T37]** T42 (dual-write): `process_job` real implementation sẽ dùng `parse_pdf_with_ocr(bytes, 30, &renderer, &ocr)` — pipeline đầy đủ: S3 → parse → chunk → embed → Qdrant + Postgres dual-write
- **[T37]** `Base64::Engine` trait import cần thiết (test fail nếu quên)
- **[T37]** `spawn_blocking` 3-level `Result` — match `Ok(Ok(Ok(pages)))` cho `Vec<PdfPage>`
- **[T37]** `text_rich.pdf` fixture >50 chars → OCR not triggered; `scanned.pdf` 0 chars → OCR triggered
- **[T37]** Test `NoOcr` panic khi gọi — verifies OCR NOT triggered for text PDFs
- **[T38]** Chunking không dùng `spawn_blocking` (nhanh hơn PDF parsing) — caller gọi trực tiếp
- **[T39]** `new_with_url` constructor cho tests — production dùng `new(&OllamaConfig)`
- **[T39]** URL = `format!("{}/api/embed", host.trim_end_matches('/'))` — trim trailing slash
- **[T39]** Vec768 test helper: `vec![seed; 768]` — distinguished bằng `seed`
- **[T40]** Wiring `select_embedder` vào `process_job` + switch worker sang `init_app_pool` — thuộc T42 (dual-write), KHÔNG phải T40
- **[T40]** T40 chỉ cung cấp factory function, không wire vào process_job

### P2 — Ghi nhớ nhỏ
- `tenant_llm_config` table columns: `tenant_id (PK)`, `provider`, `api_key`, `model`, `base_url`, `dimensions`, `enabled`, `created_at`, `updated_at`
- `select_embedder` RLS tx: `BEGIN → SET LOCAL ROLE gmrag_app → SET LOCAL app.tenant_id → SELECT → COMMIT`
- `OpenAiEmbedRequest`: `{model, input, dimensions}` → `OpenAiEmbedResponse`: `{data:[{embedding, index}]}`
- `OpenAiEmbedder::bearer_auth(&self.api_key)` — reqwest builder set `Authorization: Bearer {key}`
- `parse_pdf_with_ocr` cần `data.clone()` cho render vì `data` moved vào `spawn_blocking`
- `OllamaVisionOcr` gọi `{host}/api/chat` với vision model (moondream hoặc llama3.2-vision)
- `chunking.rs` port từ v1 — đã production-tested, giữ stable
- Test fixture `text_rich.pdf` (725 bytes, 100 chars) > MIN_TEXT_CHARS=50 → OCR NOT triggered
- Test fixture `scanned.pdf` (329 bytes, 0 chars) → OCR triggered
- `OcrClient::ocr_image` return `Pin<Box<dyn Future + Send>>` — cần `'a` lifetime cho borrow `&[u8]`

---

## 10. UNBLOCKS
- Batch 5B → unblock: T41 (Graph extraction via DeepSeek + tolerant JSON parser) — sẽ dùng `OllamaEmbedder` pattern
- Batch 5B → unblock: T42 (dual-write ingestion) — `select_embedder` + `chunk_page_texts` + `parse_pdf_with_ocr` ready
- Batch 5B → unblock: T43 (process_job real implementation) — pipeline đầy đủ components
- Batch 5B → unblock: API RAG query endpoint (kNN search chunks + embed query)
- Batch 5B → unblock: API GraphRAG query endpoint
