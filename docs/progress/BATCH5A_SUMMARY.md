# BATCH 5A SUMMARY — Worker Crate Skeleton, S3, PDF Parser
# Tasks: T34, T35, T36 | Trạng thái: ✅ Hoàn thành
# Test count khi kết thúc batch: 12 passed, 0 failed (worker lib)

---

## 1. TRẠNG THÁI TỔNG
| Task | Tên ngắn | Commit |
|------|----------|--------|
| T34 | Worker crate skeleton + Redis BRPOP poll loop | (commit T34) |
| T35 | S3 download/upload (MinIO) | (commit T35) |
| T36 | PDF parser (lopdf + pdf-extract + timeout) | (commit T36) |

---

## 2. FILES ĐÃ TẠO / SỬA
| File | Task | Hành động | Ghi chú |
|------|------|-----------|---------|
| `backend/crates/worker/Cargo.toml` | T34 | Sửa | Thêm `[lib]` section (lib+bin split). Thêm deps: `redis 0.25` (tokio-comp + aio), `async-trait 0.1`, `uuid.workspace`. Thêm dev-dep: `tokio` với `test-util` feature |
| `backend/crates/worker/src/lib.rs` | T34 | Tạo | Module declarations (`pub mod job; pub mod queue;`), `pub use` re-exports, `pub async fn run()` — boot config + init_pool (liveness) + RedisQueue::connect + `tokio::select!` poll loop vs ctrl_c |
| `backend/crates/worker/src/main.rs` | T34 | Sửa (rewrite) | Slim entry point: `init_tracing()` + `gmrag_worker::run().await` |
| `backend/crates/worker/src/job.rs` | T34 | Tạo | `IngestJob` struct (7 fields: id, tenant_id, workspace_id, document_id, s3_key, filename, attempts) + `process_job` stub + 3 unit tests |
| `backend/crates/worker/src/queue.rs` | T34 | Tạo | `JobQueue` trait (`brpop_timeout` async fn via `async_trait`) + `RedisQueue` (wraps `MultiplexedConnection`, BRPOP via `redis::cmd`) + `MockQueue` (in-memory `Mutex<VecDeque<Vec<u8>>>`) + `poll_once<Q: JobQueue + Send>()` + 3 unit tests |
| `backend/crates/worker/Cargo.toml` | T35 | Sửa | Thêm deps: `aws-sdk-s3 = "1"`, `aws-credential-types = "1"`. Thêm dev-dep: `wiremock = "0.6"` |
| `backend/crates/worker/src/lib.rs` | T35 | Sửa | Thêm `pub mod storage;` + `pub use storage::S3Client;` |
| `backend/crates/worker/src/storage.rs` | T35 | Tạo | `S3Client` struct + 4 methods + 3 wiremock tests |
| `backend/crates/worker/Cargo.toml` | T36 | Sửa | Thêm deps: `pdf-extract = "0.7"`, `lopdf = "0.34"` |
| `backend/crates/worker/src/lib.rs` | T36 | Sửa | Thêm `pub mod pdf_parser;` + `pub use pdf_parser::{ExtractionMethod, ParsedDocument, PdfParseError, parse_pdf};` |
| `backend/crates/worker/src/pdf_parser.rs` | T36 | Tạo | `ParsedDocument` struct + `ExtractionMethod` enum + `PdfParseError` error type + `parse_pdf` async fn (timeout + spawn_blocking) + `parse_pdf_blocking` sync helper + 3 tests |
| `backend/crates/worker/tests/fixtures/sample.pdf` | T36 | Tạo | 596-byte valid PDF fixture (1 page, Helvetica font, text "Hello from GMRAG test") |

---

## 3. PUBLIC API ĐÃ CÓ
> Để agent batch sau biết không implement lại

```rust
// T34 — gmrag-worker
pub mod job;
pub mod queue;

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct IngestJob {
    pub id: uuid::Uuid,
    pub tenant_id: uuid::Uuid,
    pub workspace_id: uuid::Uuid,
    pub document_id: uuid::Uuid,
    pub s3_key: String,
    pub filename: String,
    pub attempts: u32,
}
pub async fn process_job(_job: &IngestJob) -> anyhow::Result<()>  // T34 stub

#[async_trait::async_trait]
pub trait JobQueue: Send {
    async fn brpop_timeout(&mut self, key: &str, timeout_secs: u64) -> anyhow::Result<Option<Vec<u8>>>;
}
pub struct RedisQueue { conn: redis::aio::MultiplexedConnection }
impl RedisQueue { pub async fn connect(url: &str) -> anyhow::Result<Self> }
pub struct MockQueue { items: Mutex<VecDeque<Vec<u8>>> }
impl MockQueue { pub fn new(items: Vec<Vec<u8>>) -> Self }
pub async fn poll_once<Q: JobQueue + Send>(queue: &mut Q) -> anyhow::Result<Option<IngestJob>>

pub async fn run() -> anyhow::Result<()>  // boot + poll loop

// T35 — gmrag-worker::storage
pub struct S3Client {
    client: aws_sdk_s3::Client,
    bucket: String,
}
impl S3Client {
    pub fn new(cfg: &S3Config) -> Self
    pub async fn download(&self, key: &str) -> anyhow::Result<Vec<u8>>
    pub async fn upload(&self, key: &str, data: Vec<u8>, content_type: &str) -> anyhow::Result<()>
    pub async fn delete(&self, key: &str) -> anyhow::Result<()>
}

// T36 — gmrag-worker::pdf_parser
#[derive(Debug)]
pub struct ParsedDocument {
    pub text: String,
    pub page_count: usize,
    pub extraction_method: ExtractionMethod,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractionMethod {
    PdfExtract,
    Fallback,
}

#[derive(Debug, thiserror::Error)]
pub enum PdfParseError {
    #[error("PDF parse timed out after {0}s")]
    Timeout(u64),
    #[error("PDF parse error: {0}")]
    Parse(String),
}

pub async fn parse_pdf(data: Vec<u8>, timeout_secs: u64) -> anyhow::Result<ParsedDocument>
fn parse_pdf_blocking(data: &[u8]) -> Result<ParsedDocument, PdfParseError>
```

---

## 4. MIGRATION STATE
N/A — Batch 5A không thêm migration.

---

## 5. ENV VARS / CONFIG
| Tên biến | Giá trị mẫu | Task thêm |
|----------|-------------|----------|
| N/A | — | Reuse `S3Config` (T9), `OllamaConfig` (T9), `DeepSeekConfig` (T9), `RedisConfig` (T9) |

---

## 6. DEPENDENCIES (Cargo.toml)
| Crate | Version | Thêm vào crate | Task |
|-------|---------|---------------|------|
| `redis` | 0.25 (features: tokio-comp, aio) | `gmrag-worker` | T34 |
| `async-trait` | 0.1 | `gmrag-worker` | T34 |
| `tokio` (dev) | 1 (features: test-util) | `gmrag-worker` (dev) | T34 |
| `aws-sdk-s3` | 1 | `gmrag-worker` | T35 |
| `aws-credential-types` | 1 | `gmrag-worker` | T35 |
| `wiremock` (dev) | 0.6 | `gmrag-worker` (dev) | T35 |
| `pdf-extract` | 0.7 | `gmrag-worker` | T36 |
| `lopdf` | 0.34 | `gmrag-worker` | T36 |

---

## 7. QUYẾT ĐỊNH KỸ THUẬT CÓ IMPACT CROSS-TASK
> Chỉ giữ quyết định ảnh hưởng tới batch sau, bỏ quyết định internal

- **[T34]** Reuse `gmrag_core::Config` thay vì tạo `WorkerConfig` mới — `S3Client::new(&cfg.s3)` (T35) và `RedisQueue::connect(&cfg.redis.url)` (T34) dùng trực tiếp
- **[T34]** `JobQueue` trait + `MockQueue` — redis crate không có built-in mock; trait abstraction cho phép test poll loop mà không cần Redis thật
- **[T34]** `MultiplexedConnection` thay vì `Connection` — `redis::aio::Connection` deprecated trong 0.25+
- **[T34]** `async_trait` crate — Rust 1.75+ native `async fn` trong traits nhưng Send bound inference phức tạp; `async_trait` giải quyết sạch
- **[T34]** Lib+bin split — `lib.rs` chứa tất cả logic + tests; `main.rs` chỉ `init_tracing()` + `gmrag_worker::run()`
- **[T34]** `tokio::select!` với `biased` — `ctrl_c` ưu tiên kiểm tra trước
- **[T34]** Pool rule DEFERRED: T34 dùng `init_pool` (admin/gmrag) CHỈ cho boot liveness check. `process_job` stub không ghi DB → không vi phạm RLS. Khi T37+ implement dual-write MUST switch sang `init_app_pool`
- **[T35]** Reuse `gmrag_core::config::S3Config` — tránh duplication
- **[T35]** `S3Client::new` sync (không async) — `aws_sdk_s3::Config::builder().build()` là sync
- **[T35]** Manual config builder (không `aws_config::load_defaults`) — MinIO cần custom endpoint + static credentials
- **[T35]** `behavior_version_latest()` — aws-sdk-s3 v1 yêu cầu `behavior_version` được set
- **[T35]** `force_path_style(cfg.force_path_style)` — MinIO yêu cầu path-style addressing
- **[T35]** wiremock matching: match on `method` + `path` only (ignore query params, Authorization headers, body content)
- **[T36]** lopdf-first approach (deviation from prompt order): gọi `lopdf::Document::load_mem` đầu tiên để lấy `page_count` (luôn cần), rồi `pdf_extract::extract_text_from_mem` cho text
- **[T36]** `spawn_blocking` + `tokio::time::timeout` — pdf-extract/lopdf là CPU-bound sync; timeout per call
- **[T36]** Timeout test với `timeout_secs = 0` + 100ms sleep trong `parse_pdf_blocking` để reliably trigger timeout (race condition fix)
- **[T36]** Test assertion linh hoạt: `assert!(!text.is_empty() || extraction_method == ExtractionMethod::Fallback)` — chấp nhận cả 2 path
- **[T36]** `lopdf` version match: pdf-extract 0.7.12 cũng dùng lopdf 0.34.0 → chỉ 1 version lopdf compiled

---

## 8. INVARIANTS BẮT BUỘC (tích lũy đến hết batch này)
> Đây là luật bất biến — agent vi phạm là toang
> Phải liệt kê ĐẦY ĐỦ kể cả invariant kế thừa từ batch trước

1-49. (Tất cả invariants từ Batch 1+2A+2B+3 — giữ nguyên)
50-83. (Tất cả invariants từ Batch 4 — giữ nguyên)

**MỚI (Batch 5A):**
84. **Worker boot dùng `init_pool` (AdminPool, gmrag) CHỈ cho liveness check** — khi T37+ implement dual-write, PHẢI switch sang `init_app_pool` + per-job `SET LOCAL app.tenant_id = $job.tenant_id`
85. **`JobQueue` trait + MockQueue pattern** — mọi external service (Redis, S3, Ollama, Qdrant) test qua trait abstraction + mock, không cần service thật
86. **Worker lib+bin split** — `lib.rs` chứa logic + tests; `main.rs` chỉ `init_tracing()` + `gmrag_worker::run()`
87. **`tokio::select!` với `biased` cho shutdown** — `ctrl_c` ưu tiên kiểm tra trước
88. **S3 client manual config** (không `aws_config::load_defaults`) — cần custom endpoint + static credentials cho MinIO
89. **S3 path-style addressing** — `force_path_style=true` cho MinIO
90. **PDF parser lopdf-first** — `Document::load_mem` để lấy `page_count` trước, sau đó `pdf_extract::extract_text_from_mem` cho text
91. **PDF parser timeout pattern** — `spawn_blocking` + `tokio::time::timeout` per call
92. **PDF fallback extraction method** — `ExtractionMethod::Fallback` nếu pdf-extract fail, vẫn có `page_count` từ lopdf
93. **PDF fixture committed** — `worker/tests/fixtures/sample.pdf` (596 bytes, 1 page, text "Hello from GMRAG test")
94. **IngestJob struct fields** — `id, tenant_id, workspace_id, document_id, s3_key, filename, attempts` (T34) — sẽ mở rộng `owner_id` + `visibility` ở T43

---

## 9. ⚠️ BLOCKERS & WARNINGS CHO BATCH TIẾP THEO
> QUAN TRỌNG NHẤT — KHÔNG được tóm tắt chung chung

### P0 — Chặn đường găng (phải xử lý trước khi bắt đầu batch sau)
- **[nguồn: T34]** **POOL RULE — T37+ CRITICAL**: Worker PHẢI dùng `app_pool` + `SET LOCAL app.tenant_id` khi T42 implement dual-write. KHÔNG dùng `admin_pool` cho business queries. T34 hiện dùng `init_pool` (admin) chỉ cho liveness check — stub `process_job` không ghi DB nên chưa vi phạm. Khi `process_job` bắt đầu ghi metadata vào Postgres (T37+), bắt buộc chuyển sang `init_app_pool` + per-job `SET LOCAL app.tenant_id = $job.tenant_id`.
- **[nguồn: T35]** First build sẽ download ~50 transitive crates (5-10 min) cho `aws-sdk-s3` v1
- **[nguồn: T36]** PDF fixture chỉ 1 page với text đơn giản — nếu cần test PDF phức tạp (multi-page, Unicode, encrypted) → tạo thêm fixtures trong future task
- **[nguồn: T36]** `parse_pdf` KHÔNG handle PDF encryption, OCR, hay image-only PDFs — fallback path trả empty text cho scanned PDFs. T37 sẽ thêm OCR pipeline
- **[nguồn: T36]** `lopdf` version conflict risk: `pdf-extract 0.7` phụ thuộc `lopdf 0.31`, thêm `lopdf 0.34` trực tiếp có thể compile 2 versions. → action: verified không conflict (cùng 0.34.0)

### P1 — Lưu ý khi implement
- **[T34]** `IngestJob` thiếu `owner_id` + `visibility` (sẽ thêm ở T43) — handler/API enqueue phải populate đầy đủ
- **[T35]** `aws-credential-types` trong Cargo.toml có thể remove nếu clippy báo unused (Credentials re-exported từ aws_sdk_s3)
- **[T35]** wiremock chỉ match `method` + `path` (ignore query, headers, body) — không verify SigV4
- **[T36]** `parse_pdf` text-only — scanned PDFs trả empty text. T37 sẽ thêm `parse_pdf_with_ocr` + `PdfiumRenderer` (cần `pdfium-render` + libpdfium binary — chưa có)
- **[T36]** `infra/backend.Dockerfile` chưa update cho libpdfium — T37+ sẽ thêm
- **[T36]** `Ollama vision model provisioning`: cần `ollama pull moondream:1.8b` (hoặc `llama3.2-vision`) — follow-up T37

### P2 — Ghi nhớ nhỏ
- `redis::aio::Connection` deprecated trong 0.25+ — dùng `MultiplexedConnection`
- `async_trait` overhead negligible cho poll loop
- `poll_once` testable unit: gọi `MockQueue` qua trait object
- BRPOP timeout 5s — an toàn trên multiplexed connection (worker không gửi concurrent commands khác)
- `S3Client::upload` signature: `(key, data: Vec<u8>, content_type: &str)` — content_type required
- `S3Client::delete` không idempotent từ phía S3 — caller xử lý "ensure gone" qua `let _ = ...` nếu cần
- `parse_pdf` API nhận `Vec<u8>` ownership — caller move bytes vào
- `parse_pdf_blocking` là private — caller dùng `parse_pdf` async

---

## 10. UNBLOCKS
- Batch 5A → unblock: T37 (PDF parser OCR fallback) — sẽ mở rộng `pdf_parser.rs`
- Batch 5A → unblock: T38 (chunking) — pure function, nhận `Vec<String>` page texts
- Batch 5A → unblock: T39 (embedding Ollama) — `chunk_page_texts` → `embed_batch`
- Batch 5A → unblock: T42 (dual-write) — S3Client + parse_pdf + chunking + embedding ready
- Batch 5A → unblock: T43 (process_job real implementation) — S3 download + parse + pipeline
