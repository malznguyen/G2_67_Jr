# AUDIT ACTION ITEMS — GMRAG2 Pre-Frontend

**Derived from:** `docs/AUDIT_PRE_FRONTEND.md` (same date, same HEAD `3120147`)
**Purpose:** Concise, actionable checklist. Each item maps to a Conflict ID (C#) from the full audit.

---

## P0 — BLOCKING (fix before T70 frontend work)

- [ ] **C1 — Rewrite `accessible_document_ids` (`backend/crates/api/src/chat/retrieval.rs:110-139`)**
  - Replace `ra.permission = 'read'` with `ra.permission IN ('owner','editor','viewer')` (per T64 CHECK).
  - Replace `d.visibility IN ('workspace','public')` with `d.visibility = 'shared'` (per T58 `Visibility` enum).
  - **Preferred:** refactor to call `check_relation(document, viewer, user)` per-document (subsumes C5).
  - Add regression test with `visibility='shared'` + `resource_acl` grant `permission='viewer'` (production-realistic data). Current test `accessible_docs_excludes_private_foreign_owner` uses `visibility='public'` which is a false positive.

- [ ] **C2 — Fix Qdrant default URL**
  - `backend/crates/core/src/config.rs:24`: `DEFAULT_QDRANT_URL = "http://localhost:6334"` (was 6333).
  - `.env.example:15`: `QDRANT_URL=http://qdrant:6334` (was 6333).
  - Update config test `cfg.qdrant.url == DEFAULT_QDRANT_URL` to assert 6334.

---

## P1 — Fix before FE depends on the affected path

- [ ] **C7 — Add `documents.status` lifecycle update in worker**
  - In `worker/src/job.rs::process_job`: set `status='processing'` at start (via new helper or extend `update_job_status`).
  - On success: `UPDATE documents SET status='indexed' WHERE id = $1`.
  - On max-retry failure: `UPDATE documents SET status='failed' WHERE id = $1`.
  - Add RLS-scoped tx (mirror `update_job_status` pattern).

- [ ] **C14 — Add workspace inheritance to `list_sessions` predicate**
  - `backend/crates/api/src/routes/chat.rs:98-120`: add `OR (cs.workspace_id IS NOT NULL AND EXISTS (SELECT 1 FROM workspace_members wm WHERE wm.workspace_id = cs.workspace_id AND wm.user_id = $1))` to the WHERE clause.
  - Add test: chat_session in workspace, caller is workspace member, no explicit grant → visible in list.

- [ ] **C5 — Reopen T83: integrate `check_relation` into `retrieval.rs`**
  - Subsumes C1 if done via `check_relation` per-document.
  - Update T83 progress doc to reflect retrieval.rs integration.

- [ ] **C6 — Fix frontend dependency versions**
  - `frontend/package.json`: bump `eslint-config-next` from `15.0.3` to `16.x`.
  - Bump `@types/react` from `18.3.12` to `19.x`, `@types/react-dom` from `18.3.1` to `19.x`.
  - Update `description` from "Next.js 15" to "Next.js 16".
  - Run `pnpm install` + `pnpm lint` + `npx tsc --noEmit` to verify.

- [ ] **C4 — Resolve AUDIT_PRE_FRONTEND.md path**
  - Option A: restore the old file at `docs/progress/AUDIT_PRE_FRONTEND.md` (recover from git: `git checkout HEAD -- docs/progress/AUDIT_PRE_FRONTEND.md`).
  - Option B: update T61.md, T68.md, T69.md, handoff.md to reference `docs/AUDIT_PRE_FRONTEND.md` (the new path created by this audit).
  - Recommended: Option B (this audit is the new source of truth).

- [ ] **C3 — Create T70-T82 progress docs as work begins**
  - Do not pre-create empty docs; create them when each task is implemented.

---

## P2 — Fix during frontend sprint

- [ ] **C10 — Implement accept-invitation endpoint**
  - New route: `POST /tenants/:tid/invitations/:token/accept` (or `POST /invitations/:token/accept`).
  - Verify token, check `status='pending'` + not expired, create `tenant_members` row, update `invitations.status='accepted'` + `accepted_at=now()`.
  - Add audit_log entry.

- [ ] **C12 — Add `write_audit` calls to mutation routes**
  - `routes/tenants.rs`: `tenant.create`, `tenant.update`, `tenant.delete`.
  - `routes/tenant_members.rs`: `member.invite`, `member.remove`.
  - `routes/workspaces.rs`: `workspace.create`, `workspace.update`, `workspace.delete`.
  - `routes/ws_members.rs`: `ws_member.add`, `ws_member.remove`.
  - `routes/documents.rs`: `document.upload`, `document.delete`.
  - `routes/chat.rs`: `chat_session.create`, `chat_session.delete`.
  - `routes/settings.rs`: `settings.update`.
  - Reuse `routes/acl.rs::write_audit` helper (extract to shared module if needed).

- [ ] **C18 — Enforce `max_workspaces`/`max_members` quotas**
  - `routes/workspaces.rs::create_workspace`: check `tenant_quotas.max_workspaces` vs `COUNT(*) FROM workspaces WHERE tenant_id = $1`.
  - `routes/tenant_members.rs::invite_member` (or accept flow): check `max_members`.
  - Return `ApiError::QuotaExceeded` (429) on violation.

- [ ] **R2 — Add `GET /tenants/:tid/chat_sessions/:sid/messages`**
  - List messages ordered by `created_at ASC`, paginated (LIMIT/OFFSET or cursor).
  - Gate with `check_relation(chat_session, viewer, user)`.

- [ ] **C8 — Fix workbook T22 description**
  - Change "chat_sessions (+owner_id, visibility) + chat_messages" to "chat_sessions (user_id, workspace_id nullable, model) + chat_messages".

- [ ] **TD-4 — Replace `format!` SQL interpolation with `set_config` bind param**
  - Where: `rls.rs:60,131`, `chat.rs:421`, `retrieval.rs:611`, `job.rs:257`.
  - Replace `sqlx::query(&format!("SET LOCAL app.tenant_id = '{tenant_id}'"))` with `sqlx::query("SELECT set_config('app.tenant_id', $1, true)").bind(tenant_id.to_string())`.
  - Verify RLS still enforces in tests.

---

## P3 — Tech debt, post-MVP

- [ ] **C9 — Implement `PdfiumRenderer`** (or document OCR as post-MVP)
  - Add `pdfium-render` crate to `worker/Cargo.toml`.
  - Download libpdfium binary in `infra/backend.Dockerfile` (from `bblanchon/pdfium-binaries`).
  - Implement `PageRenderer` for `PdfiumRenderer`.
  - Swap `parse_pdf` → `parse_pdf_with_ocr` in `job.rs::process_job`.

- [ ] **C11 — Add `document_graph_nodes` link table for graph orphan cleanup**
  - Migration: `CREATE TABLE document_graph_nodes (document_id UUID REFERENCES documents(id) ON DELETE CASCADE, node_id UUID REFERENCES graph_nodes(id), PRIMARY KEY (document_id, node_id))`.
  - `dual_write_ingestion`: insert links after graph_nodes upsert.
  - `delete_document`: delete graph_nodes where `id IN (SELECT node_id FROM document_graph_nodes WHERE document_id = $1)` if no other document references them.
  - Qdrant: delete graph points by node_id filter.

- [ ] **TD-8 — Migrate BYOK plaintext keys to encrypted + drop `api_key` column**
  - Migration: for each row with `api_key NOT NULL AND api_key_ciphertext IS NULL`, encrypt with `GMRAG_TENANT_KEY_ENCRYPTION_KEY` and populate ciphertext + nonce, then set `api_key = NULL`.
  - Migration: `ALTER TABLE tenant_llm_config DROP COLUMN api_key`.
  - Remove plaintext fallback from `worker/embedding.rs` + `worker/graph.rs` + `routes/settings.rs`.

- [ ] **C13 — Update or delete `handoff.md`**
  - Either delete or rewrite to reflect HEAD `3120147` + all T61-T85 committed + point to `docs/AUDIT_PRE_FRONTEND.md`.

- [ ] **C15 — Fix workbook `Thiết lập B9`**: "Số task = 82" → "85".

- [ ] **C16 — Fix workbook T72 description**: remove "Hoàn thành theo TDD:" prefix.

- [ ] **C17 — Remove redundant `WHERE tenant_id = $1` filters** in `routes/metering.rs:75,102,148` (RLS already filters).

---

## Verification commands (run after fixes)

```powershell
# Backend tests (requires dev stack: postgres + qdrant + redis)
$env:SQLX_OFFLINE='true'
$env:DATABASE_URL='postgres://gmrag:<pw>@localhost:5432/gmrag'
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings

# Frontend
cd frontend
pnpm install
pnpm lint
npx tsc --noEmit
pnpm build
```

---

## Go/No-Go gate

**Frontend work (T70) may begin AFTER:**
1. C1 is fixed and `cargo test -p gmrag-api --test chat_routes` + `cargo test -p gmrag-api --lib chat::retrieval` pass with production-realistic test data.
2. C2 is fixed and `cargo test -p gmrag-core` passes + manual `docker compose up backend` verifies Qdrant connection.
3. `cargo test --workspace` re-verified at the new HEAD (prior audit's 326 passed claim should be reconfirmed).

**Recommended before T70 but not blocking:**
- C7 (documents.status), C6 (FE deps), C14 (list_sessions inheritance).
