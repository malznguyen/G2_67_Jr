# HOTFIX PRE-BATCH 5

## Trạng thái: ✅ Hoàn thành

> Hotfix pre-Batch 5. Scope thực tế thu hẹp so với prompt ban đầu sau khi
> verify codebase: **FIX B và FIX C đã được implement từ trước** (commit
> `9e3c08d` / T19 — "fix context/pool blockers and add workspaces migration"),
> xảy ra SAU khi `HOTFIX_BATCH2B.md` được viết (post-T18). Hai blocker mà
> HOTFIX_BATCH2B liệt kê đã resolve; chỉ FIX A (2 clippy errors) cần sửa code
> thật. Doc này ghi nhận trạng thái verified để Batch 5 (T34-T43) có thể tiến
> hành an toàn.

## Checklist
- [x] FIX A: 2 clippy errors đã xóa (pool_role.rs + jwt.rs)
- [x] FIX B: `tenant_middleware` (from_fn) chạy trước `rls_middleware` — **verified already-complete** (commit `9e3c08d`)
- [x] FIX B: `rls_middleware` đọc `Extension<TenantContext>` thay vì handler-extractor — **verified** (`middleware/rls.rs:52`)
- [x] FIX C: `init_app_pool` connect + `SET ROLE gmrag_app` (RLS enforced) — **verified already-complete** (commit `9e3c08d`)
- [x] FIX C: `AppPool` wire vào Extension cho tenant routes — **verified** (`main.rs:117`)
- [x] `cargo clippy --workspace --all-targets -- -D warnings` → ExitCode 0
- [x] `cargo test --workspace` → **87 passed, 0 failed**
- [x] `/health` → 200, `/users/me` fake_token → 401
- [x] Commit: {COMMIT_HASH — điền sau khi commit}

## Files đã tạo / sửa
| File | Hành động | Fix |
|------|-----------|-----|
| `backend/crates/api/tests/pool_role.rs` | Sửa (xóa 1 dòng import) | FIX A |
| `backend/crates/api/src/auth/jwt.rs` | Sửa (`decoding_key` → `_decoding_key`) | FIX A |
| `docs/progress/HOTFIX_PRE_BATCH5.md` | Tạo mới | doc |

### Files KHÔNG sửa (đã đúng từ commit trước)
| File | Lý do không sửa |
|------|-----------------|
| `backend/crates/api/src/middleware/tenant_resolve.rs` | **Không tạo mới** — `tenant_middleware` trong `auth/tenant.rs` đã là `from_fn` middleware, populate `Extension<TenantContext>` (line 139). Tạo file mới sẽ duplicate logic + code prompt đề xuất không compile (gọi `TenantContext::new(tenant_id, auth_user, conn)` — struct thực tế là `pub struct TenantContext(pub Uuid)`, không có constructor `new` hay field connection) |
| `backend/crates/api/src/middleware/rls.rs` | Đã đọc `TenantContext` từ `request.extensions()` (line 52). Không cần đổi |
| `backend/crates/api/src/main.rs` | Đã wire đúng thứ tự LIFO: `rls` → `tenant` → `auth` (chạy auth→tenant→rls). Đã dùng cả `init_pool` (admin) + `init_app_pool` (app) + wire `AdminPool` + `AppPool` extension (line 117-118) |
| `backend/crates/core/src/db.rs` | `init_app_pool` đã có `after_connect: SET ROLE gmrag_app` (line 61-66) → RLS enforced |
| `backend/crates/core/src/config.rs` | Không thêm `database_url_app` — approach `SET ROLE` dùng cùng `DATABASE_URL`, không cần URL riêng |
| `.env.example` / `.env` | Không thêm `DATABASE_URL_APP` — xem rationale ở FIX C |

## Middleware ordering sau fix (verified)
```
Request
  → Extension(AppPool) + Extension(AdminPool) + Extension(AuthState)  [wire ở main.rs:117-119]
  → auth_middleware (from_fn)                                         [auth/middleware.rs:40]
      → validate JWT → provision user (AdminPool) → populate Extension<AuthUser>
  → tenant_middleware (from_fn)                                       [auth/tenant.rs:49]
      → đọc X-Tenant-Id + check membership (AdminPool, pre-RLS)
      → populate Extension<TenantContext>
  → rls_middleware (from_fn)                                          [middleware/rls.rs:50]
      → đọc Extension<TenantContext> + Extension<AppPool>
      → acquire connection (gmrag_app role) + BEGIN + SET LOCAL app.tenant_id
      → populate Extension<SharedConnection>
  → Handler                                                           [extract Extension<SharedConnection>]
      → COMMIT ở cuối middleware
```

Axum layer stack là LIFO: `.layer()` gọi sau = chạy TRƯỚC. `main.rs:108-111`:
```rust
let tenant_scoped: Router<AppState> = Router::new()
    .layer(axum::middleware::from_fn(rls_middleware))        // chạy SAU (layer trong cùng)
    .layer(axum::middleware::from_fn(tenant_middleware))     // chạy GIỮA
    .layer(axum::middleware::from_fn(auth_middleware));      // chạy TRƯỚC (layer ngoài cùng)
```

## Pool role sau fix (verified)
| Pool | Role | Dùng cho | Cách enforce |
|------|------|---------|--------------|
| `admin_pool` (`init_pool`, `DATABASE_URL`) | `gmrag` (superuser, session_user) | migrations, auth provision, tenant membership check, `/users/me` (cross-tenant) | Không enforce RLS — intentional |
| `app_pool` (`init_app_pool`, cùng `DATABASE_URL`) | `gmrag_app` (current_role sau SET ROLE) | business queries trong `rls_middleware`, RLS enforced | `after_connect` hook: `SET ROLE gmrag_app` (db.rs:61-66) |

### Tại sao KHÔNG dùng `DATABASE_URL_APP` riêng
1. **Migration `20260617124018:17`** tạo `gmrag_app NOLOGIN` (không password). `infra/postgres/init.sql:17` tạo `gmrag_app LOGIN PASSWORD 'gmrag_app_change_me'` — **xung đột**. Trong `sqlx::test` DB (chỉ chạy migration, skip init.sql), `gmrag_app` là NOLOGIN → **không connect trực tiếp được** → 13 test `rls_isolation.rs` + 3 test `pool_role.rs` sẽ gãy.
2. Approach `SET ROLE` hiện tại work cho cả Docker production (gmrag_app có LOGIN) lẫn `sqlx::test` (gmrag_app NOLOGIN nhưng SET ROLE từ session gmrag superuser vẫn được).
3. Đổi migration thành `LOGIN PASSWORD` sẽ **lệch checksum migration** (đúng vấn đề HOTFIX_BATCH2B FIXED-3 vừa bless bằng Option C). Vi phạm tinh thần "không chạm migration".
4. `pool_role.rs` test đã verify `current_role == gmrag_app` + RLS enforced trên `tenants` table → `SET ROLE` approach đã được test xanh.

### Verify DB trực tiếp
```sql
-- gmrag_app là LOGIN role trong Docker (qua init.sql)
SELECT rolname, rolcanlogin FROM pg_roles WHERE rolname='gmrag_app';
--  rolname   | rolcanlogin
--  gmrag_app | t

-- SET ROLE (what init_app_pool's after_connect does)
SET ROLE gmrag_app; SELECT current_role, current_user, session_user;
--  current_role | current_user | session_user
--  gmrag_app    | gmrag_app    | gmrag
```
`session_user` vẫn là `gmrag` (superuser) nhưng `current_role` = `gmrag_app` → RLS policy áp dụng trên `current_role`, không phải `session_user`. Owner bypass RLS chỉ khi `current_role` là owner; sau `SET ROLE gmrag_app`, owner bypass không còn hiệu lực.

## Test results
```
$env:SQLX_OFFLINE="true"
$env:DATABASE_URL="postgres://gmrag:***@localhost:5432/gmrag"  # override postgres16→localhost cho host
cargo test --workspace

test result: ok. 42 passed; 0 failed; 0 ignored   (gmrag-api unittests: jwt/tenant/middleware/rls/main/extractor/provision)
test result: ok. 13 passed; 0 failed; 0 ignored   (tests/rls_isolation.rs — cross-tenant RLS, 13 test)
test result: ok.  2 passed; 0 failed; 0 ignored   (tests/pool_role.rs — gmrag_app role + RLS enforcement)
test result: ok.  3 passed; 0 failed; 0 ignored   (tests/schema_acl.rs)
test result: ok.  2 passed; 0 failed; 0 ignored   (tests/schema_chat.rs)
test result: ok.  2 passed; 0 failed; 0 ignored   (tests/schema_documents.rs)
test result: ok.  2 passed; 0 failed; 0 ignored   (tests/schema_graph.rs)
test result: ok.  3 passed; 0 failed; 0 ignored   (tests/schema_system.rs)
test result: ok.  2 passed; 0 failed; 0 ignored   (tests/seed_verify.rs)
test result: ok. 16 passed; 0 failed; 0 ignored   (gmrag-core unittests: config/db/error/qdrant)
test result: ok.  0 passed; 0 failed; 0 ignored   (gmrag-worker — chưa có test)
test result: ok.  0 passed; 0 failed; 0 ignored   (doc-tests gmrag_core)
Tổng: 87 passed; 0 failed
```

> Test integration (`#[sqlx::test]`) cần DB sống reachable từ host. Container
> `gmrag-postgres16` expose `localhost:5432`; `.env` dùng host `postgres16`
> (chỉ resolve trong Docker network) → override `DATABASE_URL` thành
> `localhost` khi chạy từ host Windows (per note HOTFIX_BATCH2B).

## Clippy results
```
$env:SQLX_OFFLINE="true"
cargo clippy --workspace --all-targets -- -D warnings

    Checking gmrag-api v0.1.0
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.00s
    ExitCode: 0
```
Trước FIX A có 2 error:
- `crates/api/tests/pool_role.rs:12` — `unused import: sqlx::Executor`
- `crates/api/src/auth/jwt.rs:294` — `unused variable: decoding_key`

Sau FIX A: cả 2 đã clear, ExitCode 0.

## Boot verify
```
gmrag-api starting service=gmrag-api bind=0.0.0.0:8080
admin postgres pool ready
app postgres pool ready (role=gmrag_app)        ← FIX C verified at runtime
database migrations applied
auth state ready issuer=http://localhost:8080/realms/gmrag
gmrag-api listening addr=0.0.0.0:8080

HEALTH: 200                                      ← /health
USERS_ME_STATUS: 401                             ← /users/me với Bearer fake_token
```
> `fake_token` không phải JWT well-formed → `decode_header` fail trong
> `JwtValidator::validate` (jwt.rs:115) → `AuthError::InvalidToken` → 401.
> Trước hotfix (theo HOTFIX_BATCH2B), request này trả 500/503 do
> `Extension<PgPool>` / `AuthState` missing — đã fix ở commit T16/T19.

## FIX A chi tiết

### A1 — `pool_role.rs:12` unused import
File `backend/crates/api/tests/pool_role.rs` line 12: `use sqlx::Executor;`
File này dùng `sqlx::Executor::execute(...)` fully-qualified UFCS call (line 76)
→ không cần `use` import. Xóa hẳn dòng import.

### A2 — `jwt.rs:294` unused variable
File `backend/crates/api/src/auth/jwt.rs` line 294 trong `make_validator_with_key()`:
`let decoding_key = DecodingKey::from_rsa_pem(TEST_PEM_PUB).unwrap();`
Variable không được đọc (key được inject per-test qua `inject_key` async).
Prefix `_decoding_key` để giữ PEM-parse liveness check (verify test key parseable)
mà không trigger `unused_variables` warning.

## Blocker còn lại
- [x] `.sqlx` offline cache **đã generate** (2 query JSON trong `backend/.sqlx/`).
      Prompt ghi "chưa generate" — stale, đã có từ commit trước. CI có thể dùng
      `SQLX_OFFLINE=true` để build không cần DB.
- [ ] Worker (Batch 5): phải dùng `app_pool` (hoặc `SET ROLE gmrag_app` +
      `SET LOCAL app.tenant_id` cho từng job). KHÔNG dùng `admin_pool` cho
      business queries. Xem invariant dưới.

## Invariant bổ sung cho mọi batch sau
> Pool rule:
> - `admin_pool` (`init_pool`, role `gmrag` superuser): CHỈ dùng cho migrations
>   + platform-level provision (user upsert, tenant membership check pre-RLS)
>   + cross-tenant endpoint (`/users/me`)
> - `app_pool` (`init_app_pool`, `SET ROLE gmrag_app`): BẮT BUỘC cho mọi
>   business handler tenant-scoped. RLS enforced.
> - Worker (Batch 5): phải dùng `app_pool` + `SET LOCAL app.tenant_id` cho từng
>   job. KHÔNG dùng `admin_pool` cho business queries.
>
> Middleware rule (đã enforce từ T19):
> - Thứ tự chạy: `auth_middleware` → `tenant_middleware` → `rls_middleware` → handler
> - `TenantContext` là `from_fn` middleware (không phải handler-extractor)
> - Handler tenant-scoped extract `Extension<SharedConnection>`, KHÔNG extract
>   `TenantContext` trực tiếp
