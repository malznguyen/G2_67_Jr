# BATCH 6C SUMMARY — Streaming Chat, Citation Resolution, Usage Metering
# Tasks: T49, T50, T51 | Trạng thái: ✅ Hoàn thành
# Test count khi kết thúc batch: 87+ passed, 0 failed (workspace tests, inherited from BATCH 4)

---

## 1. TRẠNG THÁI TỔNG
| Task | Tên ngắn | Commit |
|------|----------|--------|
| T49 | `streaming.rs` DeepSeek SSE stream + DeepseekTokenParser port | (commit T49) |
| T50 | `resolve_chunk_index_citations` (index → point_id) | (commit T50) |
| T51 | Metering: ghi `usage_events` (`llm_tokens`, `embedding_tokens`) | (commit T51) |

---

## 2. FILES ĐÃ TẠO / SỬA
| File | Task | Hành động | Ghi chú |
|------|------|-----------|---------|
| `backend/crates/api/src/chat/streaming.rs` | T49 | Tạo | T49 parser + orchestrator + tests |
| `backend/crates/api/src/chat/mod.rs` | T49 | Sửa | `pub mod streaming`, re-export types |
| `backend/crates/api/src/chat/mod.rs` | T50 | Sửa | T50 resolver + enrich + 5 tests |
| `backend/crates/api/src/metering.rs` | T51 | Tạo | T51 core + unit tests |
| `backend/crates/api/src/lib.rs` | T51 | Sửa | `pub mod metering` |
| `backend/crates/api/src/chat/retrieval.rs` | T51 | Sửa | `retrieve_all_with_metering`, `RetrievalError::Metering` |
| `backend/crates/api/src/chat/streaming.rs` | T51 | Sửa | `meter_rag_chat_completion`, `assistant_text_from_events` |
| `backend/crates/api/tests/metering.rs` | T51 | Tạo | RLS integration tests |
| `backend/crates/api/Cargo.toml` | T51 | Sửa | `tiktoken-rs` |

---

## 3. PUBLIC API ĐÃ CÓ
> Để agent batch sau biết không implement lại

```rust
// T49 — gmrag-api::chat::streaming
pub enum ChatStreamEvent {
    Text(String),
    Citation { index: usize },
    Done { finish_reason: String },
}

pub struct DeepseekTokenParser { /* buffer + holdback cho [chunk:N] bị cắt delta */ }
impl DeepseekTokenParser {
    pub fn new() -> Self
    pub fn feed(&mut self, delta: &str) -> Vec<ChatStreamEvent>
}

pub async fn stream_rag_response(
    conn: &mut PgConnection,
    provider: &dyn LlmProvider,
    tenant_id: Uuid,
    user_id: Uuid,
    query: &str,
    chunks: Vec<ChunkHit>,
    graph: GraphContext,
) -> Result<impl Stream<Item = Result<ChatStreamEvent, StreamingError>>, StreamingError>
// assemble_system_prompt + user query → LlmProvider::chat_stream → parsed events

pub fn collect_stream_events<S: Stream<Item = Result<ChatStreamEvent, E>>>(stream: S) -> impl Future<Output = Result<Vec<ChatStreamEvent>, E>>

pub fn assistant_text_from_events(events: &[ChatStreamEvent]) -> String

pub async fn meter_rag_chat_completion(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    user_id: Uuid,
    system_prompt: &str,
    user_query: &str,
    assistant_text: &str,
) -> Result<(), sqlx::Error>
// Ghi usage sau khi consumer thu events

// T50 — gmrag-api::chat (citation resolution)
pub struct ResolvedCitation {
    pub index: usize,
    pub point_id: Uuid,
    pub document_id: Uuid,
    pub chunk_index: i32,
    pub filename: Option<String>,
}

pub fn resolve_citation(chunks: &[ChunkHit], index: usize) -> Option<ResolvedCitation>

pub fn resolve_chunk_index_citations(chunks: &[ChunkHit], indices: impl IntoIterator<Item = usize>) -> Vec<ResolvedCitation>
// Dedupe, skip unknown index

pub enum EnrichedChatStreamEvent {
    Text(String),
    CitationResolved(ResolvedCitation),
    CitationUnknown(usize),
    Done { finish_reason: String },
}

pub fn enrich_stream_events(
    chunks: &[ChunkHit],
    events: impl IntoIterator<Item = ChatStreamEvent>,
) -> Vec<EnrichedChatStreamEvent>

// T51 — gmrag-api::metering
pub enum UsageMetric { EmbeddingTokens, LlmTokens }
pub enum UsageOperation { QueryEmbedding, ChatCompletion, GraphExtraction }

pub async fn record_usage_event(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    user_id: Option<Uuid>,
    metric: UsageMetric,
    delta: i64,
    operation: UsageOperation,
    metadata: serde_json::Value,
) -> Result<(), sqlx::Error>

pub async fn record_embedding_usage(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    user_id: Option<Uuid>,
    input_tokens: usize,
    operation: UsageOperation,
    model: &str,
) -> Result<(), sqlx::Error>

pub async fn record_llm_usage(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    user_id: Option<Uuid>,
    input_tokens: usize,
    output_tokens: usize,
    operation: UsageOperation,
    model: &str,
) -> Result<(), sqlx::Error>

pub async fn retrieve_all_with_metering(
    conn: &mut PgConnection,
    qdrant: &QdrantStore,
    tenant_id: Uuid,
    user_id: Uuid,
    query: &str,
    provider: &dyn LlmProvider,
    top_k: usize,
) -> Result<(Vec<ChunkHit>, GraphContext), RetrievalError>
// Embed once, ghi embed usage, rồi chunk/graph
```

---

## 4. MIGRATION STATE
N/A — Batch 6C không thêm migration.

---

## 5. ENV VARS / CONFIG
N/A — Batch 6C không thêm env mới.

---

## 6. DEPENDENCIES (Cargo.toml)
| Crate | Version | Thêm vào crate | Task |
|-------|---------|---------------|------|
| `tiktoken-rs` | 0.11 (giống worker T38) | `gmrag-api` | T51 |

---

## 7. QUYẾT ĐỊNH KỸ THUẬT CÓ IMPACT CROSS-TASK
> Chỉ giữ quyết định ảnh hưởng tới batch sau, bỏ quyết định internal

- **[T49]** Không Axum SSE ở T49 — chỉ stream Rust nội bộ; T61 map sang HTTP SSE
- **[T49]** Holdback suffix: Giữ partial `[chunk:` ở cuối buffer để tránh emit text sai khi delta cắt tag (R8)
- **[T49]** `ChatStreamEvent`: `Text`, `Citation { index }`, `Done { finish_reason }`
- **[T49]** Metering sau stream — `meter_rag_chat_completion` ghi usage sau khi consumer thu events (tránh borrow `PgConnection` trong async stream)
- **[T49]** Parser biên: split tag, complete tag, finish holdback
- **[T49]** Mock provider + wiremock SSE end-to-end tests
- **[T50]** 1-based index khớp `ChunkHit.citation_index` và prompt T48 `[chunk:N]`
- **[T50]** Unknown index: parser T49 vẫn emit `Citation { index }`; enrich/map bỏ qua hoặc `CitationUnknown` cho client
- **[T50]** Dedupe: Iterator trùng index chỉ resolve lần đầu (phù hợp citation list trong UI)
- **[T51]** Append-only `usage_events` — INSERT only, metadata JSONB `{ operation, model, input_tokens?, output_tokens? }`
- **[T51]** Không dùng provider usage field — DeepSeek stream chưa parse usage; tiktoken estimate đủ cho MVP quota (T69)
- **[T51]** RLS: Ghi qua `PgConnection` đã `SET LOCAL app.tenant_id` (middleware pattern T25)
- **[T51]** Token count MVP: `tiktoken-rs` cl100k (cùng family worker chunking)
- **[T51]** Metrics: `embedding_tokens` (query embed), `llm_tokens` (input system+user + output assistant text)

---

## 8. INVARIANTS BẮT BUỘC (tích lũy đến hết batch này)
> Đây là luật bất biến — agent vi phạm là toang
> Phải liệt kê ĐẦY ĐỦ kể cả invariant kế thừa từ batch trước

1-159. (Tất cả invariants từ Batch 1+2A+2B+3+4+5A+5B+5C+6A+6B — giữ nguyên)

**MỚI (Batch 6C):**
160. **`ChatStreamEvent`**: `Text`, `Citation { index }`, `Done { finish_reason }`
161. **Holdback suffix** trong `DeepseekTokenParser` — giữ partial `[chunk:` ở cuối buffer
162. **Metering sau stream** — `meter_rag_chat_completion` ghi usage sau khi consumer thu events
163. **`ResolvedCitation`**: `index, point_id, document_id, chunk_index, filename` (T50)
164. **1-based citation index** khớp `ChunkHit.citation_index` + `[chunk:N]`
165. **Dedupe citation resolution** — trùng index chỉ resolve lần đầu
166. **`EnrichedChatStreamEvent`**: `Text`, `CitationResolved`, `CitationUnknown`, `Done`
167. **Append-only `usage_events`** — INSERT only
168. **Metadata JSONB** `{ operation, model, input_tokens?, output_tokens? }` cho `usage_events`
169. **Tiktoken estimate cho token count** (không dùng provider usage field — DeepSeek stream chưa parse)
170. **`retrieve_all_with_metering`** ghi `embedding_tokens` trước khi chunk/graph
171. **`meter_rag_chat_completion`** ghi `llm_tokens` sau stream
172. **Usage ghi qua `PgConnection` đã `SET LOCAL app.tenant_id`** (RLS pattern)

---

## 9. ⚠️ BLOCKERS & WARNINGS CHO BATCH TIẾP THEO
> QUAN TRỌNG NHẤT — KHÔNG được tóm tắt chung chung

### P0 — Chặn đường găng (phải xử lý trước khi bắt đầu batch sau)
- **[nguồn: T49]** T61: map `ChatStreamEvent` / `EnrichedChatStreamEvent` → JSON SSE client (`token`, `citation`, `done`)
- **[nguồn: T49]** T61 gọi `meter_rag_chat_completion` sau khi stream xong (cùng RLS transaction)
- **[nguồn: T50]** T61 SSE payload citation nên dùng `ResolvedCitation` (point_id UUID) từ `enrich_stream_events`
- **[nguồn: T51]** T61 handler gọi `retrieve_all_with_metering` + `meter_rag_chat_completion` trong cùng RLS tx
- **[nguồn: T51]** T69 GET usage aggregate từ `usage_events` — ngoài scope Sprint 6C
- **[nguồn: T51]** `cargo test -p gmrag-api --test metering` cần Postgres dev stack

### P1 — Lưu ý khi implement
- **[T49]** Holdback suffix — bắt buộc để tránh emit text sai khi delta cắt tag
- **[T49]** Metering sau stream — `meter_rag_chat_completion` ghi usage sau khi consumer thu events
- **[T50]** Unknown index: parser T49 vẫn emit `Citation { index }`; enrich/map bỏ qua hoặc `CitationUnknown` cho client
- **[T51]** Token count MVP: `tiktoken-rs` cl100k (cùng family worker chunking T38)
- **[T51]** RLS: Ghi qua `PgConnection` đã `SET LOCAL app.tenant_id` (middleware pattern T25)

### P2 — Ghi nhớ nhỏ
- `ChatStreamEvent::Citation { index }` — parser emit khi thấy `[chunk:N]`
- `DeepseekTokenParser` — buffer + holdback pattern
- `ResolvedCitation` field order: `index, point_id, document_id, chunk_index, filename`
- `EnrichedChatStreamEvent::CitationUnknown(usize)` — khi index không match ChunkHit
- `usage_events` columns (T24 migration): `id, tenant_id, user_id, metric, delta, metadata, occurred_at`
- `UsageMetric` enum: `EmbeddingTokens`, `LlmTokens`
- `UsageOperation` enum: `QueryEmbedding`, `ChatCompletion`, `GraphExtraction`
- Test `metering.rs` RLS isolation: tenant B không thấy usage_events của tenant A
- Tiktoken cl100k estimate — không 100% accurate vs provider usage (acceptable cho MVP)

---

## 10. UNBLOCKS
- Batch 6C → unblock: T61 (SSE HTTP route mount) — map `EnrichedChatStreamEvent` → JSON SSE
- Batch 6C → unblock: T69 (GET usage aggregate) — `usage_events` data ready
- Batch 6C → unblock: Frontend chat UI (display streaming tokens + citations + usage)
- Batch 6C → unblock: Sprint 6 hoàn thành (T44-T51 RAG + LLM stack)
- Batch 6C → unblock: E2E RAG test (query → embed → retrieve → stream → meter)
