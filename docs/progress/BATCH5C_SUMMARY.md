# BATCH 5C SUMMARY — Graph Extraction, Dual-Write, Complete Ingestion Pipeline
# Tasks: T41, T42, T43 | Trạng thái: ✅ Hoàn thành
# Test count khi kết thúc batch: 69 passed, 0 failed (worker: 52 lib + 3 + 3 + 6 + 5 = 69 integration/lib)

---

## 1. TRẠNG THÁI TỔNG
| Task | Tên ngắn | Commit |
|------|----------|--------|
| T41 | Graph extraction via DeepSeek + tolerant JSON parser + idempotency schema | (commit T41) |
| T42 | Idempotent dual-write (Postgres metadata + Qdrant vectors) in one transaction | (commit T42) |
| T43 | Complete ingestion pipeline execution + in-memory retry logic + DB status tracking | (commit T43) |

---

## 2. FILES ĐÃ TẠO / SỬA
| File | Task | Hành động | Ghi chú |
|------|------|-----------|---------|
| `backend/migrations/20260618110000_graph_idempotency_and_llm.sql` | T41 | Tạo | `ALTER TABLE graph_nodes ADD COLUMN workspace_id UUID NULL REFERENCES workspaces(id) ON DELETE CASCADE` + `UNIQUE(tenant_id, workspace_id, label, kind)` + `idx_graph_nodes_workspace` + `ALTER TABLE tenant_llm_config ADD COLUMN llm_model TEXT NULL, llm_base_url TEXT NULL` |
| `backend/crates/worker/src/graph.rs` | T41 | Tạo | `DeepSeekGraphExtractor` (OpenAI-compatible `/chat/completions` + `response_format: json_object` + retry/backoff mirroring T39) + `GraphExtractor` trait (`extract`, `url`, `model`) + `parse_graph_json` tolerant parser + `select_graph_extractor` BYOK factory + `ExtractedNode`/`ExtractedEdge`/`GraphExtraction` serde structs + `ChatRequest`/`ChatResponse` serde structs + 9 wiremock lib tests |
| `backend/crates/worker/src/lib.rs` | T41 | Sửa | Thêm `pub mod graph;` + re-exports `DeepSeekGraphExtractor, ExtractedEdge, ExtractedNode, GraphExtractError, GraphExtraction, GraphExtractor, parse_graph_json, select_graph_extractor` |
| `backend/crates/worker/tests/select_graph_extractor.rs` | T41 | Tạo | 5 tests: `#[sqlx::test]` BYOK OpenAI; no-row → global DeepSeek; disabled → fallback; RLS isolation tenant A/B; compile-time global extractor check |
| `backend/crates/worker/Cargo.toml` | T42 | Sửa | Thêm `qdrant-client.workspace = true` (pin `=1.12.1` từ workspace T27) |
| `backend/crates/worker/src/qdrant_writer.rs` | T42 | Tạo | `DualWriteInput<'a>` + `DualWriteResult` + `IngestError` enum + `dual_write_ingestion` fn: RLS tx → upsert `document_chunks` → upsert `graph_nodes` → `QdrantStore::upsert_chunks` + `upsert_graph_nodes` → insert `graph_edges` → commit |
| `backend/crates/worker/src/lib.rs` | T42 | Sửa | Thêm `pub mod qdrant_writer;` + re-exports `DualWriteInput, DualWriteResult, IngestError, dual_write_ingestion` |
| `backend/crates/worker/tests/qdrant_writer.rs` | T42 | Tạo | 3 `#[sqlx::test]` integration tests: insert + verify counts; retry idempotency; rollback khi Qdrant fail |
| `backend/crates/worker/src/job.rs` | T43 | Sửa (rewrite) | `IngestJob` +`owner_id` +`visibility` (required, legacy payloads rejected) + `IngestContext` struct + `JobRunner` trait + `IngestContext::process_job` full pipeline + `process_job_with_retry` wrapper + `update_job_status` RLS helper + 2 unit tests |
| `backend/crates/worker/src/lib.rs` | T43 | Sửa (rewrite) | Module order + re-exports (`IngestContext, IngestJob, JobRunner, MAX_ATTEMPTS, process_job_with_retry`). `run()` xây `IngestContext::from_config` (app_pool + Qdrant + S3) + poll loop gọi `process_job_with_retry(&ctx, &ctx.pool, &job)`. Xóa `init_pool` import cũ |
| `backend/crates/worker/src/queue.rs` | T43 | Sửa | `sample_job_json()` test fixture thêm `owner_id` + `visibility` |
| `backend/crates/worker/tests/process_job_retry.rs` | T43 | Tạo | 3 `#[sqlx::test]` integration tests với `MockRunner`/`AlwaysFailRunner` |

---

## 3. PUBLIC API ĐÃ CÓ
> Để agent batch sau biết không implement lại

```rust
// T41 — gmrag-worker::graph
pub struct DeepSeekGraphExtractor { client, url, model, api_key, timeout, retries, backoff_ms }
impl DeepSeekGraphExtractor {
    pub fn new(cfg: &DeepSeekConfig) -> Self
    pub fn with_timeout_secs/retries/backoff_ms(self, ...) -> Self
    pub fn url(&self) -> &str
    pub fn model(&self) -> &str
    pub async fn extract(&self, text: &str) -> Result<GraphExtraction, GraphExtractError>
}
pub trait GraphExtractor: Send + Sync {
    async fn extract(&self, text: &str) -> Result<GraphExtraction, GraphExtractError>;
    fn url(&self) -> &str;
    fn model(&self) -> &str;
}
impl GraphExtractor for DeepSeekGraphExtractor

pub struct ExtractedNode { pub kind: String, pub label: String, pub description: Option<String> }
pub struct ExtractedEdge { pub source: String, pub target: String, pub kind: String }
pub struct GraphExtraction { pub nodes: Vec<ExtractedNode>, pub edges: Vec<ExtractedEdge> }
pub enum GraphExtractError { Http, Parse(String), ... }
pub fn parse_graph_json(text: &str) -> Result<RawExtraction, GraphExtractError>

pub async fn select_graph_extractor(
    pool: &PgPool, tenant_id: Uuid, deepseek_cfg: &DeepSeekConfig
) -> Result<Box<dyn GraphExtractor>, GraphExtractError>

// T42 — gmrag-worker::qdrant_writer
pub struct DualWriteInput<'a> {
    pub tenant_id: Uuid,
    pub workspace_id: Uuid,
    pub document_id: Uuid,
    pub owner_id: Uuid,
    pub visibility: String,
    pub filename: String,
    pub chunks: Vec<String>,
    pub chunk_vectors: Vec<Vec<f32>>,
    pub extraction: GraphExtraction,
    pub node_vectors: Vec<Vec<f32>>,
}
pub struct DualWriteResult { pub chunk_ids: Vec<Uuid>, pub node_ids: Vec<Uuid>, pub edges_written: usize }
pub enum IngestError { Db(String), Qdrant(String), Input(String) }
pub async fn dual_write_ingestion(
    pool: &PgPool,
    qdrant: &QdrantStore,
    input: DualWriteInput<'_>,
) -> Result<DualWriteResult, IngestError>

// T43 — gmrag-worker::job
pub struct IngestJob {
    pub id: Uuid, pub tenant_id: Uuid, pub workspace_id: Uuid,
    pub document_id: Uuid, pub s3_key: String, pub filename: String,
    pub owner_id: Uuid, pub visibility: String,
    pub attempts: u32,
}

pub struct IngestContext {
    pub pool: PgPool,         // AppPool (gmrag_app role, RLS enforced)
    pub qdrant: QdrantStore,
    pub s3: S3Client,
    pub ollama_cfg: OllamaConfig,
    pub deepseek_cfg: DeepSeekConfig,
}
impl IngestContext {
    pub async fn from_config(cfg: &Config) -> Result<Self, anyhow::Error>
    pub async fn process_job(&self, job: IngestJob) -> Result<(), String>  // full pipeline
}
impl JobRunner for IngestContext { async fn run(&self, job: IngestJob) -> Result<(), String> }

pub const MAX_ATTEMPTS: u32 = 3;
pub async fn process_job_with_retry<R: JobRunner>(
    runner: &R, pool: &PgPool, job: IngestJob
) -> Result<(), sqlx::Error>  // luôn trả Ok sau khi mark failed (trừ DB error)

pub trait JobRunner: Send + Sync {
    async fn run(&self, job: IngestJob) -> Result<(), String>;
}
```

---

## 4. MIGRATION STATE
| File migration | Version timestamp | Bảng tạo ra |
|---------------|------------------|-------------|
| (Tất cả migrations từ Batch 1-4 unchanged) | | |
| `20260618100000_tenant_llm_config.sql` (T40) | `20260618100000` | `tenant_llm_config` (PK=tenant_id) + RLS + GRANT |
| `20260618110000_graph_idempotency_and_llm.sql` (T41) | `20260618110000` | ALTER graph_nodes (workspace_id, UNIQUE constraint, idx) + ALTER tenant_llm_config (llm_model, llm_base_url) |

RLS đang enforce trên: 16 bảng cũ + `tenant_llm_config` = 17 bảng
sqlx offline cache: có (2 files từ T26)

---

## 5. ENV VARS / CONFIG
N/A — Batch 5C không thêm env mới (reuse existing `DeepSeekConfig`, `tenant_llm_config` migration T40).

---

## 6. DEPENDENCIES (Cargo.toml)
| Crate | Version | Thêm vào crate | Task |
|-------|---------|---------------|------|
| `qdrant-client` | `=1.12.1` (workspace) | `gmrag-worker` | T42 |

---

## 7. QUYẾT ĐỊNH KỸ THUẬT CÓ IMPACT CROSS-TASK
> Chỉ giữ quyết định ảnh hưởng tới batch sau, bỏ quyết định internal

- **[T41]** Tolerant JSON parser (`parse_graph_json`): strip markdown fence, locate first `{`…`}`, deserialize với `#[serde(default)]` → missing `edges` default `[]`
- **[T41]** `response_format: {type: "json_object"}` — ép DeepSeek/OpenAI trả JSON object
- **[T41]** System prompt hard-coded `SYSTEM_PROMPT` — dict shape `{nodes:[{kind,label,description}], edges:[{source,target,kind}]}`, cấm markdown fence + commentary
- **[T41]** `select_graph_extractor` 2-layer fallback: (1) tenant BYOK → BYOK extractor; (2) no row/disabled/ollama → global `DeepSeekConfig`
- **[T41]** `workspace_id` trên `graph_nodes` (NULL cho back-compat) + `UNIQUE(tenant_id, workspace_id, label, kind)` cho idempotency
- **[T41]** `DeepSeekGraphExtractor` dùng cho cả DeepSeek + OpenAI BYOK (OpenAI-compatible `/chat/completions`)
- **[T41]** `GraphExtractor` trait có `url()`/`model()` — cho phép integration test inspect endpoint/model
- **[T42]** Transaction cross-service Postgres ↔ Qdrant: (1) `BEGIN` Postgres + upsert metadata; (2) gọi Qdrant upsert TRƯỚC `COMMIT`; (3) nếu Qdrant Err → drop tx (Postgres `ROLLBACK` tự động); (4) nếu OK → `COMMIT`
- **[T42]** Idempotency `qdrant_point_id` stable trên retry: `ON CONFLICT (document_id, chunk_index) DO UPDATE SET content = EXCLUDED.content` KHÔNG ghi đè `qdrant_point_id` → `RETURNING qdrant_point_id` trả UUID gốc
- **[T42]** `graph_nodes` idempotent qua `UNIQUE(tenant_id, workspace_id, label, kind)` (T41 constraint)
- **[T42]** `graph_edges` idempotent qua `UNIQUE(src_node_id, dst_node_id, kind)` (schema T21 sẵn)
- **[T42]** Qdrant point id = UUID string: `qdrant_client::PointId` có `From<String>` → dùng `point_id.to_string()`
- **[T42]** `setup_tenant_collections` trước upsert — đảm bảo `chunks_{tenant_id}`/`graph_{tenant_id}` tồn tại
- **[T42]** Payload schema khớp T28/T29: chunk payload `{workspace_id, document_id, chunk_index, filename, owner_id, visibility}`; node payload `{node_id, workspace_id, entity_name}`. UUID lưu string
- **[T42]** `IngestError` enum: `Db(String)`, `Qdrant(String)`, `Input(String)` — `From<sqlx::Error>` + `From<gmrag_core::Error>`
- **[T43]** `JobRunner` trait cho testability — `IngestContext::process_job` cần S3 + Qdrant + Ollama + DeepSeek live → không unit-test được. `MockRunner` (fail N lần rồi Ok) + `AlwaysFailRunner` — verify retry/status logic độc lập
- **[T43]** Retry in-memory, KHÔNG re-enqueue Redis: `process_job_with_retry` loop `0..MAX_ATTEMPTS` với `tokio::time::sleep` giữa các fail
- **[T43]** Backoff `1s * 2^attempt` cap 16s: `BACKOFF_BASE_MS=1000`, `BACKOFF_CAP_MS=16000`. Attempt 0 fail → sleep 1s, attempt 1 fail → sleep 2s, attempt 2 fail → mark failed
- **[T43]** `process_job_with_retry` LUÔN trả `Ok(())` — chỉ `Err` khi `update_job_status` (DB) fail. Job pipeline error → recorded trong `ingest_jobs.last_error` + status, wrapper trả `Ok` → poll loop không crash
- **[T43]** `update_job_status` RLS-scoped tx: `BEGIN` + `SET LOCAL ROLE gmrag_app` + `SET LOCAL app.tenant_id = {tenant_id}` + `UPDATE ingest_jobs`
- **[T43]** `IngestJob` +`owner_id`/`visibility` (required): worker stateless, API enqueue nhồi metadata vào payload. Legacy payload thiếu 2 field → serde_json reject
- **[T43]** Pipeline dùng `parse_pdf` (text path), chưa `parse_pdf_with_ocr` — T37 `PdfiumRenderer` chưa implement (cần `pdfium-render` + libpdfium binary — blocker T37)
- **[T43]** Graph node embed dùng `description` (fallback `label`): `node_texts = nodes.iter().map(|n| if description empty { label } else { description })`
- **[T43]** `IngestContext::from_config`: khởi tạo `init_app_pool` (RLS) + `QdrantStore::new` (health probe) + `S3Client::new` (sync). Fail-fast nếu DB/Qdrant unreachable

---

## 8. INVARIANTS BẮT BUỘC (tích lũy đến hết batch này)
> Đây là luật bất biến — agent vi phạm là toang
> Phải liệt kê ĐẦY ĐỦ kể cả invariant kế thừa từ batch trước

1-113. (Tất cả invariants từ Batch 1+2A+2B+3+4+5A+5B — giữ nguyên)

**MỚI (Batch 5C):**
114. **Tolerant JSON parser cho graph extraction** — strip markdown fence, locate first `{`…`}`, `#[serde(default)]` cho missing fields
115. **`response_format: {type: "json_object"}`** — ép LLM trả JSON object (DeepSeek + OpenAI hỗ trợ)
116. **`GraphExtractor` system prompt** hard-coded — cấm markdown fence + commentary, dict shape cố định
117. **`select_graph_extractor` 2-layer fallback** — tenant BYOK → global DeepSeek
118. **`graph_nodes.workspace_id` NULL cho back-compat** — worker luôn insert non-NULL workspace_id
119. **UNIQUE constraint `graph_nodes(tenant_id, workspace_id, label, kind)`** — idempotent retry
120. **UNIQUE constraint `graph_edges(src_node_id, dst_node_id, kind)`** — idempotent retry
121. **`DeepSeekGraphExtractor` cho cả DeepSeek + OpenAI BYOK** — OpenAI-compatible `/chat/completions`
122. **Transaction cross-service Postgres ↔ Qdrant**: `BEGIN Postgres` → upsert metadata → Qdrant upsert → `COMMIT`. Qdrant fail → Postgres ROLLBACK tự động
123. **`qdrant_point_id` stable trên retry** — `ON CONFLICT DO UPDATE SET content` KHÔNG ghi đè `qdrant_point_id` → `RETURNING` trả UUID gốc
124. **`setup_tenant_collections` TRƯỚC `dual_write_ingestion`** — Qdrant op, không ảnh hưởng Postgres tx
125. **Payload UUID lưu string** trong Qdrant — payload index `FieldType::Uuid` chấp nhận keyword match
126. **`IngestJob` struct BẮT BUỘC có `owner_id` + `visibility`** — legacy payloads sẽ reject (serde_json fail)
127. **`JobRunner` trait** cho testability — pattern giống `JobQueue`/`MockQueue` (T34)
128. **Retry in-memory, KHÔNG re-enqueue Redis** — `tokio::time::sleep` giữa fail attempts; sau 3 fail → drop khỏi context
129. **`process_job_with_retry` luôn trả `Ok(())`** — chỉ `Err` khi `update_job_status` (DB) fail → poll loop không crash
130. **`update_job_status` RLS-scoped tx** — `SET LOCAL ROLE gmrag_app` + `SET LOCAL app.tenant_id` + `UPDATE ingest_jobs`
131. **Backoff `1s * 2^attempt` cap 16s** — `BACKOFF_BASE_MS=1000`, `BACKOFF_CAP_MS=16000`
132. **Pipeline `parse_pdf` (text-only) trước** — OCR wiring follow-up khi `PdfiumRenderer` sẵn sàng
133. **Graph node embed dùng `description` fallback `label`** — cùng `select_embedder` cho chunks + nodes → shared semantic space
134. **`IngestContext::from_config` fail-fast** — DB/Qdrant unreachable → return error (không silently skip)
135. **Worker hiện dùng `init_app_pool` (gmrag_app role)** — DONE (T43) — thay vì `init_pool` (admin) ở T34-T36

---

## 9. ⚠️ BLOCKERS & WARNINGS CHO BATCH TIẾP THEO
> QUAN TRỌNG NHẤT — KHÔNG được tóm tắt chung chung

### P0 — Chặn đường găng (phải xử lý trước khi bắt đầu batch sau)
- **[nguồn: T43]** OCR pipeline chưa wire: T43 dùng `parse_pdf` (text-only). Scanned/image-only PDF → empty text → empty chunks → empty graph. Cần implement `PdfiumRenderer` (`pdfium-render` + libpdfium trong Docker) rồi swap sang `parse_pdf_with_ocr(bytes, 30, &renderer, &ocr)` — follow-up task (T37 blocker).
- **[nguồn: T43]** API enqueue route chưa có: `routes/documents.rs` chưa tồn tại. API phải tạo route enqueue `IngestJob` với đầy đủ `owner_id`/`visibility` + `LPUSH gmrag:ingest_jobs`. Worker đã sẵn sàng consume.
- **[nguồn: T43]** `documents.status` chưa update: T43 update `ingest_jobs.status` nhưng chưa update `documents.status` ('processing'/'indexed'/'failed'). Có thể thêm `UPDATE documents SET status` trong `process_job` hoặc `dual_write_ingestion` — follow-up.
- **[nguồn: T43]** Long document → DeepSeek context window: `extract(&full_text)` truyền toàn bộ text. Document rất dài có thể vượt DeepSeek context limit → cần truncate hoặc chunk-level extraction + merge graph — future perf task.
- **[nguồn: T42]** `IngestJob` cần `owner_id` + `visibility`: T42 `DualWriteInput` nhận `owner_id`/`visibility`. Struct `IngestJob` hiện thiếu 2 field này → T43 phải bổ sung vào struct + serde (API enqueue sẽ populate). T43 đã fix.
- **[nguồn: T41]** `workspace_id` NULL trên graph_nodes cũ: Migration để NULL cho existing rows. Nếu có dữ liệu seed cũ với NULL workspace_id, unique constraint cho phép duplicate — chỉ ảnh hưởng dữ liệu pre-T41. Worker mới luôn set workspace_id non-NULL.
- **[nguồn: T41]** `response_format: json_object` cần model hỗ trợ: `deepseek-v4-flash` + `gpt-4o-mini` hỗ trợ. Nếu tenant BYOK model không hỗ trợ → API có thể reject hoặc ignore → parser tolerant vẫn handle prose-wrapped JSON, nhưng nếu model trả纯 prose không có `{}` → `Err(Parse)`. Job retry (T43) sẽ catch.
- **[nguồn: T42]** `setup_tenant_collections` trước upsert: Gọi idempotent setup trong tx (Qdrant op, không ảnh hưởng Postgres tx). Đảm bảo `chunks_{tenant_id}`/`graph_{tenant_id}` tồn tại trước upsert — tránh error "collection doesn't exist". T30 setup đã guard `collection_exists`.

### P1 — Lưu ý khi implement
- **[T41]** Edges có endpoint label không nằm trong extraction → skip (continue) + không count
- **[T41]** System prompt dict shape: `{nodes:[{kind,label,description}], edges:[{source,target,kind}]}`
- **[T42]** Qdrant payload schema khớp T28/T29 — agent batch sau KHÔNG thay đổi payload schema
- **[T42]** Nếu Postgres `COMMIT` fail sau Qdrant success (rare) → orphan Qdrant points; retry re-upsert cùng point id (idempotent) → acceptable cho MVP
- **[T43]** Backoff test slow: `retry_marks_failed_after_three_attempts` pay real backoff ~3s. Nếu CI chặt chẽ, có thể inject backoff duration qua trait/config — future refactor
- **[T43]** Workspace clippy: chỉ chạy `-p gmrag-worker`. Core/api không chạm T43 → nên clean, nhưng chạy `cargo clippy --workspace --all-targets -D warnings` (SQLX_OFFLINE=true) ở cuối sprint để confirm
- **[T43]** Test `process_job_with_retry` verify wrapper không propagate job error (`.expect("wrapper must not propagate job error")`)

### P2 — Ghi nhớ nhỏ
- `tenant_llm_config` columns mới (T41): `llm_model TEXT NULL`, `llm_base_url TEXT NULL`
- `graph_nodes` UNIQUE constraint: `(tenant_id, workspace_id, label, kind)` — `NULL` workspace_id được phép nhiều row
- `IngestJob.attempts` field: u32 — track retry count, không reset trên retry (worker chỉ update `ingest_jobs.attempts` trong DB)
- `process_job_with_retry` thứ tự: attempt 0 → fail → sleep 1s → attempt 1 → fail → sleep 2s → attempt 2 → fail → mark failed (no sleep)
- `MockRunner` test: `attempts: 0..3` thì success cuối cùng; verify `status='completed'`, `attempts=N`
- `AlwaysFailRunner` test: 3 fails liên tiếp; verify `status='failed'`, `attempts=3`, `last_error="permanent failure"`
- `IngestContext` clone fields: `ctx.pool` clone (PgPool cheap clone via Arc)
- `qdrant_writer` integration test cần Qdrant localhost:6334 — chạy trong stack Docker dev

---

## 10. UNBLOCKS
- Batch 5C → unblock: API ingestion endpoint (enqueue job → worker full pipeline)
- Batch 5C → unblock: Sprint 5 hoàn thành (T34-T43 worker ingestion pipeline ready)
- Batch 5C → unblock: Sprint 6 RAG/LLM (retrieval dùng chunks + graph nodes đã có vector)
- Batch 5C → unblock: E2E ingestion test (upload PDF → worker → Qdrant + Postgres verify)
- Batch 5C → unblock: T44+ (API LLM provider, BYOK, retrieval, RAG)
