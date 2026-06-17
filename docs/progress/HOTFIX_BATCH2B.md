# HOTFIX — Post Batch 2B

## Trạng thái: ✅ Hoàn thành

> Hotfix vá 3 broken state phát hiện sau review Batch 2B (T13-T18).
> Scope nghiêm ngặt: chỉ sửa 2 file source + bless checksum DB.
> KHÔNG chạm `routes/`, `auth/`, `middleware/`, `migrations/`.

## Checklist
- [x] BUG-1: `main.rs` đã wire `Extension(pool)` + `Extension(auth_state)`
  - `AuthState` khởi tạo từ `cfg.oidc` (`JwtValidator::new(issuer, client_id)`) — JWKS lazy-fetch
  - 2 layer thêm vào router tại `main.rs:76-77`
- [x] BUG-2: Audit handlers tenant-scoped — **0 handler cần sửa**
  - Số handlers đã sửa: **0**
  - Hit duy nhất dùng `Extension<PgPool>` trong handler thật: `routes/users.rs:32` (`get_me`) — cross-tenant "list my memberships", KHÔNG phải tenant-scoped → giữ `PgPool` (xem bảng audit)
- [x] BUG-3: Migration checksum fix — **Option C** (bless checksum trong `_sqlx_migrations`)
  - Bless `20260617124018` (identity_and_tenant) + `20260617132425` (rls_tenants_table)
  - Cả 2 migration đều lệch checksum do commit `350eda4` (T15) sửa file T12 sau khi apply
- [x] `cargo build --workspace` PASS sạch (ExitCode 0)
- [x] `cargo test --workspace` PASS — **51 test, 0 failed** (37 api + 5 RLS isolation + 9 core)
- [x] `cargo clippy --workspace -- -D warnings` PASS sạch (ExitCode 0)
- [x] `/health` trả **200** khi boot thật
- [x] `/users/me` trả **401** với `Bearer fake_token` (KHÔNG phải 500, KHÔNG phải 503)
- [x] `sqlx migrate info` — 3 Applied, 0 Pending, 0 `(different checksum)`
- [x] Commit: `4a8c17c`

## FIX 1 — Wiring `PgPool` + `AuthState` vào `main.rs`

### Vấn đề
`main.rs` build router chỉ dùng `State<AppState>` + `.with_state(state)`. Không có `.layer(Extension(...))`. `AuthUser` extractor (`auth/extractor.rs:60-65`) đọc `AuthState` từ `parts.extensions` → production luôn fail. Đồng thời `get_me` extract `Extension<PgPool>` cũng fail.

### Sửa
File: `backend/crates/api/src/main.rs`

1. **Imports** (dòng 20-24): thêm `Extension` vào `axum` use, thêm 2 use cho auth:
   ```rust
   use auth::extractor::AuthState;
   use auth::jwt::JwtValidator;
   use axum::{Extension, Router, ...};
   ```
2. **Khởi tạo `AuthState`** sau `sqlx::migrate!().run()`, trước router (dòng 65-69):
   ```rust
   let jwt_validator = JwtValidator::new(cfg.oidc.issuer.clone(), cfg.oidc.client_id.clone());
   let auth_state = AuthState { jwt_validator };
   ```
   > Lý do không có `AuthState::new(&config).await?` như pseudo-code prompt: struct `AuthState` (extractor.rs:35) chỉ có field `pub jwt_validator`, không có constructor. `JwtValidator::new` (jwt.rs:77) không async → build trực tiếp, không `.await`. JWKS fetch lazy ở lần validate đầu tiên (`get_key`).
3. **Wire 2 layer** (dòng 76-77):
   ```rust
   .layer(Extension(pool.clone()))   // PgPool cho AuthUser provisioning + get_me
   .layer(Extension(auth_state))     // JwtValidator cho AuthUser extractor
   ```
4. **Inline `AppState`** vào `.with_state(...)` (dòng 78-81), bỏ `let state = AppState {...};` rời rạc.
5. **`#![allow(dead_code)]`** crate-level (dòng 13): suppress dead_code cho `middleware::rls::*` (RLS middleware chưa wire — xem Blocker) và reserved `ApiError` variants. Tạm thời, sẽ gỡ khi wire RLS middleware ở batch sau.

### Test impact
Test `health_returns_200` (main.rs) build router riêng chỉ `/health` + `with_state` → không đụng Extension → vẫn xanh. **Không sửa test nào.**

## FIX 2 — Audit handlers dùng `PgPool`

### Lệnh audit
```powershell
rg -n "Extension\(pool\)|Extension<PgPool>|pool: PgPool" backend/crates/api/src/routes backend/crates/api/src/auth
```

### Bảng audit đầy đủ (toàn api crate)
| File | Handler/Item | Dùng PgPool? | Tenant-scoped? | Xử lý |
|------|--------------|--------------|----------------|-------|
| `routes/users.rs:32` | `get_me` | YES | **NO** (cross-tenant, list ALL memberships) | Giữ PgPool — justified |
| `auth/extractor.rs:80` | `AuthUser` (extractor) | YES | NO (platform-level, auto-provision `users` trước RLS) | Hợp lệ — giữ |
| `auth/tenant.rs:56` | `TenantContext` (extractor) | YES | NO (membership check phải chạy TRƯỚC RLS middleware — chicken-and-egg) | Hợp lệ — giữ |
| `auth/provision.rs:16` | `provision_user` (helper) | YES | NO (platform-level upsert `users`) | Hợp lệ — giữ |
| `middleware/rls.rs:62` | `rls_middleware` | YES | — (infra: tạo `SharedConnection` từ pool) | Hợp lệ — giữ |

### Kết luận
**Không có handler tenant-scoped nào vi phạm invariant.** `get_me` là cross-tenant (không đọc `X-Tenant-Id`, trả về tất cả tenant mà user là member) → được phép dùng `PgPool`. Ép sang `SharedConnection` sẽ scope về 1 tenant = thay đổi behavior nghiệp vụ (vi phạm quy tắc "KHÔNG sửa logic nghiệp vụ").

## FIX 3 — Sửa lệch checksum migration (Option C: bless)

### Vấn đề
- Migration `20260617124018_identity_and_tenant.sql`: DB lưu checksum `8b288f2f...e2b`, file hiện tại sha384 `c1b0ace5...a1fa` → lệch.
- Migration `20260617132425_rls_tenants_table.sql`: DB lưu `254d2ac3...6406`, file hiện tại `0afee2c6...a29e` → lệch.
- Nguyên nhân: commit `350eda4` (T15) đã sửa 2 file migration SAU khi apply (thêm `pgcrypto`, role `gmrag_app`, function `gmrag_current_tenant()` vào T12; chỉnh T15). `sqlx migrate run` tiếp theo báo `was previously applied but has been modified`.

### Tại sao Option A không khả thi
`sqlx migrate revert` chỉ revert migration **cuối cùng** (`20260617132425`), không revert được `20260617124018` (nằm giữa). Các file `.sql` đơn thuần không có down-migration → revert cũng lỗi. Option B (nuke `gmrag-pgdata`) khả thi nhưng mất dev data không cần thiết.

### Sửa — Option C (bless checksum)
An toàn vì phần SQL thêm vào T15 đều **idempotent**: `CREATE EXTENSION IF NOT EXISTS pgcrypto`, `DO $$ IF NOT EXISTS role`, `CREATE OR REPLACE FUNCTION`. Schema thực tế không đổi → bless không gây lệch ngữ nghĩa.

```sql
UPDATE _sqlx_migrations SET checksum = decode('<sha384 mới của file trên disk>','hex')
WHERE version = '20260617124018';
-- và tương tự cho 20260617132425
```

sha384 tính trực tiếp từ bytes file trên disk bằng `[System.Security.Cryptography.SHA384]` để khớp 100% với gì `sqlx migrate run` sẽ tính.

## Migration status sau fix

```
$ sqlx migrate info --database-url $DATABASE_URL --source backend/migrations
20260101000000/installed init
20260617124018/installed identity and tenant
20260617132425/installed rls tenants table
```

Checksum sau bless (`SELECT version, encode(checksum,'hex') FROM _sqlx_migrations ORDER BY version`):
```
20260101000000 | 912816a945563b58bdbc32c254446fe67571f6d5d8064f2c34693766f59a095846591f3c714e3effe47dfe26a9f2e972
20260617124018 | c1b0ace550e3f09e365d99deca42c47600499a18c1620604f9b699111e34892a528b635fef6d2fe13adda51e78dca1fa
20260617132425 | 0afee2c6c563e4998f5a2ae70d764be39e2aca791d1e35478bde8587e32d2390f0fcfbc7cf50e1da64702debd465a29e
```

`sqlx migrate run` sau bless → no-op, ExitCode 0, không còn `was previously applied but has been modified`.

## VERIFY tổng — output thật

### `cargo build --workspace`
```
Compiling gmrag-worker v0.1.0
Compiling gmrag-api v0.1.0
Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.24s
ExitCode: 0
```

### `cargo test --workspace`
```
test result: ok. 37 passed; 0 failed; 0 ignored   (gmrag-api unittests)
test result: ok.  5 passed; 0 failed; 0 ignored   (tests/rls_isolation.rs — cross-tenant)
test result: ok.  9 passed; 0 failed; 0 ignored   (gmrag-core)
Tổng: 51 passed; 0 failed
```

### `cargo clippy --workspace -- -D warnings`
```
Checking gmrag-api v0.1.0
Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.80s
ExitCode: 0
```

### Boot thật + probe
```
gmrag-api starting service=gmrag-api bind=127.0.0.1:8091
postgres pool ready
database migrations applied
auth state ready issuer=http://keycloak:8080/realms/gmrag
gmrag-api listening addr=127.0.0.1:8091
HEALTH: 200
USERS_ME (fake_token): 401
```
> `fake_token` không phải JWT well-formed → `decode_header` fail trong `JwtValidator::validate` (jwt.rs:115) → `AuthError::InvalidToken` → **401**. Trước hotfix, request này rơi vào `Extension<PgPool>` missing hoặc `AuthState` missing → **500**/503.

## Invariant mới cần nhớ cho mọi batch sau

> ⚠️ BẮT BUỘC: Mọi handler thao tác dữ liệu **tenant-scoped** PHẢI dùng `SharedConnection`
> (do `rls_middleware` set `SET LOCAL app.tenant_id`), KHÔNG dùng `PgPool` trực tiếp.
>
> **Ngoại lệ hợp lệ** (được phép dùng `PgPool`):
> - Handler **cross-tenant** (vd: `get_me` — list ALL memberships, không đọc `X-Tenant-Id`)
> - Extractor/helper **platform-level** (`AuthUser` provisioning, `TenantContext` membership check, `provision_user`) — phải chạy TRƯỚC khi RLS context được set
> - `rls_middleware` (infra: tạo `SharedConnection` từ pool)

## Blocker còn lại (chuyển sang batch sau)

### ⚠️ BLOCKER-1: RLS middleware ordering gãy
`rls_middleware` (`middleware/rls.rs:50`) đọc `TenantContext` từ `request.extensions()`, nhưng `TenantContext` là **handler-extractor** (chỉ populate vào extensions KHI handler chạy, tức SAU middleware). Do đó trong production, RLS middleware sẽ **không bao giờ** tìm thấy `TenantContext` → luôn trả 500 `rls-missing-tenant`.

Test T14 hiện đang PASS chỉ vì `build_app_with_rls` **self-seed** `Extension(tenant_ctx)` bằng tay (`rls.rs:172`) — không phản ánh production flow.

→ Cần refactor tenant-resolve thành `from_fn` middleware chạy **trước** `rls_middleware`, populate `Extension<TenantContext>`, rồi handler extract `Extension<SharedConnection>`. Đây là refactor architecture T13/T14, không thuộc scope hotfix.

Hệ quả: hotfix này **KHÔNG wire `rls_middleware`** vào router (sẽ fail runtime). `SharedConnection` hiện chưa được consume bởi handler nào — dead_code, đã suppress bằng `#![allow(dead_code)]` crate-level (tạm thời).

### ⚠️ BLOCKER-2: Pool dùng role owner → RLS chưa enforce
`DATABASE_URL` trong `.env` connect bằng role `gmrag` (superuser/owner). `tenant_members` chỉ `ENABLE ROW LEVEL SECURITY`, chưa `FORCE` → owner bypass RLS. `tenants` có `FORCE` nhưng pool là owner nên vẫn bypass.

→ Cần chuyển pool sang role `gmrag_app` (đã tạo trong init.sql + migration) + `SET ROLE` để RLS có hiệu lực thật. Liên quan đến connection string + provisioning, không thuộc scope hotfix.

### ⚠️ BLOCKER-3: Chưa có handler tenant-scoped thật sự
`SharedConnection` chưa được consume bởi handler nào (chỉ có `get_me` cross-tenant). Invariant BUG-2 sẽ thực sự được test khi thêm endpoint tenant-scoped đầu tiên (vd: `GET /tenants/{id}/datasets`). Lúc đó phải dùng `SharedConnection` + wire RLS middleware (sau khi fix BLOCKER-1).

### Ghi chú môi trường dev
- `cargo build`/`test`/`clippy` cần `DATABASE_URL` trỏ tới DB sống (không có `.sqlx` offline cache) — `query_as!` macro trong `users.rs` validate compile-time. Override host `postgres16` → `localhost` khi chạy từ host Windows.
- Host không resolve `keycloak` → dùng `fake_token` (non-JWT) để có 401. JWT well-formed sẽ fetch JWKS → 503 (giới hạn dev, không phải bug).
- Khuyến nghị tạo `.sqlx` offline cache (`cargo sqlx prepare --workspace`) trong task riêng để build không phụ thuộc DB.

## Files thay đổi trong hotfix này
| File | Loại | Số dòng |
|------|------|---------|
| `backend/crates/api/src/main.rs` | edit | +12 / -5 |
| `docs/progress/HOTFIX_BATCH2B.md` | new | (file này) |

**KHÔNG chạm**: `routes/`, `auth/`, `middleware/`, `migrations/` (Option C chỉ update DB, không edit file SQL).
