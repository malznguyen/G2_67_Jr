# BATCH 3 SUMMARY — Two-Pool Design, Domain Migrations, RLS, Seed
# Tasks: T19, T20, T21, T22, T23, T24, T25, T26 | Trạng thái: ✅ Hoàn thành
# Test count khi kết thúc batch: 80 passed, 0 failed (workspace tests)

---

## 1. TRẠNG THÁI TỔNG
| Task | Tên ngắn | Commit |
|------|----------|--------|
| T19 | Fix BLOCKER 1 & 2 + workspaces migration (two-pool + middleware refactor) | `9e3c08d` |
| T20 | documents + document_chunks migrations | `4616792` |
| T21 | graph_nodes and graph_edges migrations | `ac3d147` |
| T22 | chat_sessions and chat_messages migrations | `0b8a3a1` |
| T23 | resource_acl and invitations migrations | `bd0979d` |
| T24 | tenant_quotas, usage_events, audit_log, ingest_jobs migrations | `ac3f29a` |
| T25 | RLS policies for 14 domain tables + isolation tests | `ca77ab9` |
| T26 | DB seed script + .sqlx offline cache | `f634840` |

---

## 2. FILES ĐÃ TẠO / SỬA
| File | Task | Hành động | Ghi chú |
|------|------|-----------|---------|
| `backend/crates/core/src/db.rs` | T19 | Sửa | Added `init_app_pool` với `after_connect(SET ROLE gmrag_app)` |
| `backend/crates/core/src/lib.rs` | T19 | Sửa | Re-export `init_app_pool` |
| `backend/crates/api/src/pool.rs` | T19 | Tạo | `AdminPool`/`AppPool` newtypes cho extension disambiguation |
| `backend/crates/api/src/auth/middleware.rs` | T19 | Tạo | `auth_middleware` (from_fn) — JWT validate + provision, populate `Extension<AuthUser>` |
| `backend/crates/api/src/auth/tenant.rs` | T19 | Sửa | `TenantContext` từ extractor → `tenant_middleware` (from_fn), populate `Extension<TenantContext>` |
| `backend/crates/api/src/auth/extractor.rs` | T19 | Sửa | Xóa `FromRequestParts` impl + 6 tests; giữ `AuthUser`/`AuthState` structs + 2 unit tests |
| `backend/crates/api/src/auth/mod.rs` | T19 | Sửa | Thêm `pub mod middleware` |
| `backend/crates/api/src/middleware/rls.rs` | T19 | Sửa | `Extension<PgPool>` → `Extension<AppPool>`; update 3 tests |
| `backend/crates/api/src/routes/users.rs` | T19 | Sửa | `get_me` dùng `Extension<AuthUser>` + `Extension<AdminPool>`; update 2 tests |
| `backend/crates/api/src/main.rs` | T19 | Sửa | Two pools, 3 route groups (public/authed/tenant-scoped), middleware chain wiring |
| `backend/crates/api/tests/pool_role.rs` | T19 | Tạo | 3 integration tests: admin role, app role, RLS enforced |
| `backend/migrations/20260617143508_workspaces.sql` | T19 | Tạo | DDL workspaces + workspace_members |
| `backend/migrations/20260617143700_documents.sql` | T20 | Tạo | DDL documents (owner_id, visibility, share_token, s3_key) + document_chunks (qdrant_point_id UUID) |
| `backend/crates/api/tests/schema_documents.rs` | T20 | Tạo | 2 schema verification tests |
| `backend/migrations/20260617143822_graph_entities.sql` | T21 | Tạo | DDL graph_nodes (kind, label, properties JSONB) + graph_edges (src/dst, kind, weight, UNIQUE) |
| `backend/crates/api/tests/schema_graph.rs` | T21 | Tạo | 2 schema verification tests |
| `backend/migrations/20260617144046_chat.sql` | T22 | Tạo | DDL chat_sessions (workspace_id nullable, model) + chat_messages (role, content, token_count, cascade) |
| `backend/crates/api/tests/schema_chat.rs` | T22 | Tạo | 2 schema verification tests (columns + cascade functional test) |
| `backend/migrations/20260617144756_acl.sql` | T23 | Tạo | DDL resource_acl (polymorphic composite UNIQUE) + invitations (token UUID default, status, expires_at) |
| `backend/crates/api/tests/schema_acl.rs` | T23 | Tạo | 2 schema verification tests |
| `backend/migrations/20260617145246_system_tracking.sql` | T24 | Tạo | DDL tenant_quotas + usage_events + audit_log + ingest_jobs |
| `backend/crates/api/tests/schema_system.rs` | T24 | Tạo | 3 schema verification tests |
| `backend/migrations/20260617145935_rls_apply_all.sql` | T25 | Tạo | ENABLE + FORCE RLS + CREATE POLICY `tenant_id = gmrag_current_tenant()` trên 14 bảng (T19-T24) |
| `backend/crates/api/tests/rls_isolation.rs` | T25 | Sửa | Thêm 8 cross-tenant isolation tests + 1 no-context test cho 14 bảng; diagnostic assertions |
| `infra/postgres/seed.sql` | T26 | Tạo | Mock data: 2 tenants, 3 users, 4 tenant_members, 2 workspaces, 3 documents, 5 chunks, 1 chat session, 2 messages, 2 quotas, 1 audit log, 1 ingest job. Idempotent (ON CONFLICT DO NOTHING) |
| `backend/crates/api/tests/seed_verify.rs` | T26 | Tạo | 2 tests: seed runs clean + idempotent re-run |
| `backend/.sqlx/query-*.json` | T26 | Tạo (2 files) | Offline cache cho `query_as!` macros trong `routes/users.rs` |

---

## 3. PUBLIC API ĐÃ CÓ
> Để agent batch sau biết không implement lại

```rust
// T19 — gmrag-core db
pub async fn init_app_pool(database_url: &str) -> Result<PgPool, sqlx::Error>
// after_connect hook: SET ROLE gmrag_app

// T19 — gmrag-api pool
pub struct AdminPool(pub PgPool);  // superuser, bypass RLS
pub struct AppPool(pub PgPool);    // gmrag_app role, RLS enforced
impl Deref for AdminPool { type Target = PgPool; }
impl Deref for AppPool { type Target = PgPool; }
impl Clone for AdminPool { ... }
impl Clone for AppPool { ... }

// T19 — auth middleware
pub async fn auth_middleware(
    State(state): State<AppState>,
    Extension(auth_state): Extension<AuthState>,
    Extension(admin_pool): Extension<AdminPool>,
    mut request: Request,
    next: Next,
) -> Result<Response, ApiError>
// JWT validate + provision + populate Extension<AuthUser>

pub async fn tenant_middleware(
    Extension(auth_user): Extension<AuthUser>,
    Extension(admin_pool): Extension<AdminPool>,
    Extension(headers): Extension<HeaderMap>,  // or via request
    mut request: Request,
    next: Next,
) -> Result<Response, ApiError>
// Read X-Tenant-Id + membership check (AdminPool) + populate Extension<TenantContext>

// T20-T24 — domain tables (DDL only, no public API code)
// tables: workspaces, workspace_members, documents, document_chunks,
//         graph_nodes, graph_edges, chat_sessions, chat_messages,
//         resource_acl, invitations, tenant_quotas, usage_events,
//         audit_log, ingest_jobs

// T25 — RLS policy uniform: tenant_id = gmrag_current_tenant()

// T26 — seed.sql chạy thủ công (hoặc một lần ở provisioning)
```

---

## 4. MIGRATION STATE
| File migration | Version timestamp | Bảng tạo ra |
|---------------|------------------|-------------|
| `20260101000000_init.sql` | `20260101000000` | (placeholder) |
| `20260617124018_identity_and_tenant.sql` | `20260617124018` | users, tenants, tenant_members, platform_admins, pgcrypto, gmrag_app, gmrag_current_tenant() |
| `20260617132425_rls_tenants_table.sql` | `20260617132425` | RLS trên tenants |
| `20260617143508_workspaces.sql` | `20260617143508` | workspaces, workspace_members |
| `20260617143700_documents.sql` | `20260617143700` | documents, document_chunks |
| `20260617143822_graph_entities.sql` | `20260617143822` | graph_nodes, graph_edges |
| `20260617144046_chat.sql` | `20260617144046` | chat_sessions, chat_messages |
| `20260617144756_acl.sql` | `20260617144756` | resource_acl, invitations |
| `20260617145246_system_tracking.sql` | `20260617145246` | tenant_quotas, usage_events, audit_log, ingest_jobs |
| `20260617145935_rls_apply_all.sql` | `20260617145935` | RLS policy trên 14 bảng mới |
| `infra/postgres/seed.sql` | (not a migration, manual) | 2 tenants + mock data |

RLS đang enforce trên: `tenant_members`, `tenants`, `workspaces`, `workspace_members`, `documents`, `document_chunks`, `graph_nodes`, `graph_edges`, `chat_sessions`, `chat_messages`, `resource_acl`, `invitations`, `tenant_quotas`, `usage_events`, `audit_log`, `ingest_jobs` (16 bảng total)
sqlx offline cache: **có** (2 query JSON files trong `backend/.sqlx/`)

---

## 5. ENV VARS / CONFIG
| Tên biến | Giá trị mẫu | Task thêm |
|----------|-------------|----------|
| `SQLX_OFFLINE` | `true` (tùy chọn, build không cần DATABASE_URL) | T26 |

---

## 6. DEPENDENCIES (Cargo.toml)
| Crate | Version | Thêm vào crate | Task |
|-------|---------|---------------|------|
| (none — only test deps for new test files) | — | — | — |

---

## 7. QUYẾT ĐỊNH KỸ THUẬT CÓ IMPACT CROSS-TASK
> Chỉ giữ quyết định ảnh hưởng tới batch sau, bỏ quyết định internal

- **[T19]** Two-pool design: `init_pool` (superuser `gmrag`) cho migrations + cross-tenant; `init_app_pool` (`SET ROLE gmrag_app`) cho RLS-enforced queries
- **[T19]** Newtype `AdminPool`/`AppPool` bắt buộc vì axum extension map dùng `TypeId` — không thể giữ 2 `Extension<PgPool>` cùng lúc
- **[T19]** Cả `AuthUser` + `TenantContext` thành middleware (`from_fn`): chain `auth_middleware → tenant_middleware → rls_middleware → handler` — handler args đổi sang `Extension<AuthUser>`/`Extension<TenantContext>`/`Extension<SharedConnection>`
- **[T19]** `tenant_middleware` dùng `AdminPool` cho membership check (phải chạy TRƯỚC khi RLS context set — chicken-and-egg)
- **[T19]** `get_me` dùng `AdminPool` (cross-tenant, list ALL memberships) — preserved behavior
- **[T19]** `tenant_scoped` group middleware chain wired (compile + dùng) nhưng chưa có handler tenant-scoped thật (BLOCKER-3 carry) — `#![allow(dead_code)]` giữ
- **[T19]** `healthz` đổi sang `Extension<AdminPool>` — AppState không còn pool (pools là Extension)
- **[T20]** `qdrant_point_id` là UUID — vector embedding lưu ở Qdrant; PG chỉ giữ point reference (UUID) để join metadata với vector
- **[T20]** `document_chunks.tenant_id` denormalized để RLS policy uniform
- **[T20]** `documents.workspace_id` nullable (standalone hoặc workspace)
- **[T20]** `documents.share_token` UUID nullable (visibility = 'shared')
- **[T20]** `documents.status` lifecycle: uploaded → processing → ready → failed
- **[T21]** `graph_nodes.properties` JSONB — metadata tùy biến
- **[T21]** `graph_edges` UNIQUE(src_node_id, dst_node_id, kind) — tránh edge trùng lặp cùng loại
- **[T22]** `chat_messages.role` TEXT (không enum) — 'user'/'assistant'/'system'; validate ở app layer
- **[T22]** `chat_messages.workspace_id` ON DELETE SET NULL (khác `document_chunks` CASCADE) — chat có thể tồn tại độc lập
- **[T22]** `chat_messages.token_count` nullable (không phải message nào cũng có)
- **[T23]** Polymorphic ACL (resource_type + resource_id) cho nhiều loại resource; `resource_id` UUID
- **[T23]** `invitations.token` UUID default `gen_random_uuid()` — không cần application layer generate
- **[T24]** `tenant_quotas` PK = tenant_id (1:1) — FK REFERENCES tenants(id) ON DELETE CASCADE
- **[T24]** `usage_events` append-only — `delta BIGINT` cho phép negative
- **[T24]** `audit_log.actor_id` nullable (system actions)
- **[T24]** `ingest_jobs.document_id` FK CASCADE + `attempts` + `last_error`
- **[T25]** Chỉ thêm policy cho 14 bảng mới — KHÔNG động `tenants`/`tenant_members` (đã có RLS từ T12/T15)
- **[T25]** `FORCE ROW LEVEL SECURITY` trên tất cả 14 bảng — runtime enforcement phụ thuộc `init_app_pool` (T19) downgrades → `gmrag_app`
- **[T26]** Seed UUIDs hex-valid only (a-f, 0-9) — prefix `a1`/`b1`/`c1`/`d1`/`e1`/`f1`/`a2`/`b2`/`c2`
- **[T26]** Seed idempotent (ON CONFLICT DO NOTHING) — chạy nhiều lần OK
- **[T26]** Seed trong 1 transaction — atomic
- **[T26]** Seed chạy as superuser (gmrag) — bypass RLS, insert cho ALL tenants
- **[T26]** `.sqlx` offline cache: `cargo sqlx prepare --workspace` sinh 2 JSON files — `SQLX_OFFLINE=true` build không cần DB

---

## 8. INVARIANTS BẮT BUỘC (tích lũy đến hết batch này)
> Đây là luật bất biến — agent vi phạm là toang
> Phải liệt kê ĐẦY ĐỦ kể cả invariant kế thừa từ batch trước

1. **Tenant identity phải tới từ request context** (qua `TenantContext` middleware), KHÔNG pull từ URL hoặc config tĩnh
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
14. **`gmrag_app` role tồn tại trong DB**
15. **Frontend App Router**
16. **Frontend `output: "standalone"`**
17. **Corepack pin pnpm version**
18. **`.env` (local, gitignored) phải tạo từ `.env.example`**
19. **Mọi env var backend phải có trong `.env.example`**
20. **Compose port mapping**: backend `8088:8080`, frontend `3000:3000`
21. **JWT auth dùng `rustls-tls`**
22. **JWKS cache TTL 300s** default
23. **`AuthState` inject qua `axum::Extension`**
24. **HTTP error mapping AuthError**: 401 cho auth failures, 503 cho JWKS fetch failures
25. **`tenant_members` RLS policy** `tenant_members_isolation` filter `gmrag_current_tenant()`
26. **`tenants` RLS policy** với FORCE ROW LEVEL SECURITY (T15)
27. **Tenant-scoped queries phải set `app.tenant_id` GUC** qua middleware
28. **`AuthUser.claims` field luôn available**
29. **Test RSA keys ở `auth/test_keys/` phải force-add**
30. **`MissingHeader` + `UserNotFound` variants dùng ở T13+**
31. **`with_cache_ttl` available** cho tests custom TTL
32. **Handler parameter order**: `AuthUser` TRƯỚC `TenantContext`
33. **`TENANT_HEADER` = `"x-tenant-id"` (lowercase)**
34. **`SharedConnection` PHẢI dùng bởi tenant-scoped handler** (qua `rls_middleware`)
35. **Middleware ordering LIFO**: `.layer()` gọi sau = chạy TRƯỚC; chain `auth → tenant → rls → handler`
36. **Tenant-scoped handler dùng `Extension<SharedConnection>`**, KHÔNG extract `TenantContext` trực tiếp
37. **RLS policy uniform** `tenant_id = gmrag_current_tenant()` cho mọi bảng có `tenant_id` (denormalized)
38. **Tests dùng `SET LOCAL ROLE gmrag_app`** để PostgreSQL enforce RLS
39. **`FORCE ROW LEVEL SECURITY` bắt buộc** trên mọi bảng có RLS
40. **`provision_user` optional nếu PgPool missing** — production PHẢI có PgPool
41. **Profile changes via `ON CONFLICT (id) DO UPDATE`**
42. **Error envelope PHẢI là** `{ error: { code: <string>, message: <string> } }`
43. **`AuthError` codes là kebab-case strings**
44. **Cross-tenant handler (`get_me`)** dùng `PgPool` (AdminPool) — KHÔNG ép `SharedConnection`
45. **Platform-level extractors/helpers dùng `PgPool` (AdminPool)**: provisioning, membership check pre-RLS
46. **`sqlx::query_as!` macro yêu cầu `DATABASE_URL`** lúc compile — CI dùng `.sqlx` offline cache (T26)
47. **`AuthState::new(&config).await?` KHÔNG tồn tại** — chỉ `JwtValidator::new(issuer, client_id)` (không async)
48. **Migration checksum**: KHÔNG edit migration đã apply — dùng migration mới với timestamp cao hơn
49. **Two-pool design**: `init_pool` (AdminPool, gmrag superuser) CHỈ dùng cho migrations + cross-tenant; `init_app_pool` (AppPool, gmrag_app role) BẮT BUỘC cho tenant-scoped business queries
50. **`AdminPool`/`AppPool` newtype bắt buộc** vì axum extension map dùng `TypeId`
51. **Migrations chạy trên `admin_pool`** vì `gmrag_app` thiếu `CREATE` trên schema
52. **`tenant_middleware` dùng `AdminPool` cho membership check** (pre-RLS) — chicken-and-egg
53. **Vector embeddings lưu ở Qdrant; PostgreSQL chỉ giữ metadata + `qdrant_point_id` UUID reference**
54. **Polymorphic ACL** (resource_type + resource_id, resource_id UUID) — mọi PK trong schema đều UUID
55. **JSONB (không JSON)** cho properties/metadata — support indexing + efficient querying
56. **`chat_messages.workspace_id` ON DELETE SET NULL** (khác CASCADE) — chat tồn tại độc lập
57. **`usage_events` append-only** — không UPDATE/DELETE trong application logic
58. **Mọi bảng có `tenant_id` đều có RLS** với `FORCE ROW LEVEL SECURITY`
59. **`#[sqlx::test]` cache test databases** — khi thêm migration mới, phải DROP `_sqlx_test_%` databases trước khi chạy test
60. **Seed chạy as superuser (gmrag)** — bypass RLS, insert cho ALL tenants
61. **`.sqlx` offline cache cần regenerate** khi thêm/sửa `query!`/`query_as!` macro mới: `cargo sqlx prepare --workspace`
62. **Worker hiện dùng `init_pool` (admin/gmrag) CHỈ cho boot liveness check** — khi T42 implement dual-write, PHẢI switch sang `init_app_pool` + per-job `SET LOCAL app.tenant_id = $job.tenant_id`
63. **`workspace_id` nullable trên documents/chat_sessions** — standalone hoặc workspace

---

## 9. ⚠️ BLOCKERS & WARNINGS CHO BATCH TIẾP THEO
> QUAN TRỌNG NHẤT — KHÔNG được tóm tắt chung chung

### P0 — Chặn đường găng (phải xử lý trước khi bắt đầu batch sau)
- **[nguồn: T19]** `tenant_scoped` route group KHÔNG mount route thật (BLOCKER-3 carry) — middleware chain wired (compile + dùng) nhưng chưa có handler tenant-scoped consume `SharedConnection`. HTTP-level RLS path chưa test end-to-end. → action: tạo handler tenant-scoped đầu tiên (vd: `GET /tenants/{id}/datasets`) consume `Extension<SharedConnection>` + verify RLS
- **[nguồn: T19]** Worker hiện dùng `init_pool` (admin/gmrag) — khi T37+ implement dual-write, PHẢI switch sang `init_app_pool` (gmrag_app role) + per-job `SET LOCAL app.tenant_id = $job.tenant_id`. KHÔNG dùng `admin_pool` cho business queries.
- **[nguồn: T25]** `#[sqlx::test]` cache test databases — khi thêm migration mới (T26 seed hoặc batch sau), PHẢI DROP `_sqlx_test_%` databases trước khi chạy test, hoặc sqlx sẽ dùng DB cũ thiếu migration mới
- **[nguồn: T26]** `.sqlx` offline cache cần regenerate khi thêm/sửa `query!`/`query_as!` macro mới: `cargo sqlx prepare --workspace` (cần DATABASE_URL trỏ tới DB có schema đúng)
- **[nguồn: T26]** Seed data đã insert vào main DB — nếu cần reset, xóa manually hoặc dùng `DATABASE_URL_ADMIN` để truncate

### P1 — Lưu ý khi implement
- **[T19]** `init_app_pool` share với `worker` qua `gmrag_core` — worker hiện dùng `init_pool` (superuser). Khi worker có ingest pipeline, cần `SET LOCAL app.tenant_id` per job hoặc dùng `init_app_pool`
- **[T19]** Handler args đã đổi từ extractor sang `Extension<AuthUser>`/`Extension<TenantContext>`/`Extension<SharedConnection>` — KHÔNG còn `FromRequestParts` impl trên `AuthUser`/`TenantContext` (chỉ structs + 2 unit tests còn lại)
- **[T20]** `documents.status` lifecycle: uploaded → processing → ready → failed — agent xử lý document status updates cần respect
- **[T21]** `graph_edges` UNIQUE(src_node_id, dst_node_id, kind) — nhiều edge khác kind giữa cùng cặp node OK
- **[T22]** Cascade test functional dùng insert→delete→count (không check `pg_constraint.confdeltype` vì sqlx char type map lỗi)
- **[T23]** Polymorphic ACL `resource_type` là TEXT (không enum) — validate ở app layer
- **[T24]** `tenant_quotas` PK = tenant_id — không cần id riêng
- **[T25]** Test DB stale: RED run tạo test DB trước migration → policy missing → tests vẫn fail dù migration applied. Fix: DROP `_sqlx_test_%` databases
- **[T26]** Host Windows cần override `DATABASE_URL` từ `postgres16` → `localhost` (per HOTFIX_BATCH2B note)
- **[T26]** Worker compile cần `SQLX_OFFLINE=true` trên host không có DB sống (pre-existing từ T26)

### P2 — Ghi nhớ nhỏ
- `_sqlx_migrations` table có 7 rows sau Batch 3: `20260101000000`, `20260617124018` (blessed), `20260617132425` (blessed), `20260617143508`, `20260617143700`, `20260617143822`, `20260617144046`, `20260617144756`, `20260617145246`, `20260617145935`
- `vector` extension vẫn KHÔNG có sẵn trong `postgres:16-alpine` — pgvector install qua custom image (chưa thuộc batch này)
- `chat_messages.role` validate ở app layer (không enum constraint DB)
- `usage_events.delta` BIGINT — cho phép negative (vd: xóa document giảm usage)
- `audit_log.metadata` JSONB nullable — system actions không có actor
- `ingest_jobs.attempts` track retry count; `last_error` lưu error gần nhất
- Seed prefix UUIDs: tenants `a1*`, users `b1*`, workspaces `c1*`, documents `d1*`, chunks `e1*`, chat `f1*`/`f2*`, qdrant points `a2*`, audit `b2*`, ingest jobs `c2*`

---

## 10. UNBLOCKS
- Batch 3 → unblock: Batch 4 (T27-T33) — Qdrant vector store
- Batch 3 → unblock: Worker ingest pipeline (T34+) — schema + RLS + seed ready
- Batch 3 → unblock: Frontend dev (mock data có sẵn để test API)
- Batch 3 → unblock: CI build (offline cache unblocked — không cần DB sống)
- Batch 3 → unblock: Tenant-scoped API handlers (RLS + middleware sẵn sàng)
