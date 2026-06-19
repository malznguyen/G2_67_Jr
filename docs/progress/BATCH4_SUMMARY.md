# BATCH 4 SUMMARY — Qdrant Vector Store + RLS Middleware Wiring (HOTFIX)
# Tasks: T27, T28, T29, T30, T31, T32, T33, HOTFIX_PRE_BATCH5 | Trạng thái: ✅ Hoàn thành
# Test count khi kết thúc batch: 87 passed, 0 failed (workspace tests, sau HOTFIX)

---

## 1. TRẠNG THÁI TỔNG
| Task | Tên ngắn | Commit |
|------|----------|--------|
| T27 | core: qdrant-client dep + QdrantStore wrapper (create/delete collection, upsert/search) | `64a64bb` |
| T28 | Collection chunks_{tenant_id} dim 768 HNSW cosine + 6 payload indexes | `322dab2` |
| T29 | Collection graph_{tenant_id} dim 768 + 3 payload indexes | `deb1d83` |
| T30 | setup_tenant_collections (idempotent) + teardown_tenant_collections | `4505345` |
| T31 | QdrantStore::upsert_chunks(tenant_id, points) | `805b767` |
| T32 | QdrantStore::search_chunks(tenant_id, vector, filter, top_k) | `be839ea` |
| T33 | QdrantStore::upsert_graph_nodes + search_graph_nodes | `7fe8a2a` |
| HOTFIX | Pre-Batch 5: cleanup 2 clippy errors + verify RLS middleware wiring | `cc443ea` |

---

## 2. FILES ĐÃ TẠO / SỬA
| File | Task | Hành động | Ghi chú |
|------|------|-----------|---------|
| `backend/Cargo.toml` | T27 | Sửa | Thêm `qdrant-client = "=1.12.1"` vào `[workspace.dependencies]` |
| `backend/crates/core/Cargo.toml` | T27 | Sửa | Thêm `tokio.workspace = true` + `qdrant-client.workspace = true` |
| `backend/crates/core/src/qdrant/mod.rs` | T27 | Tạo | Module entry: `mod store; pub use store::QdrantStore;` |
| `backend/crates/core/src/qdrant/store.rs` | T27 | Tạo | `QdrantStore` struct (client trong `Arc`), `new(cfg)` async + health probe, `health_check()` |
| `backend/crates/core/src/lib.rs` | T27 | Sửa | `pub mod qdrant;` + `pub use qdrant::QdrantStore;` |
| `backend/crates/core/src/error.rs` | T27 | Sửa | `Error::Qdrant(Box<qdrant_client::QdrantError>)` + manual `From<QdrantError>` + `code()` case `"qdrant-error"` |
| `backend/Cargo.lock` | T27 | Sửa | Sinh bởi `cargo build` |
| `backend/crates/core/src/qdrant/store.rs` | T28 | Sửa | Thêm `create_chunks_collection(tenant_id)`, `delete_collection(name)`, `list_collection_names()`, private helper `create_collection_with_indexes(name, indexes)`, const `EMBED_DIM = 768` |
| `backend/crates/core/src/qdrant/store.rs` | T29 | Sửa | Thêm `create_graph_collection(tenant_id)` — reuse `create_collection_with_indexes` với 3 payload indexes |
| `backend/crates/core/src/qdrant/store.rs` | T30 | Sửa | Đổi `create_chunks_collection` + `create_graph_collection` từ `pub` → private + thêm guard idempotency `collection_exists`. Thêm private `collection_exists(name)`. Thêm `pub async fn setup_tenant_collections(tenant_id)` + `pub async fn teardown_tenant_collections(tenant_id)` |
| `backend/crates/core/src/qdrant/store.rs` | T31 | Sửa | Thêm import `PointStruct`, `UpsertPointsBuilder`. Thêm `pub async fn upsert_chunks(tenant_id, points)` |
| `backend/crates/core/src/qdrant/store.rs` | T32 | Sửa | Thêm import `Filter`, `ScoredPoint`, `SearchPointsBuilder`. Thêm `pub async fn search_chunks(...)` |
| `backend/crates/core/src/qdrant/store.rs` | T33 | Sửa | Thêm import `Condition` (non-test). Thêm `pub async fn upsert_graph_nodes(...)` + `pub async fn search_graph_nodes(...)` |
| `backend/crates/api/tests/pool_role.rs` | HOTFIX | Sửa | Xóa 1 dòng `use sqlx::Executor;` (FIX A unused import) |
| `backend/crates/api/src/auth/jwt.rs` | HOTFIX | Sửa | `decoding_key` → `_decoding_key` (FIX A unused variable) |

---

## 3. PUBLIC API ĐÃ CÓ
> Để agent batch sau biết không implement lại

```rust
// gmrag-core::qdrant
pub struct QdrantStore { client: Arc<qdrant_client::Qdrant> }
impl QdrantStore {
    pub async fn new(cfg: &QdrantConfig) -> Result<Self, Error>  // health probe
    pub async fn health_check(&self) -> Result<(), Error>

    // T28/T29/T30 — collection lifecycle
    async fn create_chunks_collection(&self, tenant_id: Uuid) -> Result<(), Error>   // private
    async fn create_graph_collection(&self, tenant_id: Uuid) -> Result<(), Error>    // private
    pub async fn setup_tenant_collections(&self, tenant_id: Uuid) -> Result<(), Error>   // idempotent
    pub async fn teardown_tenant_collections(&self, tenant_id: Uuid) -> Result<(), Error> // idempotent
    pub async fn delete_collection(&self, name: &str) -> Result<(), Error>          // wraps Qdrant delete
    pub async fn list_collection_names(&self) -> Result<Vec<String>, Error>
    async fn collection_exists(&self, name: &str) -> Result<bool, Error>            // private

    // T31 — chunks
    pub async fn upsert_chunks(&self, tenant_id: Uuid, points: Vec<PointStruct>) -> Result<(), Error>

    // T32 — chunks search
    pub async fn search_chunks(
        &self,
        tenant_id: Uuid,
        query_vector: Vec<f32>,
        filter: Option<Filter>,
        top_k: u64,
    ) -> Result<Vec<ScoredPoint>, Error>

    // T33 — graph
    pub async fn upsert_graph_nodes(&self, tenant_id: Uuid, points: Vec<PointStruct>) -> Result<(), Error>
    pub async fn search_graph_nodes(
        &self,
        tenant_id: Uuid,
        workspace_id: Uuid,
        query_vector: Vec<f32>,
        top_k: u64,
    ) -> Result<Vec<ScoredPoint>, Error>
}

// Constants
pub const EMBED_DIM: usize = 768;
```

---

## 4. MIGRATION STATE
N/A — Batch 4 không thêm migration (Qdrant là vector DB, không phải PostgreSQL).

Tất cả migrations từ Batch 1-3 vẫn còn:
- `20260101000000_init.sql`
- `20260617124018_identity_and_tenant.sql` (blessed)
- `20260617132425_rls_tenants_table.sql` (blessed)
- `20260617143508_workspaces.sql`
- `20260617143700_documents.sql`
- `20260617143822_graph_entities.sql`
- `20260617144046_chat.sql`
- `20260617144756_acl.sql`
- `20260617145246_system_tracking.sql`
- `20260617145935_rls_apply_all.sql`

RLS đang enforce trên: 16 bảng (unchanged)
sqlx offline cache: có (2 files từ T26)

---

## 5. ENV VARS / CONFIG
| Tên biến | Giá trị mẫu | Task thêm |
|----------|-------------|----------|
| `QDRANT_URL` | `http://qdrant:6334` (gRPC, KHÔNG REST 6333) | T27 (cần update) |

> ⚠️ T9 đã set `DEFAULT_QDRANT_URL = "http://localhost:6333"` (REST) — SAI cho rust client 1.12.1. T27 test bypass bằng cách construct config trực tiếp với 6334. Cần update config + .env.example ở task wiring (future).

---

## 6. DEPENDENCIES (Cargo.toml)
| Crate | Version | Thêm vào crate | Task |
|-------|---------|---------------|------|
| `qdrant-client` | `=1.12.1` (workspace, pin exact) | `gmrag-core` | T27 |
| `tokio` | workspace (cho health probe async) | `gmrag-core` | T27 |

---

## 7. QUYẾT ĐỊNH KỸ THUẬT CÓ IMPACT CROSS-TASK
> Chỉ giữ quyết định ảnh hưởng tới batch sau, bỏ quyết định internal

- **[T27]** `qdrant-client = "=1.12.1"` (pin exact) — version `1.12.4` KHÔNG tồn tại trên crates.io; pin closest same-minor
- **[T27]** **gRPC port 6334, KHÔNG REST port 6333** — qdrant-client rust 1.12.1 dùng gRPC exclusively. Cả 2 port đều mapped ra host (docker ps: `6333-6334->6333-6334/tcp`)
- **[T27]** `qdrant_client::Qdrant` KHÔNG impl `Clone` (verify docs.rs) — wrap `Arc<qdrant>` để giữ `QdrantStore: Clone`
- **[T27]** `Error::Qdrant(Box<QdrantError>)` + manual `From<QdrantError>` — `QdrantError` ~176 bytes trigger `clippy::result_large_err`; Box variant giữ `Error` nhỏ
- **[T27]** `new` async + `health_check` probe (fail-fast pattern như `init_pool`)
- **[T28]** Collection naming `chunks_{tenant_id}` — tenant-isolated, UUID format trực tiếp vào tên
- **[T28]** Vector config: `VectorParamsBuilder::new(768, Distance::Cosine)` — single (unnamed) vector, HNSW default (Qdrant mặc định đã là HNSW cho dense vectors)
- **[T28]** Payload indexes: `workspace_id` Uuid, `document_id` Uuid, `chunk_index` Integer, `filename` Keyword, `owner_id` Uuid, `visibility` Keyword
- **[T28]** `.wait(true)` trên `CreateFieldIndexCollectionBuilder` — index materialize trước khi return
- **[T28]** `delete_collection` semantics: wraps Qdrant delete; nuốt `Err` nếu collection không tồn tại
- **[T29]** Collection naming `graph_{tenant_id}` — song song với `chunks_{tenant_id}`; 1 tenant có 2 collection độc lập
- **[T29]** Payload indexes: `node_id` Uuid, `workspace_id` Uuid, `entity_name` Keyword
- **[T30]** `create_chunks_collection`/`create_graph_collection` đổi từ `pub` → private + idempotency guard (`collection_exists`) — theo user decision
- **[T30]** `setup_tenant_collections`/`teardown_tenant_collections` là pub API cho provisioning
- **[T30]** `teardown` dùng `list_collection_names` + `contains` (idempotent semantics)
- **[T31]** `upsert_chunks` signature: `(tenant_id, points: Vec<PointStruct>) -> Result<()>` — caller chịu trách nhiệm build payload
- **[T31]** `.wait(true)` trên upsert — search ngay sau sẽ thấy point
- **[T31]** Payload UUID lưu dạng string (`ws.to_string()`) — Qdrant Uuid-index chấp nhận keyword match (T32 verify)
- **[T32]** `search_chunks` signature: `(tenant_id, query_vector: Vec<f32>, filter: Option<Filter>, top_k: u64) -> Result<Vec<ScoredPoint>>` — caller build filter
- **[T32]** `SearchPointsBuilder::new(name, vec, top_k).with_payload(true)` — include payload, exclude vector (default)
- **[T32]** Filter UUID = keyword match: `Filter::must([Condition::matches("workspace_id", ws.to_string())])`
- **[T32]** `MatchValue` KHÔNG có variant `Uuid` trong client 1.12.1 — match dạng `Keyword` (string equality)
- **[T32]** Multi-tenant isolation 2 lớp: `tenant_id` = collection boundary (hard), `workspace_id` = payload filter (soft partition)
- **[T33]** `upsert_graph_nodes` mirror `upsert_chunks` — cùng signature, cùng `UpsertPointsBuilder::new(name, points).wait(true)`, chỉ khác tên collection
- **[T33]** `search_graph_nodes` KHÁC `search_chunks`: nhận `workspace_id: Uuid` trực tiếp (không `Option<Filter>`) — build filter nội bộ
- **[T33]** GraphRAG full pipeline chưa implement — T33 chỉ upsert/search vector
- **[HOTFIX]** Pool rule (verified): `admin_pool` CHỈ dùng cho migrations + platform-level provision + cross-tenant endpoint; `app_pool` BẮT BUỘC cho tenant-scoped business
- **[HOTFIX]** Middleware ordering verified: chain `auth_middleware → tenant_middleware → rls_middleware → handler` (LIFO)
- **[HOTFIX]** Tại sao KHÔNG dùng `DATABASE_URL_APP` riêng: `gmrag_app` là NOLOGIN trong migration (sqlx::test) — `SET ROLE` approach work cho cả Docker production (LOGIN) lẫn sqlx::test (NOLOGIN)

---

## 8. INVARIANTS BẮT BUỘC (tích lũy đến hết batch này)
> Đây là luật bất biến — agent vi phạm là toang
> Phải liệt kê ĐẦY ĐỦ kể cả invariant kế thừa từ batch trước

1-49. (Tất cả invariants từ Batch 1+2A+2B+3 — giữ nguyên)

50. **Two-pool design**: `init_pool` (AdminPool, gmrag superuser) CHỈ dùng cho migrations + cross-tenant; `init_app_pool` (AppPool, gmrag_app role) BẮT BUỘC cho tenant-scoped business queries
51. **`AdminPool`/`AppPool` newtype bắt buộc** vì axum extension map dùng `TypeId`
52. **Migrations chạy trên `admin_pool`** vì `gmrag_app` thiếu `CREATE` trên schema
53. **`tenant_middleware` dùng `AdminPool` cho membership check** (pre-RLS)
54. **Vector embeddings lưu ở Qdrant; PostgreSQL chỉ giữ metadata + `qdrant_point_id` UUID reference**
55. **Polymorphic ACL** (resource_type + resource_id, resource_id UUID)
56. **JSONB (không JSON)** cho properties/metadata
57. **`chat_messages.workspace_id` ON DELETE SET NULL** (khác CASCADE)
58. **`usage_events` append-only**
59. **Mọi bảng có `tenant_id` đều có RLS** với `FORCE ROW LEVEL SECURITY`
60. **`#[sqlx::test]` cache test databases** — DROP `_sqlx_test_%` trước khi chạy test khi thêm migration mới
61. **Seed chạy as superuser (gmrag)** — bypass RLS
62. **`.sqlx` offline cache cần regenerate** khi thêm/sửa `query!`/`query_as!` macro: `cargo sqlx prepare --workspace`
63. **Worker hiện dùng `init_pool` (admin/gmrag) CHỈ cho boot liveness check** — switch sang `init_app_pool` + per-job `SET LOCAL app.tenant_id = $job.tenant_id` khi T37+ dual-write
64. **`workspace_id` nullable trên documents/chat_sessions** — standalone hoặc workspace

**MỚI (Batch 4):**
65. **Qdrant client dùng gRPC port 6334** (KHÔNG REST 6333) — `QDRANT_URL` phải set `http://qdrant:6334` (đang set sai 6333 trong T9 config default)
66. **Collection naming convention**: `chunks_{tenant_id}` cho document chunks, `graph_{tenant_id}` cho graph nodes — UUID format trực tiếp vào tên
67. **Vector dim = 768, Distance = Cosine** (const `EMBED_DIM = 768` trong `gmrag_core::qdrant`) — khớp OpenAI `text-embedding-3-small` với `dimensions=768` param
68. **HNSW dùng Qdrant default** (m=16, ef_construct=100) — production tuning thuộc future perf task
69. **Payload UUID lưu dạng string** trong Qdrant (`workspace_id.to_string()`) — index `FieldType::Uuid` chấp nhận keyword match
70. **Filter UUID = keyword match** (`Condition::matches("field", uuid.to_string())`) — `MatchValue` không có variant `Uuid` trong client 1.12.1
71. **`setup_tenant_collections` PHẢI gọi trước `upsert_chunks`/`upsert_graph_nodes`** (caller responsibility — không auto-provision) — Qdrant error "collection doesn't exist" nếu thiếu
72. **`create_chunks_collection`/`create_graph_collection` private** (T30) — gọi qua `setup_tenant_collections` (pub)
73. **`teardown_tenant_collections` idempotent** — list 1 lần, check `contains`, chỉ delete khi tồn tại
74. **Multi-tenant isolation 2 lớp**: `tenant_id` = collection boundary (hard), `workspace_id` = payload filter (soft)
75. **Search chunks signature**: caller build `Option<Filter>`; search graph nodes: `search_graph_nodes` build filter nội bộ từ `workspace_id`
76. **`with_payload(true)` always, `with_vectors(false)` default** — payload include (cần cho business), vector exclude (tiết kiệm bandwidth)
77. **HNSW default `ef` không tune** — production tuning future perf task
78. **Score threshold chưa set** trên search — production RAG cần `score_threshold` param (future)
79. **`top_k` không có offset** — pagination thuộc future
80. **Read consistency default** — production multi-node cần `ReadConsistency::Majority` (future)
81. **`SET LOCAL ROLE gmrag_app`** approach (không `DATABASE_URL_APP` riêng) — work cho cả Docker production (gmrag_app LOGIN) lẫn sqlx::test (gmrag_app NOLOGIN)
82. **Pool rule bổ sung cho Worker (Batch 5)**: Worker PHẢI dùng `app_pool` + `SET LOCAL app.tenant_id` cho từng job. KHÔNG dùng `admin_pool` cho business queries.
83. **Middleware chain verified**: `auth_middleware` → `tenant_middleware` → `rls_middleware` → handler (LIFO layering)

---

## 9. ⚠️ BLOCKERS & WARNINGS CHO BATCH TIẾP THEO
> QUAN TRỌNG NHẤT — KHÔNG được tóm tắt chung chung

### P0 — Chặn đường găng (phải xử lý trước khi bắt đầu batch sau)
- **[nguồn: T27]** `QdrantConfig.url` default đang port 6333 (REST) — SAI cho rust client. `config.rs:22` `DEFAULT_QDRANT_URL = "http://localhost:6333"` và `.env.example` `QDRANT_URL=http://qdrant:6333`. → action: update `DEFAULT_QDRANT_URL` → `http://localhost:6334` và `.env.example` `QDRANT_URL=http://qdrant:6334` ở task wiring (T35+ sẽ gặp). Test T27 bypass bằng cách construct config trực tiếp với 6334.
- **[nguồn: T27]** T28/T29/T31/T32/T33 cần Qdrant sống tại `localhost:6334` (gRPC) để pass — không chỉ 6333. `docker compose ps` verify container healthy + port 6334 mapped trước khi chạy test.
- **[nguồn: T28]** `create_chunks_collection`/`create_graph_collection` KHÔNG idempotent nếu gọi trực tiếp — caller PHẢI dùng `setup_tenant_collections` (T30 pub API)
- **[nguồn: T30]** `setup_tenant_collections` KHÔNG phải transaction: nếu `create_chunks_collection` OK nhưng `create_graph_collection` fail → tenant trạng thái nửa vời. Caller cần `teardown` rồi retry. Production có thể cần retry wrapper (future task)
- **[nguồn: T30]** Idempotency dựa trên `list_collection_names` (eventual consistency) — multi-node cluster có thể lag replication. Dev = single node OK
- **[nguồn: T33]** PRE-EXISTING `gmrag-api` clippy errors (không phải T33 gây ra): `cargo clippy --workspace --all-targets -D warnings` với `SQLX_OFFLINE=true` phát hiện 2 error: (1) `unused import: sqlx::Executor` tại `crates/api/tests/pool_role.rs:12`, (2) `unused variable: decoding_key` tại `crates/api/src/auth/jwt.rs:294`. → action: HOTFIX_PRE_BATCH5 đã fix (commit `cc443ea`). Nếu batch sau gặp lại, đã xử lý.
- **[nguồn: T33]** `SQLX_OFFLINE=true` bắt buộc cho workspace clippy trên host Windows — `gmrag-api` dùng `sqlx::query_as!` macro (`crates/api/src/routes/users.rs:38,48`) cần DB sống hoặc `.sqlx` offline cache

### P1 — Lưu ý khi implement
- **[T27]** `QdrantStore::new` không retry/backoff — liveness probe fail 1 lần → return error liền. Production có thể cần retry logic (future task)
- **[T31]** `upsert_chunks` không check collection tồn tại — caller phải `setup_tenant_collections` trước (T30)
- **[T31]** Single-request upsert (không chunked) — batch lớn (>1000 point) có thể timeout. Production nên dùng `upsert_points_chunked` (future)
- **[T31]** `PointsOperationResponse` bị discard — caller không biết số point thực sự upserted
- **[T32]** Score threshold chưa set — search trả tất cả top_k kể cả score 0.0/âm. Production RAG cần `score_threshold` param
- **[T32]** `top_k` không có offset — không support pagination
- **[T33]** Graph node embed dùng `node.description` (fallback `label`) — cùng `select_embedder` (Ollama/OpenAI 768-dim) cho cả chunks + nodes → shared semantic space
- **[T33]** Graph edge relation chưa có — chỉ xử lý graph NODES. Graph EDGES lưu ở PostgreSQL `graph_edges` (T21), không lưu vector
- **[HOTFIX]** `tenant_middleware` đã populate `Extension<TenantContext>` (auth/tenant.rs:139) — KHÔNG tạo file `middleware/tenant_resolve.rs` riêng
- **[HOTFIX]** Worker (Batch 5) PHẢI dùng `app_pool` + `SET LOCAL app.tenant_id` cho từng job. KHÔNG dùng `admin_pool` cho business queries
- **[T21 migration pre-existing]** `graph_nodes` ban đầu thiếu `workspace_id` — T41 sẽ thêm (nullable cho back-compat) + `UNIQUE(tenant_id, workspace_id, label, kind)` để idempotent retry

### P2 — Ghi nhớ nhỏ
- `qdrant_client::Qdrant` không impl `Clone` — `QdrantStore` wrap `Arc<qdrant>` (api/worker share 1 store cheap)
- `QdrantStore` const `EMBED_DIM = 768` ở `core/src/qdrant/store.rs:21`
- T28/T29 test verify qua `collection_info` (private field access, cùng `mod tests` của `store.rs`)
- T30 cleanup-first pattern: test bắt đầu bằng `let _ = store.delete_collection(&name).await;` để dọn stale
- T31 test verify qua `count(exact=true)` thay vì `get_points` (lighter, đủ chứng minh point land)
- T31 helper `unit_vec(pos)`: vector 768-dim với `1.0` ở `pos`, `0.0` ở phần còn lại → cosine similarity deterministic
- T32 test recall: 3 chunk vector trực giao (`unit_vec(0)`, `unit_vec(1)`, `unit_vec(0)`); query A → cosine(A,A)=1.0, cosine(A,B)=0.0
- T33 test distinguish node 1 vs node 2 qua `entity_name` (vì `chunk_index` không có trong graph schema)
- `tenant_members`/`tenants` chưa có RLS policy mới (T25 chỉ cover 14 bảng mới) — đã có RLS từ T12/T15

---

## 10. UNBLOCKS
- Batch 4 → unblock: Batch 5 (T34-T43) — Worker crate skeleton, S3, PDF, chunking, embedding, dual-write
- Batch 4 → unblock: T35 (S3 download/upload — `S3Client::new(&cfg.s3)` reuse `S3Config` từ T9)
- Batch 4 → unblock: T36 (PDF parser — worker cần parse sau khi download)
- Batch 4 → unblock: T42 (dual-write ingestion — `QdrantStore::upsert_chunks` + `upsert_graph_nodes` sẵn sàng)
- Batch 4 → unblock: API RAG/GraphRAG retrieval (search_chunks + search_graph_nodes sẵn sàng)
- Batch 4 → unblock: T70+ Frontend (search results display)
