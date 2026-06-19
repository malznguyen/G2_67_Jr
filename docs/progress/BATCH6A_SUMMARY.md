# BATCH 6A SUMMARY — API LLM Provider Trait + BYOK Encryption
# Tasks: T44, T45 | Trạng thái: ✅ Hoàn thành
# Test count khi kết thúc batch: 87+ passed, 0 failed (workspace tests, inherited from BATCH 4)

---

## 1. TRẠNG THÁI TỔNG
| Task | Tên ngắn | Commit |
|------|----------|--------|
| T44 | API LLM provider trait + DeepSeek/OpenAI-compatible provider | (commit T44) |
| T45 | API BYOK decrypt (AES-GCM) + encrypted tenant LLM config | (commit T45) |

---

## 2. FILES ĐÃ TẠO / SỬA
| File | Task | Hành động | Ghi chú |
|------|------|-----------|---------|
| `backend/crates/api/src/llm/mod.rs` | T44 | Tạo | Module entry cho Sprint 6 API LLM |
| `backend/crates/api/src/llm/provider.rs` | T44 | Tạo | `LlmProvider`, `DeepSeekProvider`, `ProviderConfig`, `EmbeddingProviderConfig`, `ChatMessage`, `ChatDelta`, graph extraction types, SSE parser, tests |
| `backend/crates/api/src/main.rs` | T44 | Sửa | Thêm `mod llm;` |
| `backend/crates/api/Cargo.toml` | T44 | Sửa | Thêm `reqwest` stream feature, `futures`, `async-stream`, `wiremock` dev-dep |
| `backend/migrations/20260618130000_tenant_llm_config_encrypted_keys.sql` | T45 | Tạo | Add encrypted key columns + pair constraint |
| `backend/crates/api/src/llm/byok.rs` | T45 | Tạo | `resolve_llm_config`, `ResolvedLlmConfig`, `LlmConfigSource`, AES-GCM decrypt, integration tests |
| `backend/crates/api/tests/schema_llm.rs` | T45 | Tạo | Schema tests cho encrypted key columns/constraint |
| `backend/crates/core/src/config.rs` | T45 | Sửa | Parse optional `GMRAG_TENANT_KEY_ENCRYPTION_KEY` as base64 32 bytes + tests |
| `backend/crates/core/Cargo.toml` | T45 | Sửa | Thêm `base64` |
| `backend/crates/api/Cargo.toml` | T45 | Sửa | Thêm `aes-gcm` |
| `.env.example` | T45 | Sửa | Thêm `GMRAG_TENANT_KEY_ENCRYPTION_KEY=` |

---

## 3. PUBLIC API ĐÃ CÓ
> Để agent batch sau biết không implement lại

```rust
// T44 — gmrag-api::llm
pub mod provider;
pub mod byok;

// provider.rs
pub struct ProviderConfig { pub deepseek: DeepSeekConfig, pub ollama: OllamaConfig, pub embedding: EmbeddingProviderConfig }
pub struct EmbeddingProviderConfig { /* config cho embed_query */ }

pub struct ChatMessage { pub role: String, pub content: String }
pub enum ChatDelta { Text(String), Done { finish_reason: String } }

pub trait LlmProvider: Send + Sync {
    fn embed_query<'a>(&'a self, text: &'a str) -> Pin<Box<dyn Future<Output = Result<Vec<f32>, LlmError>> + Send + 'a>>;
    fn chat_stream<'a>(&'a self, messages: &'a [ChatMessage]) -> Pin<Box<dyn Stream<Item = Result<ChatDelta, LlmError>> + Send + 'a>>;
    fn graph_extract<'a>(&'a self, text: &'a str) -> Pin<Box<dyn Future<Output = Result<GraphExtraction, LlmError>> + Send + 'a>>;
}

pub struct DeepSeekProvider { /* config: base_url, api_key, model, http client */ }
impl DeepSeekProvider {
    pub fn new(cfg: ProviderConfig) -> Self
    pub fn from_resolved(resolved: &ResolvedLlmConfig, fallback: ProviderConfig) -> Self
}
impl LlmProvider for DeepSeekProvider { ... }

pub fn parse_sse_data_events(body: &str) -> Vec<ChatDelta>  // SSE parser

// T45 — byok.rs
pub enum LlmConfigSource { Tenant { provider: String, model: String }, Global }

pub struct ResolvedLlmConfig {
    pub provider: String,
    pub model: String,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub source: LlmConfigSource,
}

pub async fn resolve_llm_config(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    encryption_key: Option<&[u8; 32]>,
    global_deepseek: &DeepSeekConfig,
) -> Result<ResolvedLlmConfig, ByokError>

pub enum ByokError { MissingEncryptionKey, DecryptFailed, NoConfigFound, Db(sqlx::Error), ... }
```

---

## 4. MIGRATION STATE
| File migration | Version timestamp | Bảng tạo ra |
|---------------|------------------|-------------|
| (Tất cả migrations từ Batch 1-5 unchanged) | | |
| `20260618130000_tenant_llm_config_encrypted_keys.sql` (T45) | `20260618130000` | ALTER tenant_llm_config (add api_key_ciphertext BYTEA, api_key_nonce BYTEA + CHECK constraint) |

RLS đang enforce trên: 16 bảng cũ + `tenant_llm_config` = 17 bảng
sqlx offline cache: có (2 files từ T26)

---

## 5. ENV VARS / CONFIG
| Tên biến | Giá trị mẫu | Task thêm |
|----------|-------------|----------|
| `GMRAG_TENANT_KEY_ENCRYPTION_KEY` | base64 32 bytes (AES-256 key) | T45 |

---

## 6. DEPENDENCIES (Cargo.toml)
| Crate | Version | Thêm vào crate | Task |
|-------|---------|---------------|------|
| `reqwest` (stream feature) | 0.12 | `gmrag-api` | T44 |
| `futures` | 0.3 | `gmrag-api` | T44 |
| `async-stream` | workspace | `gmrag-api` | T44 |
| `wiremock` (dev) | 0.6 | `gmrag-api` (dev) | T44 |
| `base64` | workspace | `gmrag-core` | T45 |
| `aes-gcm` | workspace | `gmrag-api` | T45 |

---

## 7. QUYẾT ĐỊNH KỸ THUẬT CÓ IMPACT CROSS-TASK
> Chỉ giữ quyết định ảnh hưởng tới batch sau, bỏ quyết định internal

- **[T44]** Trait object-safe bằng boxed future/stream — tránh `async_trait` dependency mới trong API
- **[T44]** `chat_stream` trả `ChatStream` provider-level — Sprint này parse SSE thành delta; T49 sẽ bọc thành Axum SSE route/citation parser
- **[T44]** Embedding mặc định vẫn Ollama: `ProviderConfig::from_global` dùng DeepSeek cho chat/graph và Ollama cho query embedding
- **[T44]** OpenAI embedding vẫn pin 768 dim: Request `/embeddings` gửi `dimensions=768` (giữ tương thích Qdrant collection 768-dim)
- **[T44]** DeepSeek endpoint: `POST {base_url}/chat/completions` cho chat stream + graph extraction
- **[T44]** Ollama endpoint: `POST {ollama_host}/api/embed` cho default query embedding
- **[T44]** OpenAI endpoint: `POST {openai_base_url}/embeddings` cho BYOK OpenAI embedding (T45 resolver)
- **[T44]** Provider-level SSE parser chuyển OpenAI-compatible `data:` events thành `ChatDelta`
- **[T44]** Graph extraction types + tolerant JSON parser — giữ logic tương thích với worker T41
- **[T45]** AES-256-GCM với AAD = tenant UUID bytes — ciphertext bind với tenant cụ thể, tránh copy ciphertext sang tenant khác rồi decrypt thành công
- **[T45]** Fail rõ khi decrypt lỗi: Nếu encrypted fields tồn tại nhưng thiếu env key, nonce sai, hoặc ciphertext hỏng thì resolver trả lỗi; KHÔNG fallback global và KHÔNG dùng plaintext che lỗi
- **[T45]** Additive migration: KHÔNG đổi/xóa `api_key` (TEXT); worker hiện vẫn đọc plaintext được cho đến task refactor riêng
- **[T45]** Resolver dùng `&mut PgConnection` đã có RLS context từ middleware — KHÔNG tự mở pool/transaction mới
- **[T45]** Plaintext `api_key` giữ read-only fallback CHỈ khi encrypted fields vắng mặt — không phá worker T40/T41 hiện tại
- **[T45]** `GMRAG_TENANT_KEY_ENCRYPTION_KEY` value là base64 của 32-byte AES-256 key

---

## 8. INVARIANTS BẮT BUỘC (tích lũy đến hết batch này)
> Đây là luật bất biến — agent vi phạm là toang
> Phải liệt kê ĐẦY ĐỦ kể cả invariant kế thừa từ batch trước

1-135. (Tất cả invariants từ Batch 1+2A+2B+3+4+5A+5B+5C — giữ nguyên)

**MỚI (Batch 6A):**
136. **`LlmProvider` trait object-safe** — dùng boxed future/stream, KHÔNG dùng `async_trait` mới
137. **`chat_stream` trả provider-level** — T49 sẽ bọc thành Axum SSE route
138. **Embedding mặc định vẫn Ollama** — `ProviderConfig::from_global` dùng DeepSeek cho chat/graph + Ollama cho query embed
139. **OpenAI embedding pin 768 dim** — request `{model, input, dimensions: 768}`
140. **SSE parser OpenAI-compatible** — chuyển `data:` events thành `ChatDelta`
141. **AES-256-GCM cho BYOK** — AAD = tenant UUID bytes (bind ciphertext với tenant)
142. **Fail rõ khi decrypt lỗi** — KHÔNG fallback global khi encrypted fields tồn tại mà decrypt fail
143. **Additive migration cho encrypted BYOK** — KHÔNG đổi/xóa plaintext `api_key` (worker T40/T41 vẫn đọc được)
144. **`resolve_llm_config` dùng `&mut PgConnection` đã có RLS context** — KHÔNG tự mở pool/transaction
145. **Plaintext `api_key` read-only fallback** — CHỈ khi encrypted fields vắng mặt
146. **`GMRAG_TENANT_KEY_ENCRYPTION_KEY` required** cho BYOK encrypted — base64 32 bytes

---

## 9. ⚠️ BLOCKERS & WARNINGS CHO BATCH TIẾP THEO
> QUAN TRỌNG NHẤT — KHÔNG được tóm tắt chung chung

### P0 — Chặn đường găng (phải xử lý trước khi bắt đầu batch sau)
- **[nguồn: T45]** Chưa có API endpoint ghi BYOK settings; endpoint đó phải encrypt trước khi insert/update encrypted fields
- **[nguồn: T45]** Worker T40/T41 vẫn đọc plaintext `api_key`; khi chuyển worker sang encrypted BYOK cần reuse/di chuyển decrypt helper hoặc tạo shared core module
- **[nguồn: T44]** T46 có thể gọi `embed_query` để build retrieval query vector, nhưng ACL/resource filtering vẫn thuộc T46/T47
- **[nguồn: T44]** T49 vẫn cần route streaming thật ở Axum layer; T44 chỉ cung cấp provider stream abstraction và parser delta

### P1 — Lưu ý khi implement
- **[T44]** `LlmProvider` trait object-safe — handler giữ `Box<dyn LlmProvider>`
- **[T44]** `chat_stream` trả `ChatStream` provider-level, chưa phải Axum SSE — T49 sẽ wrap
- **[T45]** Decrypt fail phải trả error rõ (không fallback)
- **[T45]** AAD = tenant UUID bytes — bind ciphertext với tenant

### P2 — Ghi nhớ nhỏ
- `DeepSeekProvider` dùng cho cả DeepSeek + OpenAI BYOK (OpenAI-compatible `/chat/completions`)
- `ProviderConfig::from_global` → DeepSeek cho chat/graph + Ollama cho query embed
- `GMRAG_TENANT_KEY_ENCRYPTION_KEY` parse optional — nếu thiếu thì resolver không thể decrypt encrypted BYOK
- `tenant_llm_config` columns mới: `api_key_ciphertext BYTEA`, `api_key_nonce BYTEA` + CHECK constraint bắt buộc có cả hai hoặc không có
- `resolved_llm_config` 2 fallback: tenant (encrypted) → global (plaintext)
- `parse_sse_data_events` — OpenAI-compatible SSE parser

---

## 10. UNBLOCKS
- Batch 6A → unblock: T46 (chunk retrieval) — dùng `LlmProvider::embed_query`
- Batch 6A → unblock: T47 (graph retrieval) — embed query + ILIKE fallback
- Batch 6A → unblock: T48 (assemble_system_prompt) — pure function
- Batch 6A → unblock: T49 (streaming.rs DeepSeek SSE stream) — provider abstraction
- Batch 6A → unblock: API RAG query endpoint (handler mount ở T61)
