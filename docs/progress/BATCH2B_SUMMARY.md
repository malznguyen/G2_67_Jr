# BATCH 2B SUMMARY — RLS Middleware, Isolation, Provision, Error Envelope, /users/me
# Tasks: T13, T14, T15, T16, T17, T18, HOTFIX_BATCH2B | Trạng thái: ✅ Hoàn thành
# Test count khi kết thúc batch: 51 passed, 0 failed (workspace tests, sau HOTFIX)

---

## 1. TRẠNG THÁI TỔNG
| Task | Tên ngắn | Commit |
|------|----------|--------|
| T13 | TenantContext extractor + membership validation | `975e6a1` |
| T14 | RLS middleware for setting `app.tenant_id` | `e4112b2` |
| T15 | RLS policies for tenants + isolation testing | `350eda4` |
| T16 | Auto-provision user from Keycloak claims | `1e5a7fa` |
| T17 | Global error envelope JSON formatter | `b009fa0` |
| T18 | GET /users/me endpoint | `1d19581` |
| HOTFIX | Post Batch 2B: wire PgPool+AuthState, audit, bless migration checksum | `4a8c17c` |

---

## 2. FILES ĐÃ TẠO / SỬA
| File | Task | Hành động | Ghi chú |
|------|------|-----------|---------|
| `backend/crates/api/src/auth/tenant.rs` | T13 | Tạo | `TenantContext` extractor + `FromRequestParts` + 3 tests |
| `backend/crates/api/src/auth/mod.rs` | T13 | Sửa | Added `pub mod tenant;` |
| `backend/crates/api/src/auth/extractor.rs` | T13 | Sửa | `AuthUser` stores self in request extensions after extraction |
| `backend/crates/api/src/middleware/mod.rs` | T14 | Tạo | Module declaration for middleware |
| `backend/crates/api/src/middleware/rls.rs` | T14 | Tạo | `SharedConnection` + `rls_middleware` + 3 tests |
| `backend/crates/api/src/main.rs` | T14 | Sửa | Added `mod middleware;` |
| `backend/migrations/20260617132425_rls_tenants_table.sql` | T15 | Tạo | ALTER TABLE tenants ENABLE/FORCE ROW LEVEL SECURITY + policy |
| `backend/migrations/20260617124018_identity_and_tenant.sql` | T15 | Sửa | Added `pgcrypto`, `gmrag_app` role, `gmrag_current_tenant()` cho sqlx::test |
| `backend/crates/api/tests/rls_isolation.rs` | T15 | Tạo | 5 integration tests using `#[sqlx::test]` |
| `backend/crates/api/src/auth/provision.rs` | T16 | Tạo | `provision_user()` + 2 tests |
| `backend/crates/api/src/auth/mod.rs` | T16 | Sửa | Added `pub mod provision;` |
| `backend/crates/api/src/auth/extractor.rs` | T16 | Sửa | `AuthUser` calls `provision_user` after JWT validation |
| `backend/crates/api/src/auth/tenant.rs` | T16 | Sửa | Removed PgPool from test app (avoid provision conflict) |
| `backend/crates/api/src/error.rs` | T17 | Sửa | `AuthError` → string codes, added `ApiError` enum + 12 tests |
| `backend/crates/api/src/routes/mod.rs` | T18 | Tạo | Module declaration for routes |
| `backend/crates/api/src/routes/users.rs` | T18 | Tạo | GET /users/me handler + 2 tests |
| `backend/crates/api/src/main.rs` | T18 | Sửa | Added `mod routes;` + mounted `/users/me` route |
| `backend/crates/api/src/main.rs` | HOTFIX | Sửa | Wire `Extension(pool)` + `Extension(auth_state)`; inline `AppState`; `#![allow(dead_code)]` crate-level |

---

## 3. PUBLIC API ĐÃ CÓ
> Để agent batch sau biết không implement lại

```rust
// T13
pub struct TenantContext(pub Uuid);
pub const TENANT_HEADER: &str = "x-tenant-id";  // lowercase
impl FromRequestParts for TenantContext { ... }  // reads AuthUser from extensions, then header

// T14
pub struct SharedConnection(Arc<Mutex<PgConnection>>);
impl SharedConnection { /* deref to PgConnection */ }
impl Clone for SharedConnection { ... }
pub async fn rls_middleware(
    State(state): State<AppState>,
    Extension(tenant_ctx): Extension<TenantContext>,
    Extension(pool): Extension<PgPool>,
    request: Request,
    next: Next,
) -> Result<Response, ApiError>

// T15
// Migration adds: ALTER TABLE tenants ENABLE/FORCE ROW LEVEL SECURITY
// + policy using gmrag_current_tenant()

// T16
pub async fn provision_user(pool: &PgPool, claims: &JwtClaims) -> Result<(), sqlx::Error>
// ON CONFLICT (id) DO UPDATE — graceful profile changes

// T17
pub enum ApiError {
    BadRequest(String),
    Unauthorized(AuthError),
    Forbidden(String),
    NotFound(String),
    Internal(String),
    Core(gmrag_core::Error),
    // ... mapped to JSON envelope { error: { code, message } }
}
impl ApiError { pub fn code(&self) -> String }
impl IntoResponse for ApiError { ... }
impl From<AuthError> for ApiError { ... }
impl From<gmrag_core::Error> for ApiError { ... }
impl From<sqlx::Error> for ApiError { ... }

// T18
pub async fn get_me(
    Extension(pool): Extension<PgPool>,
    Extension(auth_user): Extension<AuthUser>,
) -> Result<Json<MeResponse>, ApiError>
// Response: { user: {id, email, name, created_at}, tenants: [{id, name, role}] }
```

---

## 4. MIGRATION STATE
| File migration | Version timestamp | Bảng tạo ra |
|---------------|------------------|-------------|
| `backend/migrations/20260101000000_init.sql` | `20260101000000` | (placeholder) |
| `backend/migrations/20260617124018_identity_and_tenant.sql` | `20260617124018` | `users`, `tenants`, `tenant_members`, `platform_admins` + `pgcrypto` + `gmrag_app` role + `gmrag_current_tenant()` |
| `backend/migrations/20260617132425_rls_tenants_table.sql` | `20260617132425` | RLS policy trên `tenants` (FORCE) |

RLS đang enforce trên: `tenant_members` (T12), `tenants` (T15 — FORCE)
sqlx offline cache: **chưa có**

---

## 5. ENV VARS / CONFIG
N/A — không thêm env mới ở batch này.

---

## 6. DEPENDENCIES (Cargo.toml)
| Crate | Version | Thêm vào crate | Task |
|-------|---------|---------------|------|
| (none) | — | — | — |

---

## 7. QUYẾT ĐỊNH KỸ THUẬT CÓ IMPACT CROSS-TASK
> Chỉ giữ quyết định ảnh hưởng tới batch sau, bỏ quyết định internal

- **[T13]** `AuthUser` stores self in extensions sau extraction — cho phép downstream extractor chain (`TenantContext`, future) access user mà không cần handler param
- **[T13]** Handler parameter order: `AuthUser` PHẢI trước `TenantContext` (vì TenantContext đọc AuthUser từ extensions)
- **[T13]** Tenant membership check: `SELECT EXISTS(SELECT 1 FROM tenant_members WHERE tenant_id = $1 AND user_id = $2)` — single-row check, 403 nếu không phải member
- **[T13]** `TENANT_HEADER` hardcoded const `"x-tenant-id"` (lowercase) — `Config.tenant_header` field available nhưng chưa wire (integration ở main.rs sau)
- **[T14]** `SharedConnection` pattern: `Arc<Mutex<PgConnection>>` wrapped in Clone struct — mutex guard held briefly
- **[T14]** Transaction lifecycle: middleware `BEGIN` → `SET LOCAL app.tenant_id` → handler → `COMMIT`. `SET LOCAL` scoped to transaction — khi connection return về pool, tenant setting gone
- **[T14]** Connection detach: `PoolConnection::detach()` để lấy raw `PgConnection` quản lý độc lập pool
- **[T14]** Middleware phải layer AFTER TenantContext extraction (TenantContext phải ở extensions trước khi middleware chạy)
- **[T15]** `FORCE ROW LEVEL SECURITY` bắt buộc vì `DATABASE_URL` user là superuser — without FORCE, superusers bypass RLS
- **[T15]** Tests dùng `SET LOCAL ROLE gmrag_app` để PostgreSQL enforce RLS policies
- **[T15]** `gmrag_current_tenant()` + `gmrag_app` role thêm vào `identity_and_tenant` migration — sqlx::test DB skip init.sql nên cần function có sẵn trong migration
- **[T16]** `provision_user` optional qua `if let Some(pool) = parts.extensions.get::<PgPool>()` — silent skip nếu pool missing; production phải có pool
- **[T16]** `ON CONFLICT (id) DO UPDATE` — update email/name latest claims, handle profile changes
- **[T17]** `AuthError` codes đổi từ integer sang kebab-case string (`"invalid-token"`, `"missing-header"`, `"jwks-fetch-failed"`, `"user-not-found"`) — BREAKING CHANGE cho frontend
- **[T17]** Envelope invariant: `{ error: { code: <string>, message: <string> } }` cho MỌI error path
- **[T18]** Handler dùng `Extension<AuthUser>` + `Extension<PgPool>` — KHÔNG dùng `TenantContext` (cross-tenant, list all memberships)
- **[T18]** Response shape: `{ user: {id, email, name, created_at}, tenants: [{id, name, role}] }`
- **[T18]** `sqlx::query_as!` macro yêu cầu `DATABASE_URL` lúc compile — CI cần DB sống hoặc `.sqlx` offline cache
- **[HOTFIX]** `AuthState::new(&config).await?` KHÔNG tồn tại — struct chỉ có field `pub jwt_validator`; `JwtValidator::new` không async (build trực tiếp, JWKS lazy ở first validate)
- **[HOTFIX]** Middleware ordering LIFO: `.layer()` gọi sau = chạy TRƯỚC — chain `auth_middleware → tenant_middleware → rls_middleware → handler`
- **[HOTFIX]** `#![allow(dead_code)]` crate-level tạm thời cho `middleware::rls::*` (chưa wire thật) + reserved `ApiError` variants

---

## 8. INVARIANTS BẮT BUỘC (tích lũy đến hết batch này)
> Đây là luật bất biến — agent vi phạm là toang
> Phải liệt kê ĐẦY ĐỦ kể cả invariant kế thừa từ batch trước

1. **Tenant identity phải tới từ request context** (qua `TenantContext` middleware/extractor), KHÔNG pull từ URL hoặc config tĩnh
2. **Domain schema đi qua `sqlx::migrate!`**, KHÔNG edit `infra/postgres/init.sql` sau khi volume đã init
3. **Mọi Rust binary dùng `gmrag_core::Config::from_env()`**
4. **Mọi binary boot sequence**: tracing → config → pool → `sqlx::migrate!` → serve/loop
5. **Liveness/readiness split**: `/health` không touch DB, `/healthz` ping DB
6. **`/health` HTTP 200 = service alive**; compose healthcheck dùng `/healthz`
7. **Docker compose context = repo root**
8. **Env serialised trong test** bằng `static ENV_LOCK: Mutex<()>`
9. **Workspace deps pin trong `[workspace.dependencies]`**
10. **Commit `Cargo.lock`**
11. **Migrations ở `backend/migrations/`**
12. **`pgcrypto` extension bắt buộc**
13. **`gmrag_current_tenant()` function tồn tại trong DB**
14. **`gmrag_app` role tồn tại trong DB** — tests dùng `SET LOCAL ROLE gmrag_app` để RLS enforce
15. **Frontend App Router**
16. **Frontend `output: "standalone"`**
17. **Corepack pin pnpm version**
18. **`.env` (local, gitignored) phải tạo từ `.env.example`**
19. **Mọi env var backend phải có trong `.env.example`**
20. **Compose port mapping**: backend `8088:8080`, frontend `3000:3000`
21. **JWT auth dùng `rustls-tls`**
22. **JWKS cache TTL 300s** default
23. **`AuthState` inject qua `axum::Extension`**, KHÔNG qua axum `State`
24. **HTTP error mapping AuthError**: 401 cho auth failures, 503 cho JWKS fetch failures
25. **`tenant_members` RLS policy** `tenant_members_isolation` filter `gmrag_current_tenant()`
26. **`tenants` RLS policy** với FORCE ROW LEVEL SECURITY (T15)
27. **Tenant-scoped queries phải set `app.tenant_id` GUC** qua middleware để RLS filter hoạt động
28. **`AuthUser.claims` field luôn available** trong handler
29. **Test RSA keys ở `auth/test_keys/` phải force-add**
30. **`MissingHeader` + `UserNotFound` variants dùng ở T13+**
31. **`with_cache_ttl` available** cho tests custom TTL
32. **Handler parameter order**: `AuthUser` TRƯỚC `TenantContext` (vì TenantContext đọc AuthUser từ extensions)
33. **`TENANT_HEADER` = `"x-tenant-id"` (lowercase)** — wire `Config.tenant_header` vào extractor ở main.rs integration
34. **`SharedConnection` PHẢI được dùng bởi tenant-scoped handler** (qua `rls_middleware`) — handler nào không dùng `SharedConnection` (dùng `PgPool` trực tiếp) sẽ KHÔNG có RLS context
35. **Middleware ordering LIFO**: `.layer()` gọi sau = chạy TRƯỚC; chain `auth → tenant → rls → handler`
36. **Tenant-scoped handler dùng `Extension<SharedConnection>`**, KHÔNG extract `TenantContext` trực tiếp
37. **RLS policy uniform** `tenant_id = gmrag_current_tenant()` cho mọi bảng có `tenant_id` (denormalized)
38. **Tests dùng `SET LOCAL ROLE gmrag_app`** để PostgreSQL enforce RLS (vì owner/superuser bypass)
39. **`FORCE ROW LEVEL SECURITY` bắt buộc** trên mọi bảng có RLS (vì user chính là owner)
40. **`provision_user` optional nếu PgPool missing** — production PHẢI có PgPool trong extensions
41. **Profile changes via `ON CONFLICT (id) DO UPDATE`** — email/name update mỗi auth request
42. **Error envelope PHẢI là** `{ error: { code: <string>, message: <string> } }` — kebab-case string codes
43. **`AuthError` codes là kebab-case strings** (KHÔNG integer) — frontend PHẢI update error parsing
44. **Cross-tenant handler (`get_me`)** dùng `PgPool` (AdminPool) — KHÔNG ép `SharedConnection` (sẽ scope về 1 tenant, vi phạm business)
45. **Platform-level extractors/helpers dùng `PgPool` (AdminPool)**: `AuthUser` provisioning, `TenantContext` membership check (phải chạy TRƯỚC RLS), `provision_user` — phải pre-RLS
46. **`sqlx::query_as!` macro yêu cầu `DATABASE_URL`** lúc compile — CI cần `.sqlx` offline cache (T26 sẽ sinh)
47. **`AuthState::new(&config).await?` KHÔNG tồn tại** — chỉ `JwtValidator::new(issuer, client_id)` (không async)
48. **Migration checksum**: nếu edit migration file SAU khi apply, cần bless checksum trong `_sqlx_migrations` (Option C HOTFIX) hoặc revert DB
49. **`rls_middleware` chưa wire thật vào router** (BLOCKER-1) — chỉ set up infrastructure; cần refactor tenant-resolve thành `from_fn` middleware ở batch sau

---

## 9. ⚠️ BLOCKERS & WARNINGS CHO BATCH TIẾP THEO
> QUAN TRỌNG NHẤT — KHÔNG được tóm tắt chung chung

### P0 — Chặn đường găng (phải xử lý trước khi bắt đầu batch sau)
- **[nguồn: HOTFIX BLOCKER-1]** `rls_middleware` ordering gãy: middleware đọc `TenantContext` từ `request.extensions()`, nhưng `TenantContext` là **handler-extractor** (chỉ populate vào extensions KHI handler chạy, SAU middleware) → trong production, RLS middleware **không bao giờ** tìm thấy `TenantContext` → luôn trả 500 `rls-missing-tenant`. Test T14 PASS chỉ vì `build_app_with_rls` self-seed `Extension(tenant_ctx)` manually. → action: refactor tenant-resolve thành `from_fn` middleware chạy TRƯỚC `rls_middleware`, populate `Extension<TenantContext>`, rồi handler extract `Extension<SharedConnection>`. Đây là refactor architecture T13/T14.
- **[nguồn: HOTFIX BLOCKER-2]** Pool dùng role owner → RLS chưa enforce: `DATABASE_URL` connect bằng `gmrag` (superuser/owner). `tenant_members` chỉ `ENABLE ROW LEVEL SECURITY`, chưa `FORCE` → owner bypass RLS. `tenants` có `FORCE` nhưng pool là owner vẫn bypass. → action: chuyển pool sang role `gmrag_app` (đã tạo) + `SET ROLE` để RLS có hiệu lực thật. Liên quan connection string + provisioning.
- **[nguồn: HOTFIX BLOCKER-3]** Chưa có handler tenant-scoped thật sự consume `SharedConnection`. Invariant BUG-2 sẽ thực sự test khi thêm endpoint tenant-scoped đầu tiên (vd: `GET /tenants/{id}/datasets`). Lúc đó PHẢI dùng `SharedConnection` + wire RLS middleware (sau khi fix BLOCKER-1).
- **[nguồn: T17]** `AuthError` codes BREAKING CHANGE từ integer sang kebab-case string — frontend PHẢI update error parsing để handle string codes
- **[nguồn: T12]** `vector` extension KHÔNG có sẵn trong `postgres:16-alpine` image — pgvector sẽ install qua custom image hoặc official pgvector image

### P1 — Lưu ý khi implement
- **[HOTFIX]** `cargo build`/`test`/`clippy` cần `DATABASE_URL` trỏ DB sống (không có `.sqlx` offline cache) — `query_as!` macro trong `users.rs` validate compile-time. Override host `postgres16` → `localhost` khi chạy từ host Windows.
- **[HOTFIX]** Khuyến nghị tạo `.sqlx` offline cache (`cargo sqlx prepare --workspace`) trong task riêng để build không phụ thuộc DB — sẽ làm ở T26.
- **[T18]** `PgPool` + `AuthState` PHẢI add vào request extensions trong main.rs cho `/users/me` work production — hiện tại main.rs add nhưng chỉ cho routes đã mount
- **[T15]** Tests integration `#[sqlx::test]` cần `DATABASE_URL` env var pointing running PostgreSQL — không có thì không tạo được test DB
- **[T13]** Header name hardcoded `"x-tenant-id"` — `Config.tenant_header` field available nhưng chưa wire (integration ở main.rs sau)
- **[T15]** Migration checksum: T15 sửa `identity_and_tenant.sql` (T12) SAU khi apply → checksum lệch. Đã bless bằng Option C trong HOTFIX. Batch sau KHÔNG tự ý edit migration đã apply — dùng migration mới với timestamp cao hơn.
- **[HOTFIX]** `SharedConnection` chưa được consume bởi handler nào — dead_code, đã suppress bằng `#![allow(dead_code)]` crate-level (tạm thời)
- **[T17]** `gmrag_core::Error` maps trực tiếp tới `ApiError::Core` với kebab-case codes sẵn (`"database-error"`, `"config-error"`, etc.)
- **[T18]** Handler `get_me` cross-tenant — KHÔNG đọc `X-Tenant-Id` — trả tất cả tenant user là member

### P2 — Ghi nhớ nhỏ
- Host không resolve `keycloak` → dùng `fake_token` (non-JWT) để có 401; JWT well-formed sẽ fetch JWKS → 503 (giới hạn dev)
- `_sqlx_migrations` table có 3 rows: `20260101000000` (init), `20260617124018` (identity_and_tenant, blessed), `20260617132425` (rls_tenants_table, blessed)
- `pg_hba: cần trust rule` cho dev — KHÔNG ship pg_hba relax này vào init.sql production
- `gmrag-net` 8088→8080 port mapping — test từ host dùng `localhost:8088`
- Tất cả test handler `get_me` dùng `sqlx::query_as!` (compile-time verified) — cần `.sqlx` offline cache nếu CI không có DB

---

## 10. UNBLOCKS
- Batch 2B → unblock: Batch 3 (T19-T26) — Pool infra, migrations, RLS, seed
- Batch 2B → unblock: T19 (refactor AuthUser/TenantContext thành middleware + two-pool design) — giải quyết BLOCKER-1/2
- Batch 2B → unblock: T26 (offline cache + seed) — unblock CI build
- Batch 2B → unblock: Frontend (gọi GET /users/me để display user profile + tenant selector)
