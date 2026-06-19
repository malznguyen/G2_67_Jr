# BATCH 6B SUMMARY — RAG Retrieval: Chunks, Graph, System Prompt
# Tasks: T46, T47, T48 | Trạng thái: ✅ Hoàn thành
# Test count khi kết thúc batch: 87+ passed, 0 failed (workspace tests, inherited from BATCH 4)

---

## 1. TRẠNG THÁI TỔNG
| Task | Tên ngắn | Commit |
|------|----------|--------|
| T46 | API chunk retrieval: embed query → Qdrant search top-5 (ACL filter) | (commit T46) |
| T47 | Graph retrieval: Qdrant nodes top-5 + ILIKE fallback + edges | (commit T47) |
| T48 | `assemble_system_prompt` (port + `[chunk:N]` citations) | (commit T48) |

---

## 2. FILES ĐÃ TẠO / SỬA
| File | Task | Hành động | Ghi chú |
|------|------|-----------|---------|
| `backend/crates/api/src/chat/retrieval.rs` | T46 | Tạo | T46 chunk retrieval + ACL |
| `backend/crates/api/src/chat/mod.rs` | T46 | Tạo | Re-export retrieval types |
| `backend/crates/api/src/lib.rs` | T46 | Tạo | Library root + Qdrant init |
| `backend/crates/api/src/main.rs` | T46 | Sửa | Thin binary wrapper |
| `backend/crates/api/Cargo.toml` | T46 | Sửa | `[lib]`, `qdrant-client` |
| `backend/crates/api/src/chat/retrieval.rs` | T47 | Sửa | T47 graph + orchestrator |
| `backend/crates/api/src/chat/mod.rs` | T47 | Sửa | Re-export `retrieve_all_with_provider`, `GraphContext` |
| `backend/crates/api/src/chat/mod.rs` | T48 | Sửa | `assemble_system_prompt` + 4 unit tests |

---

## 3. PUBLIC API ĐÃ CÓ
> Để agent batch sau biết không implement lại

```rust
// T46 — gmrag-api::chat::retrieval
pub struct ChunkHit {
    pub point_id: Uuid,           // qdrant_point_id
    pub document_id: Uuid,
    pub chunk_index: i32,
    pub score: f32,
    pub content: String,
    pub filename: Option<String>,
    pub citation_index: usize,    // 1-based
}

pub struct RetrievalFilters { /* workspace_id, document_id, visibility, owner_id, ... */ }

pub async fn accessible_document_ids(
    conn: &mut PgConnection, tenant_id: Uuid, user_id: Uuid
) -> Result<HashSet<Uuid>, sqlx::Error>
// workspace member check + documents readable qua visibility/owner_id/resource_acl

pub async fn retrieve_chunks(
    conn: &mut PgConnection,
    qdrant: &QdrantStore,
    tenant_id: Uuid,
    user_id: Uuid,
    query: &str,
    embedder: &dyn Embedder,
    top_k: usize,
) -> Result<Vec<ChunkHit>, RetrievalError>

pub async fn retrieve_chunks_with_vector(
    conn: &mut PgConnection,
    qdrant: &QdrantStore,
    tenant_id: Uuid,
    user_id: Uuid,
    query_vector: Vec<f32>,
    top_k: usize,
) -> Result<Vec<ChunkHit>, RetrievalError>

// T47 — graph retrieval
pub struct GraphNode { pub node_id: Uuid, pub workspace_id: Option<Uuid>, pub entity_name: String, pub score: f32 }
pub struct GraphEdge { pub source_label: String, pub target_label: String, pub kind: String }
pub struct GraphContext { pub nodes: Vec<GraphNode>, pub edges: Vec<GraphEdge> }

pub async fn retrieve_graph_context(
    conn: &mut PgConnection,
    qdrant: &QdrantStore,
    tenant_id: Uuid,
    user_id: Uuid,
    query_vector: Vec<f32>,
    top_k: usize,
) -> Result<GraphContext, RetrievalError>
// search_graph_nodes top-5, hydrate graph_nodes từ PG
// ILIKE fallback khi vector hits rỗng hoặc score < GRAPH_SCORE_THRESHOLD (0.25) trên label/properties.description
// load_graph_edges: edges có endpoint trong hit set, join labels

pub async fn retrieve_all(
    conn: &mut PgConnection,
    qdrant: &QdrantStore,
    tenant_id: Uuid,
    user_id: Uuid,
    query: &str,
    embedder: &dyn Embedder,
    top_k: usize,
) -> Result<(Vec<ChunkHit>, GraphContext), RetrievalError>

pub async fn retrieve_all_with_provider(
    conn: &mut PgConnection,
    qdrant: &QdrantStore,
    tenant_id: Uuid,
    user_id: Uuid,
    query: &str,
    provider: &dyn LlmProvider,
    top_k: usize,
) -> Result<(Vec<ChunkHit>, GraphContext), RetrievalError>

// T48 — pure function
pub fn assemble_system_prompt(chunks: &[ChunkHit], graph: &GraphContext) -> String
// Template: GMRAG instructions + `## Document excerpts` với `[chunk:N] (source: …)` + optional `## Knowledge graph`
// N = ChunkHit.citation_index (1-based)
```

---

## 4. MIGRATION STATE
N/A — Batch 6B không thêm migration.

---

## 5. ENV VARS / CONFIG
N/A — Batch 6B không thêm env mới (reuse `QDRANT_URL` (cần fix sang 6334 theo T27 note), `OLLAMA_*`, etc.).

---

## 6. DEPENDENCIES (Cargo.toml)
| Crate | Version | Thêm vào crate | Task |
|-------|---------|---------------|------|
| `qdrant-client` | `=1.12.1` (workspace) | `gmrag-api` | T46 |

---

## 7. QUYẾT ĐỊNH KỸ THUẬT CÓ IMPACT CROSS-TASK
> Chỉ giữ quyết định ảnh hưởng tới batch sau, bỏ quyết định internal

- **[T46]** **ACL ở PG trước Qdrant**: `resource_acl` không có trong Qdrant payload → pre-filter `document_id` list, build `Filter::should` + `min_should=1`
- **[T46]** Embed qua `LlmProvider` — Handler T49 gọi `resolve_llm_config` rồi `DeepSeekProvider::new` (T45 pattern)
- **[T46]** `retrieve_chunks_with_vector` — cho phép T47/T49 embed một lần, reuse vector
- **[T46]** Wire `QdrantStore` vào API bootstrap (`lib.rs` Extension) + refactor `main.rs` → `gmrag_api::run()`
- **[T46]** Qdrant filter: `must workspace_id` + `should document_id` (min_should=1), `top_k=5` default
- **[T46]** Hydrate `document_chunks.content` + `documents.title` từ `qdrant_point_id`; post-filter ACL; gán `citation_index` 1..N
- **[T47]** Reuse query vector từ T46 — không gọi `embed_query` lần hai trong `retrieve_all_with_provider`
- **[T47]** ILIKE `%query%` đơn giản, LIMIT `top_k`; dedupe theo `node_id` với vector hits
- **[T47]** `GRAPH_SCORE_THRESHOLD = 0.25` — dưới threshold → fallback ILIKE
- **[T47]** Edges MVP: Load edges touch bất kỳ retrieved node; cả hai endpoint labels trong output
- **[T47]** Sequential retrieve chunks → graph (cùng `PgConnection`); parallel `tokio::join!` cần hai RLS connections — defer đến khi có pool borrow helper
- **[T48]** Pure function, no I/O — T49 gọi sau `retrieve_all_with_provider`
- **[T48]** Source label: Ưu tiên `filename` (document title từ PG), fallback `document_id`
- **[T48]** Graph section omitted khi `graph.nodes` rỗng
- **[T48]** Template: GMRAG instructions + `## Document excerpts` với `[chunk:N] (source: …)` + optional `## Knowledge graph` (entities + relationships)
- **[T48]** `N` = `ChunkHit.citation_index` (1-based) — căn T50 `resolve_chunk_index_citations`

---

## 8. INVARIANTS BẮT BUỘC (tích lũy đến hết batch này)
> Đây là luật bất biến — agent vi phạm là toang
> Phải liệt kê ĐẦY ĐỦ kể cả invariant kế thừa từ batch trước

1-146. (Tất cả invariants từ Batch 1+2A+2B+3+4+5A+5B+5C+6A — giữ nguyên)

**MỚI (Batch 6B):**
147. **ACL pre-filter ở PG trước Qdrant** — `resource_acl` không có trong Qdrant payload → pre-filter `document_id` list
148. **Qdrant filter shape** cho chunks: `must workspace_id` + `should document_id` (min_should=1)
149. **Qdrant `top_k=5` default** cho chunk retrieval
150. **Embed một lần, reuse vector** — `retrieve_chunks_with_vector` cho T47/T49
151. **`QdrantStore` wire vào API bootstrap** qua `lib.rs` Extension
152. **Qdrant graph top_k=5 + ILIKE fallback** dưới `GRAPH_SCORE_THRESHOLD = 0.25`
153. **ILIKE `%query%` trên `label` / `properties.description`** — dedupe theo `node_id` với vector hits
154. **Sequential retrieve chunks → graph** (cùng `PgConnection`); parallel defer đến khi có pool borrow helper
155. **`assemble_system_prompt` pure function** — no I/O; T49 gọi sau retrieval
156. **Source label ưu tiên `filename` (PG title), fallback `document_id`**
157. **Graph section omitted khi `graph.nodes` rỗng**
158. **`N` = `ChunkHit.citation_index` (1-based)** — `[chunk:N]` trong system prompt
159. **`accessible_document_ids` check** workspace member + documents readable qua visibility/owner_id/resource_acl

---

## 9. ⚠️ BLOCKERS & WARNINGS CHO BATCH TIẾP THEO
> QUAN TRỌNG NHẤT — KHÔNG được tóm tắt chung chung

### P0 — Chặn đường găng (phải xử lý trước khi bắt đầu batch sau)
- **[nguồn: T46]** Chưa có HTTP route; T49 mount chat SSE sau khi có `assemble_system_prompt` (T48)
- **[nguồn: T46]** T47 mở rộng cùng file `retrieval.rs`
- **[nguồn: T46]** `#[sqlx::test]` retrieval tests cần Postgres + Qdrant (`localhost:6334`) — chạy trong stack Docker dev
- **[nguồn: T47]** Graph integration tests (`#[sqlx::test]`) cần Postgres + Qdrant dev stack
- **[nguồn: T48]** T49: `ChatMessage::new("system", assemble_system_prompt(...))` + user query → `LlmProvider::chat_stream`
- **[nguồn: T48]** T50: parse `[chunk:N]` trong SSE stream, map `N` → `ChunkHit.point_id`

### P1 — Lưu ý khi implement
- **[T46]** `retrieve_chunks_with_vector` — cho phép T47/T49 embed một lần
- **[T47]** Parallel `tokio::join!` chunks+graph cần hai RLS connections — defer đến khi có pool borrow helper
- **[T48]** `N` = `ChunkHit.citation_index` 1-based — căn T50 resolver

### P2 — Ghi nhớ nhỏ
- `ChunkHit` field: `point_id, document_id, chunk_index, score, content, filename, citation_index`
- `GraphContext` field: `nodes, edges`
- Qdrant `top_k=5` default — caller override nếu cần
- `GRAPH_SCORE_THRESHOLD = 0.25` — dưới threshold → fallback ILIKE
- ILIKE dedupe theo `node_id` với vector hits
- Edges load khi endpoint trong hit set
- Test `retrieve_all_embeds_once` verify embed 1 lần (T47)

---

## 10. UNBLOCKS
- Batch 6B → unblock: T49 (streaming.rs DeepSeek SSE) — `assemble_system_prompt` ready
- Batch 6B → unblock: T50 (resolve_chunk_index_citations) — `ChunkHit.citation_index` 1-based
- Batch 6B → unblock: T51 (metering) — `retrieve_all_with_metering` wraps
- Batch 6B → unblock: API RAG endpoint (handler mount ở T61)
- Batch 6B → unblock: Frontend chat UI (display chunks + citations)
