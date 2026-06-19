# BATCH 2A SUMMARY — Backend Auth, Tenant Context, Identity Migrations
# Tasks: T9, T10, T11, T12 | Trạng thái: ✅ Hoàn thành
# Test count khi kết thúc batch: 24 passed, 0 failed (workspace tests)

---

## 1. TRẠNG THÁI TỔNG
| Task | Tên ngắn | Commit |
|------|----------|--------|
| T9 | core config struct + env parsing TDD (subsystem configs) | `bd5af2e` |
| T10 | Keycloak OIDC JWT validation + JWKS caching | `b8e8776` |
| T11 | AuthUser axum extractor | `250d049` |
| T12 | migrations: users, tenants, tenant_members, platform_admins + RLS | `42dee00` |

---

## 2. FILES ĐÃ TẠO / SỬA
| File | Task | Hành động | Ghi chú |
|------|------|-----------|---------|
| `backend/crates/core/src/config.rs` | T9 | Sửa | Added `OidcConfig`, `QdrantConfig`, `S3Config`, `RedisConfig`, `OllamaConfig`, `DeepSeekConfig` structs + `from_process_env()` pub(crate) + 5 new tests |
| `backend/crates/api/src/auth/mod.rs` | T10 | Tạo | Auth module root |
| `backend/crates/api/src/auth/jwt.rs` | T10 | Tạo | `JwtValidator` with JWKS caching, OIDC discovery, 8 tests |
| `backend/crates/api/src/auth/test_keys/test_rsa_private.pem` | T10 | Tạo | RSA 2048 test private key (force-add despite `*.pem` gitignore) |
| `backend/crates/api/src/auth/test_keys/test_rsa_public.pem` | T10 | Tạo | RSA 2048 test public key (force-add) |
| `backend/crates/api/src/error.rs` | T10 | Tạo | `AuthError` enum |
| `backend/crates/api/src/main.rs` | T10 | Sửa | Added `mod auth; mod error;` |
| `backend/crates/api/Cargo.toml` | T10 | Sửa | Added `jsonwebtoken`, `reqwest` deps |
| `backend/crates/api/src/auth/extractor.rs` | T11 | Tạo | `AuthUser` struct + `FromRequestParts` impl + `AuthState` + 6 tests |
| `backend/crates/api/src/auth/mod.rs` | T11 | Sửa | Added `pub mod extractor;` |
| `backend/crates/api/src/error.rs` | T11 | Sửa | Added `IntoResponse` impl for `AuthError` |
| `backend/migrations/20260617124018_identity_and_tenant.sql` | T12 | Tạo | DDL 4 domain tables + RLS policy + grants |

---

## 3. PUBLIC API ĐÃ CÓ
> Để agent batch sau biết không implement lại

```rust
// T9 — gmrag-core config
impl Config {
    pub fn from_env() -> Result<Self, Error>
    pub(crate) fn from_process_env() -> Result<Self, Error>
}
pub struct OidcConfig { pub issuer: String, pub client_id: String, pub client_secret: String, pub frontend_client_id: Option<String> }
pub struct QdrantConfig { pub url: String, pub api_key: Option<String>, pub collection_default: Option<String> }
pub struct S3Config { pub endpoint: String, pub public_endpoint: Option<String>, pub region: Option<String>, pub access_key: String, pub secret_key: String, pub bucket: String, pub force_path_style: bool }
pub struct RedisConfig { pub url: String }
pub struct OllamaConfig { pub host: String, pub embed_model: String, pub llm_model: String, pub keep_alive: String }
pub struct DeepSeekConfig { pub api_key: Option<String>, pub base_url: String, pub model: String, pub timeout_s: u64 }

// T10 — gmrag-api auth
pub struct JwtValidator { /* http_client, issuer, client_id, jwks_cache */ }
impl JwtValidator {
    pub fn new(issuer: String, client_id: String) -> Self
    pub fn with_cache_ttl(mut self, ttl: Duration) -> Self
    pub async fn validate(&self, token: &str) -> Result<JwtClaims, AuthError>
}
pub struct JwtClaims { pub sub: Uuid, pub aud: String, pub email: String, pub preferred_username: String, pub realm_access: ... }
pub enum AuthError { InvalidToken, MissingHeader, JwksFetchFailed, UserNotFound, ... }
impl AuthError { pub fn code(&self) -> &'static str }

// T11 — extractor
pub struct AuthUser { pub user_id: Uuid, pub claims: JwtClaims }
pub struct AuthState { pub jwt_validator: JwtValidator }
impl FromRequestParts for AuthUser { ... }  // reads AuthState from extensions

// T12 — schema
// tables: users, tenants, tenant_members, platform_admins
// tenant_members: RLS enabled with policy `tenant_members_isolation` filtering by `gmrag_current_tenant()`
// index: idx_users_email, idx_tenant_members_user
```

---

## 4. MIGRATION STATE
| File migration | Version timestamp | Bảng tạo ra |
|---------------|------------------|-------------|
| `backend/migrations/20260101000000_init.sql` | `20260101000000` | (placeholder) |
| `backend/migrations/20260617124018_identity_and_tenant.sql` | `20260617124018` | `users`, `tenants`, `tenant_members`, `platform_admins` |

RLS đang enforce trên: `tenant_members` (policy `tenant_members_isolation` filter `gmrag_current_tenant()`)
sqlx offline cache: **chưa có**

---

## 5. ENV VARS / CONFIG
| Tên biến | Giá trị mẫu | Task thêm |
|----------|-------------|----------|
| `KEYCLOAK_ISSUER` | `http://keycloak:8080/realms/gmrag` | T9 |
| `KEYCLOAK_CLIENT_ID` | `gmrag-backend` | T9 |
| `KEYCLOAK_CLIENT_SECRET` | `secret` | T9 |
| `KEYCLOAK_FRONTEND_CLIENT_ID` | `gmrag-frontend` (default) | T9 |
| `QDRANT_URL` | `http://qdrant:6333` (default) | T9 |
| `QDRANT_API_KEY` | (optional) | T9 |
| `QDRANT_COLLECTION_DEFAULT` | `gmrag_chunks` (default) | T9 |
| `S3_ENDPOINT` | `http://minio:9000` | T9 |
| `S3_PUBLIC_ENDPOINT` | `http://localhost:9000` (optional) | T9 |
| `S3_REGION` | `us-east-1` (optional) | T9 |
| `S3_ACCESS_KEY` / `S3_SECRET_KEY` | required | T9 |
| `S3_BUCKET` | `gmrag-uploads` | T9 |
| `S3_FORCE_PATH_STYLE` | `true` (default) | T9 |
| `REDIS_URL` | `redis://redis:6379/0` (default) | T9 |
| `OLLAMA_HOST` | `http://ollama:11434` (default) | T9 |
| `OLLAMA_EMBED_MODEL` | `nomic-embed-text` (default) | T9 |
| `OLLAMA_LLM_MODEL` | `llama3.1:8b` (default) | T9 |
| `OLLAMA_KEEP_ALIVE` | `30m` (default) | T9 |
| `DEEPSEEK_API_KEY` | (optional) | T9 |
| `DEEPSEEK_BASE_URL` | `https://api.deepseek.com/v1` (default) | T9 |
| `DEEPSEEK_MODEL` | `deepseek-v4-flash` (default) | T9 |
| `DEEPSEEK_TIMEOUT_S` | `60` (default) | T9 |

---

## 6. DEPENDENCIES (Cargo.toml)
| Crate | Version | Thêm vào crate | Task |
|-------|---------|---------------|------|
| `jsonwebtoken` | workspace | `gmrag-api` | T10 |
| `reqwest` | 0.12, features: `rustls-tls`, `json` | `gmrag-api` | T10 |
| `axum` (extension) | 0.7 | `gmrag-api` | T11 |
| `tokio` (sync `RwLock`) | 1 | `gmrag-api` | T10 |

---

## 7. QUYẾT ĐỊNH KỸ THUẬT CÓ IMPACT CROSS-TASK
> Chỉ giữ quyết định ảnh hưởng tới batch sau, bỏ quyết định internal

- **[T9]** `from_process_env()` là `pub(crate)` để test isolation; `from_env()` (public) loads `.env` first — agent sau dùng `from_env()` cho production code, `from_process_env()` chỉ trong test
- **[T9]** Optional env fields dùng `Option<String>` với pattern `env::var().ok().filter(|s| !s.is_empty())` — agent sau follow pattern này cho env mới
- **[T9]** Mỗi subsystem (OIDC, Qdrant, S3, Redis, Ollama, DeepSeek) có struct riêng trong `Config` — sẵn sàng split thành submodule khi cần
- **[T10]** JWT validation dùng `reqwest` với `rustls-tls` (không `native-tls`) — lý do: portability trong Docker slim images
- **[T10]** JWKS cache dùng `tokio::sync::RwLock` với TTL 300s default
- **[T10]** OIDC discovery fetch `/.well-known/openid-configuration` để tìm `jwks_uri`
- **[T10]** `JwtClaims` có field `aud` (Keycloak puts client_id in both `aud` and `azp`) — agent sau dùng `aud` cho audience validation
- **[T11]** `AuthUser` chứa cả `user_id: Uuid` + raw `claims: JwtClaims` — downstream có thể access email, username, roles
- **[T11]** `AuthState` inject qua `axum::Extension`, KHÔNG qua axum `State` — lý do: extractor work với mọi route mà không cần `State<AuthState>` trong handler signature
- **[T11]** `AuthError` implement `IntoResponse` với HTTP code: 401 cho auth failures, 503 cho JWKS fetch failures
- **[T12]** Tenant identity flow invariant: header → middleware → RLS context — `tenant_members` đã có RLS, `users`/`tenants` chưa (global reference tables) — RLS sẽ thêm khi cần tenant-scoped operations
- **[T12]** `platform_admins` không có RLS (intentional global table cho super-user)
- **[T12]** `gen_random_uuid()` từ `pgcrypto` extension dùng cho default UUIDs
- **[T12]** Indexes: `idx_users_email` (fast email lookup), `idx_tenant_members_user` (reverse membership)

---

## 8. INVARIANTS BẮT BUỘC (tích lũy đến hết batch này)
> Đây là luật bất biến — agent vi phạm là toang
> Phải liệt kê ĐẦY ĐỦ kể cả invariant kế thừa từ batch trước

1. **Tenant identity phải tới từ request context** (qua `TenantContext` middleware ở task sau), KHÔNG pull từ URL hoặc config tĩnh — lý do: multi-tenant isolation
2. **Domain schema đi qua `sqlx::migrate!`**, KHÔNG edit `infra/postgres/init.sql` sau khi volume đã init — lý do: bind-mount `:ro` chỉ chạy 1 lần
3. **Mọi Rust binary dùng `gmrag_core::Config::from_env()`** để đọc env — lý do: 1 source of truth, fail-fast chỉ `DATABASE_URL` required
4. **Mọi binary boot sequence**: tracing → config → pool → `sqlx::migrate!` → serve/loop — lý do: thống nhất observable
5. **Liveness/readiness split**: `/health` không touch DB, `/healthz` ping DB
6. **`/health` HTTP 200 = service alive**; compose healthcheck dùng `/healthz`
7. **Docker compose context = repo root** (build context `..` relative to `infra/`)
8. **Env serialised trong test** bằng `static ENV_LOCK: Mutex<()>` — lý do: tránh race
9. **Workspace deps pin trong `[workspace.dependencies]`**
10. **Commit `Cargo.lock`** cho binary workspace
11. **Migrations ở `backend/migrations/`**
12. **`pgcrypto` extension bắt buộc** cho `gen_random_uuid()`
13. **`gmrag_current_tenant()` function tồn tại trong DB** (từ init.sql) — đang được dùng bởi RLS policy `tenant_members_isolation`
14. **`gmrag_app` role tồn tại trong DB** (least-privilege) — sẽ dùng cho RLS-enforced queries ở batch sau
15. **Frontend App Router** (không Pages Router)
16. **Frontend `output: "standalone"`**
17. **Corepack pin pnpm version**
18. **`.env` (local, gitignored) phải tạo từ `.env.example`**
19. **Mọi env var backend phải có trong `.env.example`**
20. **Compose port mapping**: backend `8088:8080`, frontend `3000:3000`
21. **JWT auth dùng `rustls-tls`** (không `native-tls`) cho Docker portability
22. **JWKS cache TTL 300s** default; OIDC discovery lookup `jwks_uri` ở first validate
23. **`AuthState` inject qua `axum::Extension`**, KHÔNG qua axum `State` — lý do: extractor work với mọi route
24. **HTTP error mapping AuthError**: 401 cho auth failures, 503 cho JWKS fetch failures
25. **`tenant_members` RLS policy** `tenant_members_isolation` filter `gmrag_current_tenant()` — mọi query phải respect
26. **`users`/`tenants`/`platform_admins` chưa có RLS** (intentional global reference) — sẽ thêm khi cần tenant-scoped operations
27. **Tenant-scoped queries phải set `app.tenant_id` GUC** (qua middleware ở batch sau) để RLS filter hoạt động
28. **`AuthUser.claims` field luôn available** trong handler — downstream có thể access email/username/roles
29. **Test RSA keys ở `auth/test_keys/` phải force-add** (`git add -f`) vì `*.pem` trong `.gitignore` — contributor mới regenerate phải làm tương tự
30. **`MissingHeader` + `UserNotFound` variants trong `AuthError` hiện unused** — sẽ dùng ở T13+ (extractor) khi cần verify user exists trong DB
31. **`with_cache_ttl` method on `JwtValidator` available** cho tests cần custom TTL

---

## 9. ⚠️ BLOCKERS & WARNINGS CHO BATCH TIẾP THEO
> QUAN TRỌNG NHẤT — KHÔNG được tóm tắt chung chung

### P0 — Chặn đường găng (phải xử lý trước khi bắt đầu batch sau)
- **[nguồn: T10]** `test_keys/` directory chứa RSA keys bị gitignore bởi `*.pem` rule; đã force-add với `git add -f` — action: contributor mới regenerate keys phải `git add -f` tương tự
- **[nguồn: T12]** `infra/postgres/init.sql` PHẢI apply vào DB trước khi chạy migrations — lý do: `gmrag_current_tenant()` function required bởi RLS policy → action: chạy init.sql (Docker tự động) hoặc apply manually
- **[nguồn: T12]** `vector` extension KHÔNG có sẵn trong `postgres:16-alpine` image — pgvector sẽ install qua custom image hoặc official pgvector image ở task sau (warning, không phải blocker MVP)
- **[nguồn: T9]** `main.rs` trong `gmrag-api` vẫn dùng old `Config` field set — cần update khi wire new subsystem configs vào AppState (task sau)

### P1 — Lưu ý khi implement
- **[T9]** Mọi test config dùng `Config::from_process_env()` (pub(crate)) — KHÔNG dùng `from_env()` trong test vì `.env` file sẽ pollute test env
- **[T10]** `claims` field trên `AuthUser` hiện unused — sẽ dùng khi routes cần email/username/roles
- **[T11]** `UserNotFound` variant kept cho future khi cần verify user exists in DB
- **[T12]** `platform_admins` table KHÔNG có RLS — intentional global
- **[T12]** Indexes đã tạo (`idx_users_email`, `idx_tenant_members_user`) — query mới nên dùng index này

### P2 — Ghi nhớ nhỏ
- `infra/postgres/init.sql` đã apply manually trong T12 (vì sqlx::test DB skip init.sql) — production Docker tự động apply
- `JwtClaims` có `aud` field cho audience validation — Keycloak puts client_id in both `aud` and `azp`
- `OIDC` discovery URL: `{issuer}/.well-known/openid-configuration`
- Module path: `auth::jwt::JwtValidator`, `auth::extractor::AuthUser`/`AuthState`

---

## 10. UNBLOCKS
- Batch 2A → unblock: T13 (TenantContext extractor cần `AuthUser` + DB check)
- Batch 2A → unblock: T14 (RLS middleware cần TenantContext + PgPool)
- Batch 2A → unblock: T15 (RLS policies + isolation testing)
- Batch 2A → unblock: T18 (GET /users/me cần `AuthUser` + `PgPool`)
