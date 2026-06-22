# AUDIT PRE-FRONTEND — GMRAG2

**Ngày audit:** 2026-06-22
**HEAD lúc audit:** `5c3ae8a` — `docs: rebac batch progress reports, handoff, and PM assets`
**Git tree:** clean (`nothing to commit, working tree clean`)

## Trạng thái: ✅ Đã fix xong — xem section "UPDATE — Sau khi implement fixes" ở cuối file

> Trạng thái ban đầu lúc audit (read-only): ⚠️ Cần thêm fix (chưa verify cargo, xlsx chưa sync). Đã resolve — xem UPDATE ở cuối.

> **Premise correction quan trọng:** Prompt + `handoff.md` cũ nói HEAD=`5517cb6`, ReBAC T64-T67/T83-T85 "chưa commit". **Sai.** HEAD thực tế là `5c3ae8a` (7 commit phía sau), toàn bộ ReBAC + T61-T63 + T68-T69 + T52-T54 + T84 **đã commit**, tree sạch. `handoff.md` là một trong các artifact stale.

---

## A. GIT STATE

- **HEAD commit:** `5c3ae8a` — `docs: rebac batch progress reports, handoff, and PM assets`
- **Uncommitted changes:** NONE
- **Untracked files:** NONE (chỉ item bị gitignore)
- **Cần commit ngay:** NO

### Commit → task mapping (thực tế)
| Commit | Tasks |
|--------|-------|
| `548ede1` | T61 + T62 + T63 + T64 + T65 + T66 + T67 + T83 + T85 (squashed: "feat(T61): sse chat endpoint with rebac integration") |
| `12afb82` | T68 (BYOK settings CRUD) |
| `b375b20` | T69 (metering endpoints) |
| `66aea82` | T52-T54 (tenant + member routes) |
| `b6b7106` | T84 (FE ACL components) |
| `5c3ae8a` | docs: progress reports + handoff + xlsx + 5068.pdf |

---

## B. TASK STATUS (xlsx vs code reality)

| Task | xlsx status | Code thực tế | Conflict? |
|------|-------------|--------------|-----------|
| T61 | Chưa bắt đầu (0%) | ✅ `routes/chat.rs` committed `548ede1`, doc ✅ | YES |
| T62 | Chưa bắt đầu (0%) | ✅ `list/create/delete_session` in chat.rs, doc ✅ | YES |
| T63 | Chưa bắt đầu (0%) | ✅ `routes/graph.rs` committed, doc ✅ | YES |
| T64 | Chưa bắt đầu (0%) | ✅ migration `20260622000000_rebac...sql`, doc ✅ | YES |
| T65 | Chưa bắt đầu (0%) | ✅ `rbac/model.rs`, doc ✅ | YES |
| T66 | Chưa bắt đầu (0%) | ✅ `rbac/check.rs`, doc ✅ | YES |
| T67 | Chưa bắt đầu (0%) | ✅ `routes/acl.rs`, doc ✅ | YES |
| T68 | Chưa bắt đầu (0%) | ✅ `routes/settings.rs` committed `12afb82`, doc ✅ | YES |
| T69 | Chưa bắt đầu (0%) | ✅ `routes/metering.rs` committed `b375b20`, doc ✅ | YES |
| T83 | Chưa bắt đầu (0%) | ✅ `documents.rs` check_relation integration, doc ✅ | YES |
| T84 | Chưa bắt đầu (0%) | ✅ `frontend/lib/acl.ts` + `AclShareDialog.tsx` committed `b6b7106`, doc ✅ | YES |
| T85 | Chưa bắt đầu (0%) | ✅ `tests/rebac_e2e.rs`, doc ✅ | YES |

**Mọi task T61-T85 đều "Chưa bắt đầu" trong xlsx nhưng ĐÃ DONE & COMMITTED trong code.** Đây là issue sync lớn nhất.

---

## C. ISSUES TÌM THẤY

### C1 — RLS/Pool violations (CRITICAL) → NONE FOUND
- Mọi tenant-scoped handler extract `Extension<SharedConnection>` (33 usages, 10 file route).
- `AdminPool` chỉ dùng ở `tenants.rs:52,76` (GET/POST `/tenants` — pre-tenant/cross-tenant, có doc) và `users.rs:36` (`GET /users/me` — cross-tenant, có doc). **Hợp lệ.**
- `chat.rs:213` dùng `AppPool` — đường persist post-stream SSE. `persist_chat_completion` (chat.rs:420-421) chạy `SET LOCAL ROLE gmrag_app` + `SET LOCAL app.tenant_id` → RLS enforced. Khớp pattern `rls.rs::with_rls_connection`. **Hợp lệ theo design.**

### C2 — ReBAC integration gaps (HIGH) → NONE FOUND
- `check_relation` dùng ở: `documents.rs` (preview/delete), `chat.rs` (delete owner-check, viewer-check), `graph.rs` (workspace member), `acl.rs` (owner/viewer).
- `list_documents` và `list_sessions` dùng predicate SQL biên dịch tay (không gọi `check_relation` per-row) — **design decision có doc** (tránh N+1, note T83/T62). Predicate đồng bộ `model.rs` viewer rewrite.
- Không còn route dùng legacy inline ACL mà phải gọi `check_relation`.

### C3 — BYOK crypto debt (HIGH) → RESOLVED
- `core/src/crypto.rs` tồn tại: `encrypt_with_aad`/`decrypt_with_aad` (AES-256-GCM, AAD=tenant_id), 9 unit test.
- `worker/embedding.rs:579` và `worker/graph.rs:371` đều gọi `gmrag_core::crypto::decrypt_with_aad`. Fallback plaintext `api_key` chỉ khi encrypted columns NULL (backwards compat). Không silent fallback khi `enc_key` None.

### C4 — SQLX cache gaps (MEDIUM) → NONE FOUND
- Workspace-wide `sqlx::query!`/`query_as!`/`query_scalar!` macro: **chính xác 2** (cả 2 ở `users.rs:38,48`).
- `.sqlx/` cache: **chính xác 2 file**. Match.
- T61.md/T68.md/T69.md claim "`.sqlx` modified by `cargo sqlx prepare`" — **không chính xác** (route đó dùng runtime `sqlx::query`, không phải macro; prepare là no-op theo TECH_DEBT doc). **Doc inconsistency, không có code issue.**

### C5 — Uncommitted work (MEDIUM) → NONE
- Git tree clean. Toàn bộ code đã commit.
- `handoff.md` **stale**: tham chiếu HEAD `5517cb6`, nói ReBAC "chưa commit", nói T61-T63/T68-T69 "chưa làm" — đều sai tính từ `5c3ae8a`.

### C6 — Test failures (CRITICAL if present) → UNVERIFIED
- Chưa chạy `cargo test` (audit thực hiện trong plan mode read-only).
- **Known issue** (T52_T54.md:47): 16 test `auth::*`/`routes::users` có thể panic `overflow when subtracting duration from instant` trong `JwtValidator::new` (`auth/jwt.rs`) khi máy vừa khởi động (monotonic clock thấp). Cần fix → checked/saturating subtraction.
- Progress docs claim mọi suite pass (T85: 90 lib + 32 integration = 0 failed; TECH_DEBT: 209 total). Phải re-verify ở HEAD hiện tại.

### C7 — Clippy errors (HIGH) → UNVERIFIED
- Chưa chạy `cargo clippy` (plan mode).
- Docs claim `clippy --workspace --all-targets -- -D warnings` → ExitCode 0. Phải re-verify (T68/T69/T84 thêm code sau lần clippy cuối được doc trong T85).

### C8 — Misc inconsistencies
1. **`handoff.md` stale** — sai HEAD, sai commit state, sai task status. Cần rewrite/archive.
2. **xlsx T61-T85 "Chưa bắt đầu"** dù tất cả đã done — **primary sync issue**.
3. **T61.md/T68.md/T69.md** ".sqlx modified" claim sai (no-op).
4. **`chat.rs:421`** `format!("SET LOCAL app.tenant_id = '{tenant_id}'")` — string interpolation vào SQL. **Safe** (Uuid type-safe, không inject quote được) và **consistent** với `rls.rs:60,131`. Code smell priority thấp, codebase-wide pattern.
5. Không có `docs/~$GMRAG2_Project_Management.xlsx` lock file (good — handoff cũ nói untracked).

---

## D. PLAN THỰC THI (theo priority)

### D1 — P0: Fix trước khi làm Frontend (blocking)
1. **Run `cargo test --workspace`** (SQLX_OFFLINE=true) — verify C6. Nếu `JwtValidator` monotonic-clock panic tái diễn → fix `checked_sub`/`saturating_sub` trong `auth/jwt.rs`.
2. **Run `cargo clippy --workspace --all-targets -- -D warnings`** — verify C7. Fix bất kỳ error.
3. **Sync xlsx** T61-T69, T83-T85 → "Hoàn thành" / 100% / cột TDD "Hoàn tất".

### D2 — P1: Fix trong quá trình (non-blocking nhưng quan trọng)
1. **Update `handoff.md`** — mark superseded/archive, hoặc rewrite phản ánh HEAD `5c3ae8a` + tất cả đã commit.
2. **Fix T61.md/T68.md/T69.md** — sửa claim ".sqlx modified" thành "no-op (runtime queries)".
3. **Write `docs/progress/AUDIT_PRE_FRONTEND.md`** — doc bắt buộc (file này).

### D3 — P2: Tech debt, fix sau MVP
1. `chat.rs:421` + `rls.rs:60,131` — thay `format!` SQL interpolation bằng bind parameter (`SET LOCAL app.tenant_id = $1` qua `sqlx::query`). Codebase-wide, risk thấp nhưng đáng chuẩn hoá.
2. `AclShareDialog` chưa mount — mount khi T70+ FE baseline land.
3. Graph GET chưa paginate (T63 note) — lazy-load ở FE.
4. Audit log read chưa filter/pagination (T69 note).

---

## E. COMMIT ORDER ĐỀ XUẤT
Code tree **đã clean**, chỉ cần commit doc/xlsx/fix:

1. **Commit A** — `fix(audit): JwtValidator monotonic clock + clippy` *(chỉ nếu D1.1/D1.2 surface issue)*
2. **Commit B** — `chore(pm): sync xlsx status T61-T85 to Hoàn thành`
3. **Commit C** — `docs(audit): refresh handoff, fix .sqlx claims, add AUDIT_PRE_FRONTEND.md`
4. **Commit D** — `chore: sync sqlx offline cache` *(chỉ nếu `cargo sqlx prepare` thay đổi gì — dự kiến no-op)*

---

## F. SẴN SÀNG FRONTEND?

- [x] Git tree sạch (nothing to commit)
- [x] `cargo test --workspace` PASS — 326 passed, 0 failed (xem UPDATE)
- [x] `cargo clippy` PASS — ExitCode 0 (xem UPDATE)
- [x] xlsx status đồng bộ với code — T61-T69 + T83-T85 → Hoàn thành (xem UPDATE)
- [x] Không còn P0 issue — JwtValidator defensive fix applied, clippy clean

---

## G. Invariants bổ sung phát hiện qua audit

- `AdminPool` chỉ hợp lệ ở 2 endpoint cross-tenant (`GET /tenants`, `GET /users/me`) và pre-tenant (`POST /tenants`). Mọi endpoint tenant-scoped phải dùng `SharedConnection`.
- `AppPool` dùng trong `chat.rs` post-stream persist **có thể** là pattern hợp lệ nhưng phải đi kèm `SET LOCAL ROLE gmrag_app` + `SET LOCAL app.tenant_id` thủ công — đây là pattern dễ vi phạm nếu duplicate. Nếu thêm SSE/async path mới, ưu tiên dùng helper `rls.rs::with_rls_connection` thay vì tự SET LOCAL.
- ReBAC `list_*` predicate SQL biên dịch tay **phải đồng bộ** với `rbac/model.rs::rewrite_for` mỗi khi đổi luật viewer. Test `documents_acl`/`chat_routes` không tự động bắt mismatch logic nâng cao (chỉ bắt regression known-case).

---

## H. Gợi ý cho Frontend Sprint (T70-T77)

- **Auth:** FE đã có `keycloak-js` + `next-auth` 5.0.0-beta.31 trong `package.json`, nhưng chưa wire. Cần setup NextAuth provider + token passthrough cho `lib/acl.ts::AclClientConfig`.
- **`AclShareDialog` sẵn sàng mount** — nhận `AclClientConfig { baseUrl, tenantId, token }` tường minh, không global state. Mount vào nút "Share" trên document/chat detail (T75).
- **API contract đã freeze** cho ACL (T67): `GET/POST /tenants/{tid}/acl`, `DELETE /tenants/{tid}/acl/{grant_id}`. FE `lib/acl.ts` đã khớp đúng field snake_case.
- **Type sharing:** không có codegen OpenAPI — FE type tự duy trì (`acl.ts`). Nếu backend đổi envelope `{error:{code,message}}` hoặc field grant, phải đồng bộ tay.
- **Chat SSE:** `POST /tenants/{tid}/chat_sessions/{sid}/chat` trả `text/event-stream` với event JSON `{type:"text|citation|citation_unknown|done|error", ...}` (T61). FE cần EventSource/fetch-stream parser.
- **Next.js version mismatch cảnh báo:** `package.json` declare `next: 16.2.9` + `react: 19.0.0-rc` nhưng `eslint-config-next: 15.0.3` + `@types/react: 18.3.12`. Có thể cần upgrade eslint-config-next + @types/react khi build thật.
- **Quota/usage UI** (T69): 3 endpoint sẵn sàng — `GET /tenants/{tid}/metering/usage`, `GET /tenants/{tid}/quotas`, `GET /tenants/{tid}/audit_logs`. Owner-only.
- **BYOK settings UI** (T68): `GET/PUT /tenants/{tid}/settings/llm` sẵn sàng, GET trả `{configured: false}` khi chưa set, PUT encrypt on write. FE form cần field `api_key` (masked khi edit).

---

## UPDATE — Sau khi implement fixes (2026-06-22)

### D1.1 — cargo test --workspace (SQLX_OFFLINE=true, DATABASE_URL override → localhost)

```
test result: ok. 93 passed; 0 failed   (gmrag-api lib)
test result: ok. 7 passed; 0 failed    (acl_routes)
test result: ok. 7 passed; 0 failed    (chat_routes)
test result: ok. 19 passed; 0 failed   (document_routes)
test result: ok. 4 passed; 0 failed    (documents_acl)
test result: ok. 3 passed; 0 failed    (graph_routes)
test result: ok. 3 passed; 0 failed    (metering)
test result: ok. 7 passed; 0 failed    (metering_routes)
test result: ok. 3 passed; 0 failed    (pool_role)
test result: ok. 10 passed; 0 failed   (rbac_check)
test result: ok. 5 passed; 0 failed    (rebac_e2e)
test result: ok. 13 passed; 0 failed   (rls_isolation)
test result: ok. 2 passed; 0 failed    (schema_acl)
test result: ok. 2 passed; 0 failed    (schema_chat)
test result: ok. 2 passed; 0 failed    (schema_documents)
test result: ok. 2 passed; 0 failed    (schema_graph)
test result: ok. 2 passed; 0 failed    (schema_llm)
test result: ok. 6 passed; 0 failed    (schema_rebac)
test result: ok. 3 passed; 0 failed    (schema_system)
test result: ok. 2 passed; 0 failed    (seed_verify)
test result: ok. 6 passed; 0 failed    (settings_routes)
test result: ok. 13 passed; 0 failed   (tenant_routes)
test result: ok. 12 passed; 0 failed   (workspace_routes)
test result: ok. 26 passed; 0 failed   (gmrag-core lib)
test result: ok. 52 passed; 0 failed   (gmrag-worker lib)
test result: ok. 3 passed; 0 failed    (process_job_retry)
test result: ok. 3 passed; 0 failed    (qdrant_writer)
test result: ok. 9 passed; 0 failed    (select_embedder)
test result: ok. 7 passed; 0 failed    (select_graph_extractor)
```

**Tổng: 326 passed, 0 failed, 0 ignored.** Panic `overflow when subtracting duration from instant` **không tái diễn** ở lần chạy này, nhưng audit C6 đã nhận diện đây là latent bug phụ thuộc monotonic clock (máy vừa boot). Đã apply fix phòng thủ (xem D1.1-fix bên dưới).

#### D1.1-fix — JwtValidator monotonic clock (`backend/crates/api/src/auth/jwt.rs:83`)

```rust
// ❌ Trước — panic nếu monotonic clock thấp (máy vừa khởi động)
fetched_at: Instant::now() - Duration::from_secs(3600),

// ✅ Sau — checked_sub, fallback now (cache rỗng → vẫn fetch ở lần đầu)
fetched_at: Instant::now()
    .checked_sub(Duration::from_secs(3600))
    .unwrap_or(Instant::now()),
```

Verdict: behavior-preserving defensive fix, không đổi logic nghiệp vụ. 8/8 `auth::jwt::tests` PASS sau fix.

### D1.2 — cargo clippy --workspace --all-targets -- -D warnings

```
Finished `dev` profile [unoptimized + debuginfo] target(s) in 20.28s
ExitCode: 0
```

**Clippy PASS — 0 warnings, 0 errors.** Không cần fix clippy nào.

### D1.3 — xlsx sync

Trạng thái working tree **trước** audit-fix: T61-T69 đã được sync ở working tree (uncommitted, từ session trước) → "Hoàn thành"; T83-T85 vẫn "Chưa bắt đầu". Audit report (dựa trên HEAD `5c3ae8a` committed) ghi toàn bộ T61-T85 là "Chưa bắt đầu" — giải thích: session trước đã sync T61-T69 nhưng chưa commit.

Đã update **T83, T84, T85** (rows 88-90):
- Col K (Trạng thái): `Chưa bắt đầu` → `Hoàn thành`
- Col O/P/Q (Test đỏ/Triển khai/Test xanh): `Chưa` → `Hoàn tất`
- Col R (Commit): `Chưa` → `Hoàn tất` (theo convention của T1-T60: "Hoàn tất", không dùng commit hash)
- Col L (% hoàn thành): giữ nguyên formula `=IF(K##="Hoàn thành",1,...)` — auto-compute
- Col S (Kiểm thử/verify) + Col T (Ghi chú): đã đúng, không đổi

Verify sau sync: T61-T69 + T83-T85 → tất cả `[OK]` (status=Hoàn thành, red/impl/green/commit=Hoàn tất).
Dashboard sheet `Tổng quan` toàn formula (COUNTIF) — auto-update, không cần sửa tay.

### D2.1 — handoff.md archived

`docs/progress/handoff.md` (HEAD `5517cb6`, stale): prepend archive notice trỏ tới `AUDIT_PRE_FRONTEND.md`. Không xóa (preserve history).

### D2.2 — .sqlx claims fixed

`docs/progress/T61.md`, `T68.md`, `T69.md`: row `backend/.sqlx/` đổi `Sửa` → `Không đổi` + note "no-op — routes dùng runtime `sqlx::query`, không có `query!` macro (xem `AUDIT_PRE_FRONTEND.md` C4)".

### Readiness checklist FINAL
- [x] Git tree sạch (sau commit)
- [x] `cargo test --workspace` PASS — 326 passed, 0 failed
- [x] `cargo clippy --workspace --all-targets -- -D warnings` PASS — ExitCode 0
- [x] xlsx đồng bộ (T61-T69 + T83-T85 → Hoàn thành / 100% / Hoàn tất)
- [x] handoff.md archived
- [x] T61/T68/T69.md .sqlx claims fixed
- [x] Không còn P0 issue

---

## FINAL STATUS — Sẵn sàng Frontend Sprint

- cargo test: 326 passed, 0 failed
- cargo clippy: ExitCode 0
- xlsx: T61-T85 → Hoàn thành (100%)
- git: clean (HEAD: `f357eae`, pushed to origin/main)
- /health: 200 (`{"service":"gmrag-api","status":"ok","uptime_ms":460}` — boot verify với `GMRAG_HTTP_BIND=127.0.0.1:8088` tránh conflict Keycloak:8080, `QDRANT_URL=http://localhost:6334` gRPC port)

✅ SẴN SÀNG BẮT ĐẦU T70 — Frontend Sprint
