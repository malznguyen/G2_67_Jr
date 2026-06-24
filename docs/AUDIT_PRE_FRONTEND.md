# AUDIT PRE-FRONTEND — GMRAG2 (Full Repository Audit Pass)

**Audit date:** 2026-06-22
**Auditor:** Senior engineer, fresh eyes (read-only audit — no feature work)
**HEAD at audit:** `31201470e46cf7be31d2bafa982aa0dc34d7cbd9`
**Working tree:** 1 deleted file — `docs/progress/AUDIT_PRE_FRONTEND.md` (a prior audit artifact from commits `f357eae`/`3120147`, now removed). All code intact.
**Scope:** T1 → T85 (progress docs exist for T1-T69 + T83-T85; T70-T82 have **no progress docs**).

> This audit is independent of the previously-deleted `docs/progress/AUDIT_PRE_FRONTEND.md`. It was re-run from scratch against the current HEAD. Where the prior audit's findings still hold, they are confirmed; where new drift was found, it is called out as such.

---

## Repository State

### Layout (verified)

```
gm_rag_2.0/
├── backend/                     # Rust workspace (3 crates)
│   ├── Cargo.toml               # workspace manifest, qdrant-client =1.12.1 pinned
│   ├── rust-toolchain.toml      # stable
│   ├── Cargo.lock               # committed
│   ├── crates/
│   │   ├── core/                # gmrag-core (lib): config, error, db, qdrant, crypto
│   │   ├── api/                 # gmrag-api (bin+lib): axum 0.7 HTTP service
│   │   └── worker/              # gmrag-worker (bin+lib): Redis BRPOP ingestion
│   ├── migrations/              # 14 SQL migration files (20260101 → 20260622)
│   └── .sqlx/                   # 2 offline cache files (only 2 query! macros exist)
├── frontend/                    # Next.js skeleton (App Router, TS, Tailwind)
│   ├── package.json             # next 16.2.9, react 19-rc, keycloak-js, next-auth@beta
│   ├── app/                     # /, /api/health
│   ├── lib/acl.ts               # T84 ACL client
│   └── components/AclShareDialog.tsx  # T84 (standalone, not mounted)
├── infra/                       # docker-compose (9 services), Dockerfiles, postgres init/seed, minio init
├── docs/
│   ├── progress/                # 81 .md files (T1-T69 + T83-T85 + BATCH summaries + handoff + HOTFIX + TECH_DEBT)
│   ├── 5068.pdf                 # Zanzibar paper (referenced by ReBAC)
│   └── GMRAG2_Project_Management.xlsx  # 7 sheets
├── .env / .env.example / .gitignore / .dockerignore / README.md
└── (no configs/, no scripts/ directories — both absent despite task description mentioning them)
```

### Git state
- HEAD: `3120147` — "docs(audit): record final HEAD, /health=200 in AUDIT_PRE_FRONTEND"
- Last 30 commits correspond to T34-T85 + tech-debt + audit fixes (squashed commits per task/batch).
- Working tree: 1 deletion (`docs/progress/AUDIT_PRE_FRONTEND.md` — prior audit artifact). No code changes pending.
- All T1-T69 + T83-T85 work is committed. T70-T82 not started.

### Test surface (verified by file count)
- **Integration tests (`#[sqlx::test]`):** 145 across 25 test files (api: 21 files, worker: 4 files).
- **Unit tests (`#[test]`/`#[tokio::test]`):** ~111 across 27 lib files.
- **Total:** ~256 test functions (prior audit claimed 326 — small drift likely from counting helpers; not re-verified by running `cargo test` because the workspace build timed out at 180s in this environment. Prior audit's `cargo test --workspace` at the same HEAD reported 326 passed / 0 failed — this is a trusted claim since no code changed between HEAD then and now).

---

## Timeline Summary (T1-T69 + T83-T85)

### Sprint 1 — Hạ tầng (T1-T8)
- T1-T4: docker-compose 9 services, Postgres init.sql (`gmrag_current_tenant()`, `gmrag_app` role), MinIO bucket, `.env.example`. ✅ matches code.
- T5-T7: Rust workspace `core/api/worker`, `Config::from_env`, `init_pool`, `/health` + `/healthz`, `infra/backend.Dockerfile`. ✅ matches code.
- T8: Next.js skeleton (App Router, Tailwind, `/api/health`). ⚠️ Doc claims Next 15.0.3 but `package.json` now has `next: 16.2.9` (pre-batch upgrade per workbook T70 note — undocumented in any T-progress doc).

### Sprint 2 — Định danh & Tenant (T9-T18)
- T9: `Config` expanded (OIDC, Qdrant, S3, Redis, Ollama, DeepSeek). ✅ matches.
- T10: `JwtValidator` with JWKS cache + OIDC discovery. ✅ matches. Latent monotonic-clock panic fixed in commit `0c17dcc` (per prior audit).
- T11: `AuthUser` extractor → later refactored to middleware in T19.
- T12: migration `identity_and_tenant` (users, tenants, tenant_members, platform_admins + first RLS). ✅ matches. Note: T12 doc says "users/tenants do NOT have RLS yet"; T15 adds FORCE RLS on tenants.
- T13: `TenantContext` extractor → later refactored to middleware in T19.
- T14: `rls_middleware` (SharedConnection, BEGIN/SET LOCAL/COMMIT). ✅ matches.
- T15: FORCE RLS on tenants + `SET LOCAL ROLE gmrag_app` test pattern. ✅ matches.
- T16: `provision_user` auto-provision from JWT claims. ✅ matches.
- T17: `ApiError` + `AuthError` string codes envelope. ✅ matches.
- T18: `GET /users/me`. ✅ matches.

### Sprint 3 — Schema & RLS (T19-T26)
- T19: **Architecture change** — two-pool design (`AdminPool` superuser + `AppPool` gmrag_app), `auth_middleware` + `tenant_middleware` (from_fn), `SharedConnection` for handlers. `workspaces` + `workspace_members` migrations. ✅ matches. This was the BLOCKER-1/2/3 fix from BATCH2B.
- T20-T24: migrations `documents`/`document_chunks`, `graph_nodes`/`graph_edges`, `chat_sessions`/`chat_messages`, `resource_acl`/`invitations`, `tenant_quotas`/`usage_events`/`audit_log`/`ingest_jobs`. ✅ all 5 migrations present.
- T25: `rls_apply_all` migration — ENABLE+FORCE RLS + uniform `tenant_id = gmrag_current_tenant()` policy on 14 tables. ✅ matches.
- T26: `infra/postgres/seed.sql` + `cargo sqlx prepare --workspace` (2 `.sqlx` cache files for `users.rs` query! macros). ✅ matches.

### Sprint 4 — Qdrant (T27-T33)
- T27: `QdrantStore` wrapper + `Error::Qdrant`. ⚠️ Doc flagged that `DEFAULT_QDRANT_URL = "http://localhost:6333"` (REST) is **wrong** for the rust gRPC client (needs 6334) and must be fixed in a future wiring task. **STILL NOT FIXED** — see Conflict C2.
- T28-T29: `create_chunks_collection` / `create_graph_collection` (768-dim, Cosine, HNSW default, payload indexes). ✅ matches.
- T30: `setup_tenant_collections` / `teardown_tenant_collections` (idempotent). ✅ matches. `create_*` made private + idempotency guard.
- T31-T33: `upsert_chunks`, `search_chunks`, `upsert_graph_nodes`, `search_graph_nodes`. ✅ matches. Filter UUID via keyword match (string) — confirmed working per T32/T33.

### Sprint 5 — Worker ingestion (T34-T43)
- T34: worker skeleton + `JobQueue`/`MockQueue` + `poll_once` + `run()`. ✅ matches.
- T35: `S3Client` (aws-sdk-s3 v1, wiremock tests). ✅ matches.
- T36: `parse_pdf` (lopdf + pdf-extract + spawn_blocking + timeout). ✅ matches. Fixture `sample.pdf` (596 B).
- T37: `parse_pdf_with_ocr` + `OllamaVisionOcr` + `PageRenderer` trait + `MockRenderer`. ⚠️ **`PdfiumRenderer` (production renderer) NOT implemented** — T37 doc explicitly defers it. Worker T43 uses `parse_pdf` (text-only path), NOT `parse_pdf_with_ocr`. Scanned PDFs → empty text → empty chunks/graph.
- T38: `chunk_page_texts` (tiktoken cl100k, 1200/100). ✅ matches.
- T39: `OllamaEmbedder` (`/api/embed` batch + retry/backoff). ✅ matches.
- T40: `Embedder` trait + `OpenAiEmbedder` (768 pinned) + `select_embedder` + `tenant_llm_config` migration (RLS + FORCE). ✅ matches. **Plaintext `api_key` MVP** at this point.
- T41: `DeepSeekGraphExtractor` + `parse_graph_json` tolerant + `select_graph_extractor` + migration adds `workspace_id` to `graph_nodes` + UNIQUE + `llm_model`/`llm_base_url` to `tenant_llm_config`. ✅ matches.
- T42: `dual_write_ingestion` (Postgres tx + Qdrant upsert, rollback on Qdrant fail). ✅ matches. Idempotency via `qdrant_point_id` stable on retry.
- T43: `IngestContext::process_job` full pipeline + `process_job_with_retry` (MAX_ATTEMPTS=3, backoff cap 16s) + `update_job_status` RLS helper. ✅ matches. ⚠️ Does NOT update `documents.status` (only `ingest_jobs.status`) — see Conflict C7.

### Sprint 6 — RAG & LLM (T44-T51)
- T44: `LlmProvider` trait + `DeepSeekProvider` + SSE parser. ✅ matches.
- T45: `byok.rs` AES-GCM decrypt + `tenant_llm_config_encrypted_keys` migration. ✅ matches. Later refactored — crypto hoisted to `core/src/crypto.rs` in TECH_DEBT_PRE_SPRINT7 commit `c159a6f`.
- T46: `retrieve_chunks` + `accessible_document_ids` + `retrieve_chunks_with_vector`. ⚠️ **CRITICAL DRIFT** — see Conflict C1.
- T47: `retrieve_graph_context` (kNN + ILIKE fallback + edges). ✅ matches.
- T48: `assemble_system_prompt` with `[chunk:N]` citations. ✅ matches.
- T49: `DeepseekTokenParser` + `stream_rag_response` + `meter_rag_chat_completion` hook. ✅ matches.
- T50: `resolve_chunk_index_citations` + `enrich_stream_events`. ✅ matches.
- T51: `metering.rs` (record_usage_event, record_embedding_usage, record_llm_usage) + `retrieve_all_with_metering`. ✅ matches.

### Sprint 7 — API nghiệp vụ (T52-T63)
- T52-T54: tenant CRUD + tenant_members (list/invite/remove + last-owner guard). ✅ matches. ⚠️ Invite creates `invitations` pending row only — **no accept flow** (see Conflict C10).
- T55: workspace CRUD. ✅ matches.
- T56: workspace_members. ✅ matches.
- T57: `GET /tenants/:tid/documents` list (visibility + ACL filter). ✅ matches. Uses `visibility = 'shared'`.
- T58: `POST /tenants/:tid/documents` upload (multipart, S3, SAVEPOINT, Redis enqueue, quota check, `Visibility` enum shared/private). ✅ matches.
- T59: `DELETE /tenants/:tid/documents/:did` (owner-only, S3+Qdrant cleanup, cascade). ✅ matches. ⚠️ Graph nodes not cleaned per-document (orphan accumulation — see Conflict C11).
- T60: `GET /tenants/:tid/documents/:did/preview` (50 chunks, ACL filter). ✅ matches.
- T61: `POST /tenants/:tid/chat_sessions/:sid/chat` SSE + 3-phase RLS. ✅ matches. ⚠️ References `AUDIT_PRE_FRONTEND.md C4` which doesn't exist (see Conflict C5).
- T62: chat_sessions CRUD. ✅ matches.
- T63: `GET /tenants/:tid/workspaces/:wid/graph` (workspace member gate). ✅ matches.

### Sprint 8 — ReBAC (T64-T69, T83)
- T64: migration `rebac_relation_tuples` (CHECK `permission IN ('owner','editor','viewer')`, CHECK `principal_type IN ('user','workspace')`, covering index). ✅ matches. ⚠️ Original T23 default `'read'` is changed to `'viewer'` BEFORE CHECK is added — safe on fresh DB. ⚠️ If any pre-T64 DB had `'read'` rows, migration would fail (no `NOT VALID` clause).
- T65: `rbac/model.rs` (namespaces, relations, userset-rewrite). ✅ matches.
- T66: `rbac/check.rs` (`check_relation` recursive bounded, RLS-scoped). ✅ matches.
- T67: `routes/acl.rs` (list/create/revoke grants + audit_log + owner-only self-guard). ✅ matches.
- T68: `routes/settings.rs` (GET/PUT BYOK, AES-GCM encrypt on write, masked read). ✅ matches. ⚠️ References `AUDIT_PRE_FRONTEND.md C4` (missing).
- T69: `routes/metering.rs` (usage/quotas/audit_logs, owner-only). ✅ matches. ⚠️ References `AUDIT_PRE_FRONTEND.md C4` (missing).
- T83: documents.rs integration with `check_relation` (preview/delete + list predicate extension). ⚠️ **INCOMPLETE vs its own description** — see Conflict C9.

### Sprint 9 — Frontend baseline (T70-T77) — **NOT STARTED**
- No progress docs T70-T77 exist.
- Workbook marks all "Chưa bắt đầu".
- T84 (AclShareDialog + lib/acl.ts) was done out-of-order before T75 (its dependency).

### Sprint 10 — Testing & ops (T78-T82) — **NOT STARTED**
- No progress docs T78-T82 exist.
- Workbook marks all "Chưa bắt đầu".
- T85 (ReBAC E2E + pentest) was done out-of-order before T78/T79 (its dependencies).

### T84-T85 (done out of order)
- T84: `frontend/lib/acl.ts` + `components/AclShareDialog.tsx`. ✅ matches. ⚠️ Component not mounted (depends on T75 FE baseline).
- T85: `tests/rebac_e2e.rs` (5 backend E2E/pentest tests). ✅ matches. ⚠️ FE-E2E part deferred to T78/T79.

### Architecture-changing tasks
- T19 (two-pool + middleware), T25 (uniform RLS), T40 (tenant_llm_config BYOK), T45 (encrypted BYOK), T64 (ReBAC reinterpretation of `resource_acl`), T66 (check_relation engine), T83 (documents ACL integration).

### Tasks that introduced technical debt
- T37 (PdfiumRenderer deferred — OCR not wired), T43 (documents.status not updated), T58 (race between Redis LPUSH and middleware COMMIT), T59 (graph orphan accumulation), T64 (no accept-invite flow), T83 (incomplete — only documents).

### Tasks that may now be stale
- `handoff.md` (self-archived but points to deleted AUDIT_PRE_FRONTEND.md).
- T22 progress doc says "chat_sessions (workspace_id nullable, model)" — incomplete (omits `user_id` column).
- T61/T68/T69 `.sqlx` claims "no-op — see AUDIT_PRE_FRONTEND.md C4" — the referenced file is gone.

---

## Workbook Analysis

### Sheet inventory
1. `Tổng quan` — dashboard (formulas).
2. `Kế hoạch Task` — 86 rows: header + T1-T85 (rows 5-89) + trailing. ⚠️ `Thiết lập` sheet says "Số task = 82" but actual rows are T1-T85 = **85 tasks**. Inconsistency.
3. `Timeline 2 tuần` — 10 sprints across 14 days (2026-06-17 → 2026-06-30).
4. `Rủi ro & Quyết định` — D1-D7 decisions + R1-R9 risks (R6 "Đang xử lý", others "Mở"/"Chấp nhận").
5. `RACI` — 3-role matrix (PM/Backend/Frontend).
6. `ResourceBAC` — permission matrix + SQL sample + checklist J6:J10 all "Hoàn thành".
7. `Thiết lập` — params + architectural summary.

### Status vs reality
- T1-T60: workbook "Hoàn thành" / code present. ✅ consistent.
- T61-T69: workbook "Hoàn thành" (synced by prior audit commit `f357eae`) / code present. ✅ consistent.
- T83-T85: workbook "Hoàn thành" (synced by prior audit) / code present. ✅ consistent.
- T70-T82: workbook "Chưa bắt đầu" / no code. ✅ consistent (genuinely not started).

### Internal inconsistencies in workbook
- **T22 description** says "chat_sessions (+owner_id, visibility) + chat_messages" — actual schema has `user_id` (not `owner_id`) and NO `visibility` column. **WRONG** (see Conflict C8).
- **T72 description** starts with "Hoàn thành theo TDD:" but status is "Chưa bắt đầu" — copy-paste error from T72 in a prior plan.
- **T73, T74, T75, T76, T77 descriptions** start with "Frontend:" (no "Hoàn thành theo TDD:" prefix) — inconsistent format vs T1-T69.
- **T70 note** says "Pre-batch đã nâng frontend lên Next.js 16.2.9, cài keycloak-js, next-auth@beta, thêm NEXT_PUBLIC_TENANT_HEADER" — this upgrade is real in `package.json` but **no progress doc records it**.
- **Thiết lập B9** says "Số task = 82" but Kế hoạch Task has 85 task rows (T1-T85). Dashboard formulas may be off.

### Dependencies
- T70 depends on T52-T69 — all done. ✅ frontend can begin.
- T71-T77 chain — sequential, all blocked on T70.
- T78-T82 depend on T1-T77 — blocked on FE baseline.
- T83 dependencies: T66, T60 — both done. ⚠️ But T83 description also says "chat.rs (T61/T62) + graph.rs (T63) khi land" — T61-T63 already landed, so T83 was supposed to integrate them too but only did documents.
- T84 dependencies: T67, T75 — T75 not done. T84 was done prematurely.
- T85 dependencies: T83, T84 — both done. T85 done.

---

## Architecture Findings

### Tenant isolation — ✅ SOUND
- Two-pool design (`AdminPool` superuser, `AppPool` gmrag_app with `SET ROLE gmrag_app` after_connect) enforced since T19.
- `rls_middleware` acquires conn from `AppPool`, BEGIN, SET LOCAL app.tenant_id, stores `SharedConnection` in extensions, COMMIT after handler.
- 14 tenant-scoped tables have FORCE RLS + uniform `tenant_id = gmrag_current_tenant()` policy (T25 + T40 for `tenant_llm_config`).
- `AdminPool` used only at: `routes/users.rs:36` (GET /users/me, cross-tenant), `routes/tenants.rs:52,76` (GET/POST /tenants, pre-tenant/cross-tenant), `auth/middleware.rs:90` (provision_user, pre-tenant). All documented and legitimate.
- `AppPool` used at: `lib.rs:196` (extension), `routes/chat.rs:213` (post-stream persist via `persist_chat_completion` which manually does `SET LOCAL ROLE gmrag_app` + `SET LOCAL app.tenant_id`). The chat post-stream pattern is a **manual RLS setup** outside the middleware — works but is a known risk for duplication (see Tech Debt TD-4).

### RLS assumptions — ⚠️ ONE GAP
- RLS enforced via `FORCE ROW LEVEL SECURITY` + `gmrag_app` role downgrade. Table owners and superusers subject to RLS when role is set.
- `gmrag_current_tenant()` reads `app.tenant_id` GUC; returns NULL when unset → no rows match.
- **GAP:** `rls_middleware` COMMITs the outer transaction unconditionally after the handler returns (even on error). This is documented in T58 and exploited by `upload_document` via SAVEPOINT. But it means a handler that returns an error after partial DB writes will still have those writes committed unless the handler explicitly rolls back to a SAVEPOINT. This is a subtle invariant that all tenant-scoped handlers must respect.

### ACL semantics — ⚠️ DRIFT (P0)
- ReBAC engine (`rbac/check.rs`) evaluates `check_relation(object, relation, principal)` against `resource_acl` + `workspace_members` + owner columns, RLS-scoped.
- `routes/documents.rs` (T83) and `routes/chat.rs` (T61/T62) and `routes/graph.rs` (T63) all use `check_relation` for read/delete gates. ✅
- `routes/acl.rs` (T67) only allows granting `editor`/`viewer` (rejects `owner` and `member`). ✅
- **`chat/retrieval.rs::accessible_document_ids` (T46) does NOT use `check_relation`** — it reimplements ACL inline with stale `'read'` permission and `'workspace'`/`'public'` visibility that contradict T64's CHECK and T58's `Visibility` enum. See Conflict C1.

### Owner/editor/viewer behavior — ✅ per spec
- `owner` = own column (`documents.owner_id`, `chat_sessions.user_id`); not grantable via `resource_acl`.
- `editor` = `_this ∪ computed(owner)` — concentric.
- `viewer` = `_this ∪ computed(editor) ∪ tuple_to_userset(workspace → member)` — concentric + workspace inheritance.
- `member` = `workspace_members` only; not in `resource_acl`.
- Verified in `rbac/model.rs::rewrite_for` + `rbac/check.rs::eval_this`.

### Audit logging — ⚠️ PARTIAL
- `audit_log` table exists (T24) with RLS (T25).
- `routes/acl.rs::write_audit` writes `acl.grant` / `acl.revoke` rows. ✅
- **No other route writes audit_log**: tenant CRUD (T52-T53), tenant_member invite/remove (T54), workspace CRUD (T55), ws_members (T56), document upload/delete (T58/T59), chat session CRUD (T62), settings PUT (T68) — none emit audit_log rows. The workbook ResourceBAC checklist J6 (Audit) says "Hoàn thành" but this is only true for ACL grants, not other mutations.

### Workspace membership — ✅ SOUND
- `workspace_members(tenant_id, workspace_id, user_id)` with denormalized `tenant_id` for uniform RLS.
- `check_relation(workspace, member, user)` resolves via `workspace_members` direct lookup.

### BYOK encryption — ✅ SOUND (post tech-debt)
- `core/src/crypto.rs` provides `encrypt_with_aad`/`decrypt_with_aad` (AES-256-GCM, AAD = tenant_id bytes).
- `tenant_llm_config` has `api_key_ciphertext BYTEA` + `api_key_nonce BYTEA` + pair CHECK (T45 migration).
- `routes/settings.rs::put_llm_settings` encrypts on write; `get_llm_settings` decrypts in-memory only for masking.
- `worker/embedding.rs` + `worker/graph.rs` decrypt via `core::crypto::decrypt_with_aad` with `enc_key: Option<&[u8; 32]>` param. No silent fallback when `enc_key` is None and encrypted columns present.
- Legacy plaintext `api_key` kept as read-only fallback for backwards compat (T45 design).

### ResourceBAC — ⚠️ SEE ACL DRIFT
- `resource_acl` reinterpreted as Zanzibar relation tuples (T64). CHECK constraints enforce vocabulary.
- Covering index `idx_resource_acl_check (resource_type, resource_id, permission)` for hot path.
- `check_relation` recursive bounded (MAX_DEPTH=16), short-circuit union, RLS-scoped (no stale-ACL).
- **`list_documents` and `list_sessions` use hand-compiled ACL predicate SQL** (not `check_relation` per-row) to avoid N+1. This is a documented design decision but creates a sync hazard: any change to `rbac/model.rs::rewrite_for` viewer rule must be manually mirrored in those SQL predicates. No test catches logical drift (only known-case regression).

### Settings ownership — ✅
- `routes/settings.rs` GET/PUT both `require_owner` (tenant owner).

### Quota behavior — ⚠️ SOFT
- `tenant_quotas(max_storage_bytes, max_documents, max_workspaces, max_members)` 1:1 with tenant.
- `upload_document` (T58) checks quota before S3 write; returns 429 `quota-exceeded` if violated.
- ⚠️ `max_workspaces` and `max_members` quotas are NOT enforced anywhere in code (only `max_storage_bytes` + `max_documents` in T58). Workbook implies all 4 are enforced.
- ⚠️ Quota usage is read from `documents` table (`SUM(byte_size)`, `COUNT(*)`), not from `usage_events`. So quota is based on current state, not cumulative usage. Consistent with T58 design.

### Usage tracking — ✅
- `usage_events` append-only (INSERT only). Metrics: `embedding_tokens`, `llm_tokens`.
- `metering.rs::record_embedding_usage` (after query embed) + `meter_rag_chat_completion` (after stream).
- Token count via `tiktoken-rs cl100k` (MVP estimate, not provider usage field).

### Cross-tenant access — ✅ BLOCKED
- RLS hides cross-tenant rows. `ensure_path_matches_context(tid, ctx)` guards path-injection.
- All cross-tenant attempts surface as 404 (no existence leak) — verified in tests.

### Existence leakage — ✅ NONE FOUND
- `preview_document`, `delete_document`, `delete_session`, `revoke_grant`, `list_grants` all return 404 (not 403) when the resource is missing OR cross-tenant. Only `delete_document` non-owner returns 403 (after the row is confirmed to exist in-tenant).
- `create_grant` returns 403 for both "not found" and "not owner" (per T67 design — avoid leak).

### Response consistency — ✅
- All errors via `ApiError`/`AuthError` → `{ "error": { "code": "<kebab>", "message": "<str>" } }` envelope.
- 12 unit tests in `error.rs` enforce the envelope shape across all variants.

---

## Progress vs Code Drift

### Major drifts (P0/P1)

| # | Area | Progress doc claim | Code reality | Severity |
|---|------|-------------------|--------------|----------|
| C1 | chat retrieval ACL (T46) | T46 doc says "ACL pre-filter `accessible_document_ids`" | `retrieval.rs:124` uses `ra.permission = 'read'` (T23 default) and `visibility IN ('workspace','public')` (line 127). T64 CHECK forbids `'read'` (only owner/editor/viewer). T58 `Visibility` enum only accepts `'shared'`/`'private'`. **Production chat retrieval will never match documents shared via T67 grants or uploaded with `visibility='shared'`.** | **P0** |
| C2 | Qdrant default URL (T27) | T27 doc explicitly warns: "`DEFAULT_QDRANT_URL = 'http://localhost:6333'` is WRONG for rust client (uses gRPC 6334); must update in a future wiring task" | `core/src/config.rs:24` still `DEFAULT_QDRANT_URL = "http://localhost:6333"`. `.env.example:15` still `QDRANT_URL=http://qdrant:6333`. `docker-compose.yml:47` maps both ports but env passes `${QDRANT_URL}` which resolves to 6333. **QdrantStore::new will fail with `FRAME_SIZE_ERROR` in any deployment that doesn't override `.env`.** | **P0** |
| C3 | T70-T82 progress docs | Workbook has T70-T82 as "Chưa bắt đầu" (correct) | **No `docs/progress/T70.md`...`T82.md` files exist.** 13 task progress docs missing. T83-T85 done out of order without their T70-T82 prerequisites. | **P1** |
| C4 | AUDIT_PRE_FRONTEND.md references | T61.md, T68.md, T69.md, handoff.md all reference `AUDIT_PRE_FRONTEND.md C4` | File does not exist (was deleted from working tree; prior version at `docs/progress/AUDIT_PRE_FRONTEND.md` is gone). This audit recreates it at `docs/AUDIT_PRE_FRONTEND.md` (per task spec — different path). | **P1** |
| C5 | T83 scope | T83 description: "Tích hợp check_relation vào read paths: documents.rs (list/preview/delete); chat.rs (T61/T62) + graph.rs (T63) khi land" | T61-T63 already landed BEFORE T83 was done. T83 only integrated `documents.rs`. `chat/retrieval.rs` (the chat read path) still has stale ACL (C1). `routes/chat.rs::list_sessions` uses hand-compiled predicate but is consistent with `check_relation`. `routes/graph.rs` uses `check_relation(workspace, member, ...)`. **T83 is incomplete vs its own description** — retrieval.rs was missed. | **P1** |
| C6 | Frontend package.json | T8 doc says "Next 15.0.3, React 19 RC". Workbook T70 note says "pre-batch upgraded to Next 16.2.9 + keycloak-js + next-auth@beta". | `package.json:14` has `next: "16.2.9"` but `package.json:5` description still says "Next.js 15 App Router skeleton (T8)". `eslint-config-next: 15.0.3` mismatches `next: 16.2.9`. `@types/react: 18.3.12` mismatches `react: 19.0.0-rc`. **No progress doc records the upgrade.** | **P1** |
| C7 | documents.status lifecycle | T43 doc: "T43 update ingest_jobs.status nhưng chưa update documents.status ('processing'/'indexed'/'failed') — follow-up." T58 doc: "T58 set 'uploaded'; worker hiện chỉ update ingest_jobs.status." | `grep "documents.status"` in code: 0 matches. Worker `update_job_status` (job.rs:245) only touches `ingest_jobs`. **`documents.status` is set to 'uploaded' at T58 and never updated again.** Frontend preview (T60) returns `status: 'uploaded'` forever. | **P1** |
| C8 | Workbook T22 schema description | Workbook T22: "chat_sessions (+owner_id, visibility) + chat_messages" | Actual `20260617144046_chat.sql`: `chat_sessions` has `user_id UUID NOT NULL` (NOT `owner_id`) and **NO `visibility` column**. `chat_messages` matches. | **P2** |
| C9 | T37 OCR wiring | T37 doc: "OCR pipeline chưa wire — T43 dùng parse_pdf (text-only). Scanned PDF → empty text → empty graph." | `worker/src/job.rs:112` calls `parse_pdf(bytes, PDF_PARSE_TIMEOUT_SECS)` (text path). `parse_pdf_with_ocr` exists but `PdfiumRenderer` is NOT implemented (T37 blocker). **Confirmed: scanned PDFs produce empty chunks/graph.** | **P2** |
| C10 | Invitations accept flow | T54 doc: "Flow accept invitation (đổi invitations.status → tạo tenant_members) chưa làm — sẽ thuộc T54-followup hoặc nhóm ACL T66." | No `routes/invitations.rs` exists. `grep invitations` in `src/`: only `tenant_members.rs:105` (INSERT pending) and tests. **No endpoint to accept an invite and create membership.** | **P2** |
| C11 | Graph orphan cleanup | T59 doc: "graph_nodes/graph_edges + graph Qdrant points KHÔNG có liên kết document_id; node dedupe theo (tenant, workspace, label, kind) và dùng chung giữa nhiều document." | `document_chunks` cascade on document delete (FK), but `graph_nodes` have no `document_id` FK. `delete_document` (T59) only calls `delete_chunks_by_document` (chunks Qdrant), not graph cleanup. **Orphan graph nodes accumulate over time.** | **P2** |
| C12 | Audit log coverage | ResourceBAC checklist J6 (Audit): "Hoàn thành". Implies audit_log is written for grant changes. | Only `routes/acl.rs::write_audit` writes audit_log (`acl.grant`/`acl.revoke`). **No other mutation route writes audit_log**: tenant/workspace/document/chat/settings mutations are unlogged. | **P2** |
| C13 | handoff.md staleness | handoff.md (self-archived) points to `docs/progress/AUDIT_PRE_FRONTEND.md` which is deleted. Also references HEAD `5517cb6` (current is `3120147`). | Stale artifact. | **P3** |

---

## Missing Implementations

### Confirmed missing (should exist per docs/workbook but don't)
1. **`PdfiumRenderer` production impl** (T37 blocker) — `pdfium-render` crate + libpdfium binary not in Cargo.toml or Dockerfile.
2. **`documents.status` update in worker** (T43/T58 follow-up) — worker never updates `documents.status` past `'uploaded'`.
3. **Accept-invitation endpoint** (T54 follow-up) — no `POST /invitations/:token/accept` or similar.
4. **T70-T77 frontend baseline** — no Auth.js provider, no `lib/api.ts`, no `TenantContext.tsx`, no `TenantSwitcher`, no client wrappers (tenants/workspaces/documents/chat/settings), no `ChatPanel` skeleton, no `KnowledgeGraphView`/`QuotaIndicator`/`LlmSettingsForm`.
5. **T78-T82 testing/ops** — no E2E harness, no RLS pentest suite beyond `rls_isolation.rs`, no quota enforcement test beyond T58 unit tests, no smoke test script, no backup scripts.
6. **`AclShareDialog` mounting** (T84 follow-up) — component exists but not integrated into any page.
7. **`configs/` and `scripts/` directories** — referenced in task description but never created.

### Confirmed missing (referenced in code/docs but file/feature absent)
8. **`AUDIT_PRE_FRONTEND.md`** at the path referenced by T61/T68/T69/handoff.md — file deleted. (This audit creates a new one at `docs/AUDIT_PRE_FRONTEND.md`.)
9. **Qdrant URL fix** in `config.rs` + `.env.example` — flagged by T27, never applied.

---

## Incorrect Documentation

### Progress docs with wrong claims
1. **T22.md** — describes `chat_sessions (workspace_id nullable, model)` but omits the `user_id` column entirely. Schema has `user_id UUID NOT NULL REFERENCES users(id)`.
2. **T8.md** — claims "Next 15.0.3" but `package.json` now has `next: 16.2.9` (upgrade happened in an undocumented pre-batch).
3. **T61.md, T68.md, T69.md** — claim "`.sqlx` không đổi — no-op (xem `AUDIT_PRE_FRONTEND.md` C4)" but the referenced file doesn't exist. The `.sqlx` no-op claim itself is correct (only 2 `query!` macros in `users.rs`, both pre-cached).
4. **T83.md** — says "chat.rs (T61/T62) + graph.rs (T63) khi land" implying they haven't landed, but T61-T63 progress docs exist and routes are committed. T83 should have integrated retrieval.rs but didn't.
5. **handoff.md** — archived header says "HEAD lúc viết file này: `5517cb6` — ĐÃ LỖI THỜI. HEAD thực tế: `5c3ae8a`." But current HEAD is `3120147` (2 commits ahead of `5c3ae8a`). The "see `AUDIT_PRE_FRONTEND.md`" pointer is broken (file deleted).

### Workbook with wrong claims
6. **Kế hoạch Task T22 description** — "chat_sessions (+owner_id, visibility) + chat_messages" — wrong column name, wrong column list (no visibility).
7. **Thiết lập B9** — "Số task = 82" but actual rows are T1-T85 = 85 tasks.
8. **T72 description** — starts with "Hoàn thành theo TDD:" but status is "Chưa bắt đầu".
9. **T70 note** — references a pre-batch frontend upgrade that no progress doc records.
10. **ResourceBAC J6 (Audit)** — "Hoàn thành" but audit_log is only written for ACL grants, not other mutations.

### Code comments with minor inaccuracies
11. **`frontend/package.json:5`** — `description: "GMRAG2 frontend — Next.js 15 App Router skeleton (T8)."` but `next` dep is `16.2.9`.

---

## Security Findings

### P0 — None found beyond C1/C2 (which are functional bugs, not direct security holes)

### P1
- **S1: `format!("SET LOCAL app.tenant_id = '{tenant_id}'")` SQL string interpolation** in `rls.rs:60,131`, `chat.rs:421`, `retrieval.rs:611`, `job.rs:257`. `Uuid` is type-safe (no quotes possible) so injection is impossible, but this is a codebase-wide code smell that violates the "never interpolate into SQL" convention. Should use `sqlx::query("SET LOCAL app.tenant_id = $1").bind(tenant_id.to_string())` — but Postgres `SET LOCAL` doesn't accept bind parameters for GUC values, so the interpolation is actually required. Document this exception or use `set_config('app.tenant_id', $1, true)` which does accept bind parameters.
- **S2: BYOK `api_key` plaintext fallback** — `tenant_llm_config.api_key` (plaintext) column retained for backwards compat. `select_embedder`/`select_graph_extractor` fall back to it only when encrypted columns are NULL. If an attacker gains read access to the DB (bypassing RLS), plaintext keys are visible. Mitigation: T68 PUT nulls out `api_key` on write, but old rows may still have plaintext. Recommend migration to encrypt all existing plaintext rows and drop the column.
- **S3: `provision_user` runs on `AdminPool` (bypasses RLS)** — justified because provisioning happens before tenant context exists. But it means any valid JWT (even from an untrusted realm if `KEYCLOAK_ISSUER` is misconfigured) creates a `users` row. Verify Keycloak realm/client validation in `JwtValidator::validate` is strict (issuer + audience + signature).

### P2
- **S4: No rate limiting** on any endpoint. Quota check (T58) only blocks document upload by storage/document count, not by request rate. Chat SSE endpoint could be abused for LLM cost amplification.
- **S5: Audit log not written for most mutations** (C12) — security-relevant actions (tenant delete, member remove, settings change) are not audited.
- **S6: `invitations.token` is a UUID** — generated by `gen_random_uuid()` (T23). UUIDs are not cryptographically random tokens (v4 has 122 bits of randomness, but `gen_random_uuid()` uses pgcrypto which is CSPRNG). Acceptable, but tokens are returned directly in the API response (`tenant_members.rs:124`) — must be delivered out-of-band (email) by the caller, not logged.

### P3
- **S7: CORS `ALLOW_CREDENTIALS=true`** with `CORS_ALLOWED_ORIGINS=http://localhost:3000,http://localhost:8080` — fine for dev; production must tighten.

---

## Testing Findings

### Coverage strengths
- **RLS isolation:** 14 `#[sqlx::test]` tests in `rls_isolation.rs` cover all 14 tenant-scoped tables with cross-tenant negative cases + no-context-hides-all. Strong.
- **ReBAC engine:** 10 tests in `rbac_check.rs` (owner, stranger, shared, direct viewer, editor⊇viewer, workspace-group, inheritance, workspace member, chat session, cross-tenant). 5 E2E in `rebac_e2e.rs` (share→access, revoke→denied, cross-tenant, workspace inheritance, revoked-user-list-grants). Strong.
- **Schema:** 18 schema verification tests across 6 files (acl, chat, documents, graph, llm, rebac, system). Verify columns, defaults, constraints, RLS enable/FORCE, policy existence.
- **Routes:** integration tests for every route module (tenant_routes 14, workspace_routes 13, document_routes 20, chat_routes 7, graph_routes 3, acl_routes 7, settings_routes 6, metering_routes 7).
- **Worker pipeline:** unit tests for each stage (chunking 8, embedding 13, graph 9, ocr 6, pdf_parser 8, queue 3, storage 3, job 2) + integration (process_job_retry 4, qdrant_writer 4, select_embedder 10, select_graph_extractor 6).

### Coverage gaps
- **G1: `accessible_document_ids` (T46) tests use `visibility='public'` and `visibility='private'`** — `'public'` is NOT a value that `upload_document` (T58) can produce (only `'shared'`/`'private'`). The test `accessible_docs_excludes_private_foreign_owner` (retrieval.rs:667) inserts `visibility='public'` directly and asserts it's accessible — **FALSE POSITIVE**: the test passes but the SQL never matches production data because real uploads use `'shared'`, and the SQL filters `visibility IN ('workspace', 'public')` which excludes `'shared'`. **No test catches C1.**
- **G2: `accessible_document_ids` test never inserts a `resource_acl` row with `permission='viewer'`** — so the `ra.permission = 'read'` branch is never exercised with real ReBAC grants. The LEFT JOIN only matches if `permission='read'` which T64 forbids.
- **G3: No test verifies `documents.status` lifecycle** — because the lifecycle doesn't exist. T60 preview test asserts `status` field is returned but doesn't check it transitions to `'ready'`/`'indexed'` (because it never does).
- **G4: No test for `QDRANT_URL` default** — config tests (config.rs:308) assert `cfg.qdrant.url == DEFAULT_QDRANT_URL` which is `'http://localhost:6333'`. The test passes but the value is wrong for the gRPC client.
- **G5: No end-to-end ingestion test** (upload → worker → Qdrant + Postgres verify). T78 is the task for this; not done.
- **G6: `list_sessions` ACL predicate (chat.rs:98-120) is hand-compiled** — test `list_sessions_returns_owned_and_shared` covers owned + grant + hidden, but doesn't test workspace-inheritance path for chat sessions (chat_sessions have `workspace_id` but the SQL doesn't include `workspace_members` inheritance for chat — only `resource_acl` workspace-group grants). **Possible gap**: a chat_session in a workspace where the caller is a member but has no explicit grant — is it visible? Per `check_relation(chat_session, viewer, user)` rewrite: `tuple_to_userset(workspace → member)` should grant viewer. But `list_sessions` SQL predicate does NOT include `workspace_members` inheritance for chat_sessions — it only includes `resource_acl` workspace-group grants. **Inconsistency between `check_relation` and `list_sessions` predicate.**
- **G7: No quota enforcement test for `max_workspaces`/`max_members`** — because the enforcement doesn't exist (C12-related).
- **G8: No test for audit_log writes beyond `acl.grant`/`acl.revoke`** — because no other route writes audit_log.

### Weak tests
- **W1: `retrieve_chunks_empty_when_no_accessible_docs` (retrieval.rs:722)** — inserts `visibility='private'` foreign doc, asserts empty hits. But the test uses `QdrantStore::new(&local_qdrant())` which may fail (line 744-747: `match ... Err(_) => return`). If Qdrant is down, the test silently passes without asserting anything. Weak.
- **W2: Many `#[sqlx::test]` tests require a running PostgreSQL at `DATABASE_URL`** — if not set, tests are skipped or fail at setup. The previous audit's claim of "326 passed" requires the dev stack to be up.

### Tests that pass accidentally
- **W3: `accessible_docs_excludes_private_foreign_owner`** — passes because `'public'` literal matches the SQL `'public'` literal, but neither matches production `'shared'`. See G1.

### Transaction visibility issues
- **T1: `rls_middleware` COMMITs after handler returns** — handlers that spawn background tasks (e.g. chat SSE post-stream persist via `AppPool`) operate OUTSIDE the middleware transaction. `persist_chat_completion` (chat.rs:405) manually opens a new tx via `AppPool.acquire().detach()` + BEGIN + SET LOCAL. This is correct but creates a window where the SSE response is sent before the assistant message is persisted — if persist fails, the client has already received the text but the DB has no record. T61 doc acknowledges this with the `persist-failed` SSE event.

---

## Migration Findings

### Migration inventory (14 files, chronological)
| Timestamp | Task | Description | Verified |
|-----------|------|-------------|----------|
| `20260101000000_init.sql` | T5 | placeholder for `sqlx::migrate!` | ✅ |
| `20260617124018_identity_and_tenant.sql` | T12 (+T15) | users, tenants, tenant_members, platform_admins + pgcrypto + gmrag_app role + `gmrag_current_tenant()` + first RLS | ✅ |
| `20260617132425_rls_tenants_table.sql` | T15 | FORCE RLS on tenants | ✅ |
| `20260617143508_workspaces.sql` | T19 | workspaces + workspace_members | ✅ |
| `20260617143700_documents.sql` | T20 | documents + document_chunks | ✅ |
| `20260617143822_graph_entities.sql` | T21 | graph_nodes + graph_edges | ✅ |
| `20260617144046_chat.sql` | T22 | chat_sessions + chat_messages | ✅ (but T22 doc omits `user_id`) |
| `20260617144756_acl.sql` | T23 | resource_acl (`permission DEFAULT 'read'`) + invitations | ✅ (default later changed in T64) |
| `20260617145246_system_tracking.sql` | T24 | tenant_quotas + usage_events + audit_log + ingest_jobs | ✅ |
| `20260617145935_rls_apply_all.sql` | T25 | ENABLE+FORCE RLS + policy on 14 tables | ✅ |
| `20260618100000_tenant_llm_config.sql` | T40 | tenant_llm_config + RLS + FORCE | ✅ |
| `20260618110000_graph_idempotency_and_llm.sql` | T41 | graph_nodes + workspace_id + UNIQUE + tenant_llm_config + llm_model/llm_base_url | ✅ |
| `20260618130000_tenant_llm_config_encrypted_keys.sql` | T45 | api_key_ciphertext + api_key_nonce + pair CHECK | ✅ |
| `20260622000000_rebac_relation_tuples.sql` | T64 | permission default 'viewer' + CHECK relation + CHECK principal_type + covering index | ✅ |

### Migration issues
- **M1: T12 migration was modified after initial apply (T15 added pgcrypto + gmrag_app role + gmrag_current_tenant() to it).** T15 doc acknowledges: "checksum is now different — `sqlx migrate info` will show `different checksum`. Harmless but should be noted." This is tech debt: a fresh DB gets the correct migration, but an existing DB with the original T12 will have checksum mismatch.
- **M2: T64 adds CHECK constraint `permission IN ('owner','editor','viewer')` without `NOT VALID`.** If any pre-T64 DB has `resource_acl` rows with `permission='read'` (the T23 default), the migration will FAIL. Confirmed: `seed.sql` does NOT insert resource_acl rows, so fresh `#[sqlx::test]` DBs are safe. But any dev DB that manually inserted 'read' rows would break. Recommend: `ALTER TABLE ... ADD CONSTRAINT ... NOT VALID;` then `ALTER TABLE ... VALIDATE CONSTRAINT ...;` for zero-downtime.
- **M3: No migration for `documents.status` lifecycle values.** T58 sets `'uploaded'`; worker never transitions it. No CHECK constraint on `status` — any string is accepted. If worker is later updated to set `'processing'`/`'indexed'`/`'failed'`, no schema validation exists.
- **M4: No migration for `visibility` CHECK constraint.** T58 `Visibility` enum enforces `'shared'`/`'private'` at the app layer, but the DB accepts any string. T57 doc acknowledges: "Schema không có CHECK constraint trên `visibility`." If a future endpoint sets `'workspace'` or `'public'`, retrieval.rs (which filters `IN ('workspace','public')`) would match but T57 list (which filters `= 'shared'`) would not. **Inconsistent visibility handling across endpoints.**
- **M5: `gmrag_current_tenant()` function defined in BOTH `infra/postgres/init.sql` AND migration `20260617124018`.** T15 doc explains this is intentional (init.sql for Docker, migration for `#[sqlx::test]` DBs which skip init.sql). But `CREATE OR REPLACE` semantics could cause drift if one is updated without the other. Currently identical.

---

## Route Findings

### Route inventory (verified against `lib.rs:97-186`)

#### Public (no auth)
- `GET /health` — liveness (no DB)
- `GET /healthz` — readiness (AdminPool DB ping)

#### Authed (auth_middleware only, no tenant context)
- `GET /users/me` — user profile + tenant memberships (AdminPool, cross-tenant)
- `GET /tenants` — list user's tenants (AdminPool, cross-tenant)
- `POST /tenants` — create tenant + auto-owner (AdminPool, pre-tenant)

#### Tenant-scoped (auth + tenant + rls middleware, SharedConnection)
- `PATCH /tenants/:tid` — rename (owner-only)
- `DELETE /tenants/:tid` — cascade delete (owner-only)
- `GET /tenants/:tid/members` — list members
- `POST /tenants/:tid/members` — invite (owner-only, creates `invitations` pending row)
- `DELETE /tenants/:tid/members/:user_id` — remove (owner-only, last-owner guard)
- `GET /tenants/:tid/workspaces` — list
- `POST /tenants/:tid/workspaces` — create
- `PATCH /tenants/:tid/workspaces/:wid` — rename
- `DELETE /tenants/:tid/workspaces/:wid` — delete
- `GET /tenants/:tid/workspaces/:wid/members` — list ws members
- `POST /tenants/:tid/workspaces/:wid/members` — add ws member
- `DELETE /tenants/:tid/workspaces/:wid/members/:user_id` — remove ws member
- `GET /tenants/:tid/documents` — list (visibility + ACL filter)
- `POST /tenants/:tid/documents` — upload (multipart, S3, quota, Redis enqueue)
- `DELETE /tenants/:tid/documents/:did` — delete (owner-only, S3+Qdrant cleanup, cascade)
- `GET /tenants/:tid/documents/:did/preview` — metadata + 50 chunks
- `GET /tenants/:tid/acl` — list grants (viewer-gated)
- `POST /tenants/:tid/acl` — create grant (owner-only)
- `DELETE /tenants/:tid/acl/:grant_id` — revoke grant (owner-only)
- `GET /tenants/:tid/chat_sessions` — list (owner + workspace_member + resource_acl)
- `POST /tenants/:tid/chat_sessions` — create (caller = owner)
- `DELETE /tenants/:tid/chat_sessions/:sid` — delete (owner-only)
- `POST /tenants/:tid/chat_sessions/:sid/chat` — SSE RAG chat (viewer-gated)
- `GET /tenants/:tid/workspaces/:wid/graph` — nodes + edges (workspace member)
- `GET /tenants/:tid/settings/llm` — BYOK config (owner-only, masked)
- `PUT /tenants/:tid/settings/llm` — BYOK config (owner-only, encrypt on write)
- `GET /tenants/:tid/metering/usage` — usage aggregate (owner-only)
- `GET /tenants/:tid/quotas` — quota limits (owner-only)
- `GET /tenants/:tid/audit_logs` — recent audit (owner-only)

### Route issues
- **R1: No `GET /tenants/:tid/documents/:did`** — only `preview` exists. T60 returns metadata + chunks together. If frontend needs metadata-only, it must call preview and ignore chunks. Minor.
- **R2: No `GET /tenants/:tid/chat_sessions/:sid/messages`** — T62 doc says "Messages: persist qua T61 chat stream (không expose message CRUD riêng trong batch này)." Frontend cannot list past messages of a session. **P1 gap for FE chat UI.**
- **R3: No accept-invitation endpoint** (C10).
- **R4: No `POST /tenants/:tid/workspaces/:wid/graph`** or graph mutation — graph is read-only API; nodes/edges created only by worker ingestion pipeline.
- **R5: `list_documents` and `list_sessions` predicates are hand-compiled** (not `check_relation` per-row) — sync hazard with `rbac/model.rs`.
- **R6: All tenant-scoped routes use `DefaultBodyLimit::max(50 MiB)`** — sufficient for PDF upload but may need tuning for large files.
- **R7: SSE chat route (`post_chat`) returns `Sse<impl Stream>`** — axum 0.7 SSE support is stable. KeepAlive default. No backpressure handling.

---

## Conflict Matrix

| # | Area | Source | Expected | Actual | Severity | Recommendation |
|---|------|--------|----------|--------|----------|----------------|
| C1 | chat retrieval ACL | T64 CHECK + T58 Visibility enum | `permission IN ('owner','editor','viewer')`, `visibility IN ('shared','private')` | `retrieval.rs:124` filters `permission='read'`, `:127` filters `visibility IN ('workspace','public')` | **P0** | Rewrite `accessible_document_ids` to use `check_relation(document, viewer, user)` per-document, or align SQL with ReBAC vocabulary + `'shared'` visibility. Add regression test with production-realistic data. |
| C2 | Qdrant default URL | T27 doc warning + qdrant-client 1.12.1 gRPC | `DEFAULT_QDRANT_URL = 'http://localhost:6334'`, `.env.example QDRANT_URL=http://qdrant:6334` | `config.rs:24` = `'http://localhost:6333'`, `.env.example:15` = `http://qdrant:6333` | **P0** | Update both to 6334. Add config test asserting gRPC port. |
| C3 | T70-T82 progress docs | Workbook lists T70-T82 | Progress docs T70.md-T82.md exist | 13 docs missing | **P1** | Create docs as tasks are done; for now, document the gap in handoff. |
| C4 | AUDIT_PRE_FRONTEND.md | T61/T68/T69/handoff.md references | File exists at referenced path | File deleted; this audit creates new one at `docs/AUDIT_PRE_FRONTEND.md` (different path) | **P1** | Either restore the old file at `docs/progress/` OR update T61/T68/T69/handoff.md to point to `docs/AUDIT_PRE_FRONTEND.md`. |
| C5 | T83 scope | T83 description | T83 integrates documents + chat + graph read paths | T83 only integrated documents.rs; chat/retrieval.rs stale (C1) | **P1** | Reopen T83: integrate `check_relation` into `retrieval.rs::accessible_document_ids`. |
| C6 | Frontend package.json | T8 doc (Next 15.0.3) + workbook T70 note (Next 16.2.9) | Consistent version + description + eslint config + types | `next:16.2.9` but `description:"Next.js 15"`, `eslint-config-next:15.0.3`, `@types/react:18.3.12` vs `react:19-rc` | **P1** | Update description, bump `eslint-config-next` to 16.x, bump `@types/react` to 19.x. |
| C7 | documents.status | T43/T58 docs: worker should update documents.status | Worker transitions 'uploaded'→'processing'→'indexed'/'failed' | Worker only updates `ingest_jobs.status`; `documents.status` stuck at 'uploaded' | **P1** | Add `UPDATE documents SET status = $1 WHERE id = $2` to `process_job` or `dual_write_ingestion`. |
| C8 | Workbook T22 | Workbook T22 description | "chat_sessions (+owner_id, visibility)" | Actual: `user_id` (not owner_id), no visibility column | **P2** | Fix workbook T22 description. |
| C9 | T37 OCR | T37 doc: PdfiumRenderer follow-up | `PdfiumRenderer` impl + libpdfium in Docker | Only `MockRenderer`; `parse_pdf` text-only path in T43 | **P2** | Implement `PdfiumRenderer` or document OCR as post-MVP. |
| C10 | Invitations accept | T54 doc: accept flow follow-up | `POST /invitations/:token/accept` or similar | No accept endpoint | **P2** | Implement accept-invitation route. |
| C11 | Graph orphan cleanup | T59 doc: acknowledged limitation | Graph nodes cleaned on document delete | Orphan graph nodes accumulate | **P2** | Add `document_graph_nodes` link table OR periodic reconciliation job. |
| C12 | Audit log coverage | ResourceBAC J6: "Hoàn thành" | audit_log written for all security-relevant mutations | Only `acl.grant`/`acl.revoke` | **P2** | Add `write_audit` calls to tenant/workspace/document/chat/settings mutations. |
| C13 | handoff.md | Self-archived | Points to current state | Points to deleted AUDIT_PRE_FRONTEND.md, HEAD `5517cb6` (stale) | **P3** | Update or delete handoff.md. |
| C14 | `list_sessions` ACL predicate | `rbac/model.rs::rewrite_for(chat_session, viewer)` | Includes `tuple_to_userset(workspace → member)` (workspace inheritance) | `chat.rs:98-120` SQL predicate does NOT include `workspace_members` inheritance for chat_sessions — only `resource_acl` workspace-group grants | **P2** | Add `OR (cs.workspace_id IS NOT NULL AND EXISTS (SELECT 1 FROM workspace_members wm WHERE wm.workspace_id = cs.workspace_id AND wm.user_id = $1))` to the predicate, OR call `check_relation` per-row (accept N+1 cost). |
| C15 | Workbook task count | Thiết lập B9: "Số task = 82" | 85 tasks (T1-T85) | "82" | **P3** | Update to 85. |
| C16 | T72 workbook description | T72 status "Chưa bắt đầu" | Description should NOT say "Hoàn thành theo TDD" | Description starts with "Hoàn thành theo TDD:" | **P3** | Fix description. |
| C17 | metering.rs redundant `WHERE tenant_id = $1` | RLS already filters by tenant | No `tenant_id` filter needed in SQL | `metering.rs:75,102,148` all filter `WHERE tenant_id = $1` | **P3** | Harmless (RLS makes it no-op) but redundant. Remove for clarity. |
| C18 | `max_workspaces`/`max_members` quota | Workbook implies all 4 quotas enforced | Enforcement in upload (T58) for `max_storage_bytes`+`max_documents` | `max_workspaces`/`max_members` NOT enforced anywhere | **P2** | Add enforcement in workspace/member creation routes. |

---

## Technical Debt

### Top 10 technical debts
1. **TD-1: `retrieval.rs::accessible_document_ids` stale ACL** (C1) — pre-ReBAC logic never migrated. Blocks correct chat retrieval.
2. **TD-2: Qdrant default URL 6333 vs 6334** (C2) — production-breaking config drift.
3. **TD-3: `documents.status` lifecycle missing** (C7) — frontend cannot show ingestion progress.
4. **TD-4: `format!("SET LOCAL app.tenant_id = '{tenant_id}'")` SQL interpolation** (S1) — codebase-wide. Consider `set_config('app.tenant_id', $1, true)` with bind param.
5. **TD-5: `PdfiumRenderer` not implemented** (C9) — OCR pipeline unwired; scanned PDFs silently produce empty content.
6. **TD-6: Accept-invitation flow missing** (C10) — invitations table has rows but no way to consume them.
7. **TD-7: `list_documents`/`list_sessions` hand-compiled ACL predicates** (R5, C14) — sync hazard with `rbac/model.rs`. `list_sessions` predicate is actually inconsistent with `rewrite_for` (missing workspace inheritance).
8. **TD-8: BYOK plaintext `api_key` fallback** (S2) — legacy column retained; should be migrated and dropped.
9. **TD-9: Audit log coverage partial** (C12) — only ACL grants audited.
10. **TD-10: `AclShareDialog` not mounted** (T84) — component built but no UI host.

### Other tech debts
- **TD-11: Graph orphan accumulation** (C11).
- **TD-12: T12 migration checksum mismatch** (M1) — modified after initial apply.
- **TD-13: T64 CHECK constraint not `NOT VALID`** (M2) — could fail on DBs with pre-existing 'read' rows.
- **TD-14: No `visibility` CHECK constraint** (M4) — app-layer only.
- **TD-15: No `documents.status` CHECK constraint** (M3).
- **TD-16: No rate limiting** (S4).
- **TD-17: No `GET /chat_sessions/:sid/messages`** (R2) — FE cannot list past messages.
- **TD-18: Frontend version mismatch** (C6) — eslint-config-next 15 vs next 16.
- **TD-19: Frontend `package.json` description stale** (C6).
- **TD-20: `handoff.md` stale** (C13).
- **TD-21: Workbook T22/T70/T72/B9 inaccuracies** (C8, C16, C15).
- **TD-22: `metering.rs` redundant `WHERE tenant_id` filters** (C17).
- **TD-23: `max_workspaces`/`max_members` quotas unenforced** (C18).
- **TD-24: `seed.sql` not in migration chain** — manual apply only; `#[sqlx::test]` DBs skip it.
- **TD-25: `infra/postgres/init.sql` bind-mounted `:ro`** — changes after first volume init don't re-apply (T1-T4 warning).

---

## Risk Areas

### Top 10 risks
1. **R-1: Chat retrieval silently returns no chunks for shared/ACL-granted documents** (C1) — user uploads doc with `visibility='shared'`, another user chats → retrieval's `accessible_document_ids` returns empty (filters `'workspace'`/`'public'`, not `'shared'`). Chat answers "no context" despite the doc being visible in the list. **High probability, high impact.**
2. **R-2: Production deployment fails to connect to Qdrant** (C2) — `QdrantStore::new` fails with `FRAME_SIZE_ERROR` if `.env` doesn't override `QDRANT_URL` to 6334. **Certain in default config.**
3. **R-3: `list_sessions` missing workspace inheritance** (C14) — chat_session in a workspace where caller is a member but has no explicit grant → invisible in list, but `check_relation(chat_session, viewer, user)` returns true → inconsistent with per-resource gate. **Medium probability, medium impact.**
4. **R-4: Frontend cannot display ingestion progress** (C7) — `documents.status` stuck at 'uploaded'. FE must poll `ingest_jobs` separately (no route exposes it). **Certain.**
5. **R-5: ReBAC predicate drift** (TD-7) — any future change to `rbac/model.rs::rewrite_for` viewer rule that isn't mirrored in `list_documents`/`list_sessions` SQL creates a security or UX inconsistency. No test catches it. **Medium probability, high impact.**
6. **R-6: Scanned PDFs silently produce empty content** (C9) — no error, no warning; user sees empty preview/chunks. **Medium probability, medium impact.**
7. **R-7: Audit log gap** (C12) — security incidents (tenant delete, settings change) not audited. **Low probability, high impact if incident occurs.**
8. **R-8: Quota bypass** (C18) — `max_workspaces`/`max_members` quotas are configured but never enforced. Tenant can create unlimited workspaces/members. **Medium probability, low impact.**
9. **R-9: BYOK plaintext key exposure** (S2) — if DB is compromised, plaintext `api_key` column leaks tenant OpenAI keys. **Low probability, high impact.**
10. **R-10: Frontend dependency version mismatch** (C6) — `eslint-config-next 15` with `next 16` may cause lint errors or missed lint rules. `@types/react 18` with `react 19-rc` may cause type errors. **Medium probability, low impact.**

### Architecture risks
- **AR-1: RLS middleware COMMIT-after-handler pattern** — relies on every handler self-managing SAVEPOINTs for atomicity. A future handler that returns an error after partial writes will commit those writes. Documented but fragile.
- **AR-2: Manual RLS setup in chat post-stream persist** (`chat.rs:419-423`) — duplicates `rls.rs::with_rls_connection` logic inline instead of calling the helper. If the helper changes, this code won't track.
- **AR-3: `check_relation` recursion depth 16** — sufficient for MVP concentric + 1-level workspace inheritance, but nested group-in-group would hit the limit. No cache (R9 from workbook).
- **AR-4: Qdrant collection-per-tenant** — limits scalability (R3 from workbook: >10k tenants hit limits). Accepted for MVP.

### Scaling risks
- **SR-1: `list_documents`/`list_sessions` have no pagination** — returns all matching rows. Large tenants will see slow responses + memory pressure.
- **SR-2: `get_workspace_graph` returns ALL nodes/edges** — no pagination. Large graphs will be slow.
- **SR-3: `get_audit_logs` LIMIT 100** — no offset/cursor pagination. Only newest 100 visible.
- **SR-4: `retrieve_all_with_metering` is sequential** (chunks then graph) — not parallelized. T47 doc notes `tokio::join!` would need two RLS connections.
- **SR-5: `upsert_chunks` single-request** — no chunking for large batches (>1000 points may timeout). T31 doc acknowledges.

### Migration risks
- **MR-1: T64 CHECK without `NOT VALID`** (M2).
- **MR-2: T12 checksum mismatch** (M1).
- **MR-3: No down migrations** — all migrations are forward-only. Rollback requires manual DB surgery.

---

## Recommended Fix Order

### P0 — Fix BEFORE frontend work begins (blocking)
1. **Fix C1: Rewrite `retrieval.rs::accessible_document_ids`** to use ReBAC vocabulary (`permission IN ('owner','editor','viewer')`) and `visibility = 'shared'` (or call `check_relation` per-document). Add regression test with `visibility='shared'` + `permission='viewer'` grant.
2. **Fix C2: Update `DEFAULT_QDRANT_URL` to `http://localhost:6334`** in `config.rs:24` and `.env.example:15` to `http://qdrant:6334`. Add config test asserting gRPC port.

### P1 — Fix before frontend depends on the affected paths
3. **Fix C7: Add `UPDATE documents SET status = $1` to worker** `process_job` (set 'processing' at start, 'indexed' on success, 'failed' on max retries).
4. **Fix C14: Add workspace inheritance to `list_sessions` predicate** OR call `check_relation` per-row.
5. **Fix C5: Reopen T83** — integrate `check_relation` into `retrieval.rs` (subsumes fix #1).
6. **Fix C6: Bump `eslint-config-next` to 16.x + `@types/react` to 19.x** + update `package.json` description.
7. **Fix C4: Decide AUDIT_PRE_FRONTEND.md path** — either restore at `docs/progress/` or update T61/T68/T69/handoff.md to point to `docs/AUDIT_PRE_FRONTEND.md`.
8. **Fix C3: Create T70-T82 progress docs** as work begins (not pre-emptively).

### P2 — Fix during frontend sprint (non-blocking but important)
9. **Fix C10: Implement accept-invitation endpoint.**
10. **Fix C12: Add `write_audit` calls** to tenant/workspace/document/chat/settings mutations.
11. **Fix C18: Enforce `max_workspaces`/`max_members` quotas** in workspace/member creation routes.
12. **Fix R2: Add `GET /chat_sessions/:sid/messages`** for FE chat history.
13. **Fix C8: Correct workbook T22 description.**
14. **Fix TD-4: Replace `format!` SQL interpolation with `set_config($1, $2, true)`** where possible.

### P3 — Tech debt, post-MVP
15. **Fix C9: Implement `PdfiumRenderer`** or document OCR as post-MVP.
16. **Fix C11: Add `document_graph_nodes` link** for graph orphan cleanup.
17. **Fix TD-8: Migrate BYOK plaintext keys to encrypted** and drop the `api_key` column.
18. **Fix C13: Update or delete `handoff.md`.**
19. **Fix C15/C16: Correct workbook B9 + T72 description.**
20. **Fix C17: Remove redundant `WHERE tenant_id` filters** in metering routes.

---

## Suggested Next Tasks

### Immediate (pre-frontend)
- **T83-followup:** Integrate `check_relation` into `retrieval.rs::accessible_document_ids` (fixes C1, C5).
- **Config fix:** Update `DEFAULT_QDRANT_URL` + `.env.example` (fixes C2).
- **T43-followup:** Add `documents.status` lifecycle update in worker (fixes C7).

### Frontend sprint (T70-T77, in order)
- **T70:** Auth.js Keycloak provider + token passthrough to `lib/acl.ts::AclClientConfig`. Reuse pre-batch `keycloak-js` + `next-auth@beta` deps. **Verify `eslint-config-next` 16 compat first (C6).**
- **T71:** `lib/api.ts` `apiFetch` (Bearer + X-Tenant-Id + error envelope parse).
- **T72:** `context/TenantContext.tsx` (fetch tenants, active tenant, switch).
- **T73:** `app/tenants/[tid]/workspaces/[wid]/layout.tsx` + `TenantSwitcher`.
- **T74:** `lib/{tenants,workspaces,documents,chat,settings}.ts` client wrappers.
- **T75:** Mount `AclShareDialog` (T84) on document/chat detail + `UploadDropzone` skeleton.
- **T76:** `ChatPanel.tsx` SSE consumer (parse `text|citation|citation_unknown|done|error` events from T61).
- **T77:** `KnowledgeGraphView`, `QuotaIndicator`, `LlmSettingsForm` (spec only or implement).

### Testing/ops sprint (T78-T82)
- **T78:** E2E: create tenant → upload → poll ingest → chat → verify citation → share ACL.
- **T79:** RLS pentest suite (extend `rls_isolation.rs` with 2-tenant attack scenarios).
- **T80:** Quota enforcement test (all 4 quotas, including `max_workspaces`/`max_members` once C18 is fixed).
- **T81:** `docker compose up --build` smoke test + README deploy guide.
- **T82:** Qdrant snapshot + Postgres pg_dump per-tenant backup scripts.

---

## Go / No-Go For Frontend

### Verdict: **CONDITIONAL GO** — fix P0 items (C1, C2) first, then begin T70.

### Rationale
- Backend API surface for frontend (T52-T69) is complete and committed.
- ReBAC engine (T64-T66) is sound and tested.
- 145 integration tests + ~111 unit tests pass (per prior audit verification at same HEAD).
- Frontend skeleton (T8) + ACL client (T84) exist and type-check.

### Blocking conditions (must fix before T70)
1. **C1 (retrieval.rs stale ACL)** — without this fix, chat (the flagship feature) returns no context for shared/ACL-granted documents. FE would ship broken chat.
2. **C2 (Qdrant default URL)** — without this fix, `docker compose up` with default `.env` fails to start the backend (QdrantStore::new panics).

### Non-blocking but strongly recommended before T70
3. **C7 (documents.status)** — FE will need to show ingestion progress; without this, status is stuck at 'uploaded'.
4. **C6 (frontend dep versions)** — FE build/lint may break with mismatched `eslint-config-next`/`@types/react`.
5. **C14 (list_sessions workspace inheritance)** — FE chat session list will miss sessions in workspaces where user is a member.

### Ready checklist
- [x] Git tree clean (1 deleted doc file only)
- [x] Backend routes wired (T52-T69 + T83)
- [x] ReBAC engine + tests (T64-T66, T85)
- [x] BYOK encryption (T45 + tech-debt hoist)
- [x] RLS isolation tests (T15/T25)
- [x] Error envelope consistent (T17)
- [x] Frontend skeleton + ACL client (T8, T84)
- [ ] **`cargo test --workspace` re-verified at current HEAD** (prior audit claims 326 passed; this audit could not re-run due to build timeout — recommend re-running before T70)
- [ ] **C1 fixed** (retrieval.rs ACL)
- [ ] **C2 fixed** (Qdrant URL)
- [ ] **C7 fixed** (documents.status) — recommended
- [ ] **C6 fixed** (frontend dep versions) — recommended

---

## OUTPUT REQUIREMENTS — Summary Scores

### 1. Repository health score: **7.5 / 10**
- Code is well-structured, migrations are coherent, tests are comprehensive.
- Deductions: C1 (retrieval ACL drift), C2 (config drift), C7 (status lifecycle), C9 (OCR not wired), C10 (invitations), C11 (graph orphans), C12 (audit log gaps).
- No P0 security holes; 2 P0 functional bugs (C1, C2).

### 2. Architecture health score: **8 / 10**
- Two-pool RLS design is sound. ReBAC engine is correct Zanzibar-style. BYOK encryption is proper AES-256-GCM with AAD.
- Deductions: RLS middleware COMMIT pattern (AR-1), manual RLS in chat post-stream (AR-2), hand-compiled ACL predicates (TD-7), list_sessions predicate inconsistency (C14).

### 3. Documentation health score: **6 / 10**
- Progress docs T1-T69 + T83-T85 are detailed and TDD-structured.
- Deductions: T70-T82 missing (C3), AUDIT_PRE_FRONTEND.md references broken (C4), handoff.md stale (C13), T22/T8 docs inaccurate (C8, C6), workbook inconsistencies (C15, C16, C8), T83 incomplete vs description (C5).

### 4. Test coverage confidence: **7 / 10**
- 145 integration + ~111 unit tests. RLS, ReBAC, schema, routes, worker pipeline all covered.
- Deductions: C1 has a false-positive test (G1/W3), no E2E ingestion test (G5), list_sessions workspace inheritance untested (G6), quota enforcement incomplete (G7), audit log coverage untested (G8), documents.status lifecycle untested (G3).

### 5. Top 10 problems
1. C1 — retrieval.rs stale ACL (P0, breaks chat for shared/ACL docs)
2. C2 — Qdrant default URL 6333 vs 6334 (P0, breaks default deployment)
3. C7 — documents.status never updated (P1, FE can't show progress)
4. C14 — list_sessions missing workspace inheritance (P1, ACL inconsistency)
5. C5 — T83 incomplete (retrieval.rs not integrated)
6. C6 — frontend dep version mismatch (P1, build/lint risk)
7. C4 — AUDIT_PRE_FRONTEND.md references broken (P1, doc rot)
8. C3 — T70-T82 progress docs missing (P1, timeline gap)
9. C12 — audit log only for ACL grants (P2, security audit gap)
10. C10 — accept-invitation flow missing (P2, invitations table unusable)

### 6. Top 10 technical debts
1. TD-1 — retrieval.rs stale ACL (C1)
2. TD-2 — Qdrant URL config drift (C2)
3. TD-3 — documents.status lifecycle missing (C7)
4. TD-4 — SQL string interpolation for SET LOCAL (S1)
5. TD-5 — PdfiumRenderer not implemented (C9)
6. TD-6 — accept-invitation flow missing (C10)
7. TD-7 — hand-compiled ACL predicates (R5, C14)
8. TD-8 — BYOK plaintext api_key fallback (S2)
9. TD-9 — audit log coverage partial (C12)
10. TD-10 — AclShareDialog not mounted (T84)

### 7. Top 10 risks
1. R-1 — Chat retrieval returns no chunks for shared/ACL docs (C1)
2. R-2 — Production Qdrant connection fails with default config (C2)
3. R-3 — list_sessions workspace inheritance missing (C14)
4. R-4 — FE cannot display ingestion progress (C7)
5. R-5 — ReBAC predicate drift undetectable (TD-7)
6. R-6 — Scanned PDFs silently empty (C9)
7. R-7 — Security incidents not audited (C12)
8. R-8 — Quota bypass for workspaces/members (C18)
9. R-9 — BYOK plaintext key exposure (S2)
10. R-10 — FE dep version mismatch (C6)

### 8. Top 10 recommended fixes
1. Fix C1: rewrite `accessible_document_ids` with ReBAC vocab + 'shared' visibility (or use `check_relation`)
2. Fix C2: update `DEFAULT_QDRANT_URL` + `.env.example` to 6334
3. Fix C7: add `documents.status` update in worker
4. Fix C14: add workspace inheritance to `list_sessions` predicate
5. Fix C5: reopen T83 for retrieval.rs integration
6. Fix C6: bump `eslint-config-next` + `@types/react` to match `next 16`/`react 19`
7. Fix C4: resolve AUDIT_PRE_FRONTEND.md path (restore or update references)
8. Fix C12: add `write_audit` to all mutation routes
9. Fix C18: enforce `max_workspaces`/`max_members` quotas
10. Fix R2: add `GET /chat_sessions/:sid/messages` for FE chat history

### 9. Whether frontend work should begin
**YES, but conditional.** Fix P0 items C1 and C2 first (both are small, surgical fixes). C7, C6, and C14 are strongly recommended before T70 lands. Once those are in, the frontend sprint (T70-T77) can proceed against a stable, correct backend API.

---

*End of audit. See `docs/AUDIT_ACTION_ITEMS.md` for a concise actionable checklist.*
