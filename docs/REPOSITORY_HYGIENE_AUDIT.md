# Repository Hygiene Audit

**Audit date:** 2026-06-23  
**Phase:** 1 — Audit only (no deletions performed)  
**Repository state:** Backend complete · OpenAPI complete · T84A/B/C complete · Frontend not started  
**Scope:** `backend/`, `frontend/`, `docs/`, `infra/`, migrations, `.sqlx/`, config  
**Method:** Full inventory + cross-reference grep across code, docs, Docker, and tests

---

## Executive Summary

The repository is **structurally clean** at the code level (~220 tracked files). No `target/`, `tmp/`, `.bak`, or orphan config directories were found. Clutter is concentrated in **`docs/progress/`** (~73 markdown files + 1 generated txt artifact), plus **12 git-uncommitted deletions** of obsolete BATCH summary files.

| Area | Verdict |
|------|---------|
| `backend/` | Clean — all modules wired, tests active, fixtures required |
| `frontend/` | Scaffold only — no clutter files; unused npm deps (not file clutter) |
| `infra/` | All 6 files referenced; `minio/init.sh` unwired (operational gap, not clutter) |
| `docs/` | High volume; several stale/duplicate-path artifacts |
| `scripts/`, `tests/`, `examples/`, `tmp/`, `old/`, `legacy/` | **Do not exist at repo root** |
| CI | **No `.github/workflows/`** — nothing to prune |

### Classification Key

| Class | Meaning |
|-------|---------|
| **A — Critical** | Never delete |
| **B — Important** | Possible archive |
| **C — Likely stale** | Review before any action |
| **D — Safe deletion candidate** | Meets strict safe-deletion rules (see below) |

---

## Critical Files

**Class A — never delete.**

### Application & Infrastructure

| Path | Role |
|------|------|
| `backend/migrations/` (14 SQL files) | Runtime migrations + all `#[sqlx::test]` suites |
| `backend/.sqlx/` (2 JSON cache files) | Required for `SQLX_OFFLINE` builds in `infra/backend.Dockerfile` |
| `backend/Cargo.lock` | Workspace lockfile |
| `frontend/pnpm-lock.yaml` | Frontend lockfile |
| `infra/docker-compose.yml` | Primary dev stack (9 services) |
| `infra/backend.Dockerfile` | Backend + worker image build |
| `infra/frontend.Dockerfile` | Frontend image build |
| `infra/postgres/init.sql` | Compose bind mount → `/docker-entrypoint-initdb.d/` |
| `infra/postgres/seed.sql` | `include_str!` in `backend/crates/api/tests/seed_verify.rs` |
| `infra/minio/init.sh` | Documented in `README.md` (unwired in compose — operational debt) |
| `.env.example` | Environment template |
| `.gitignore`, `.dockerignore` | Ignore rules |

### Active Documentation (pre-frontend SSOT)

| Path | Role |
|------|------|
| `docs/AUDIT_PRE_FRONTEND.md` | Full repo audit pass |
| `docs/AUDIT_ACTION_ITEMS.md` | Actionable checklist (P0–P3) |
| `docs/API_INVENTORY.md` | 34-endpoint API catalog (T84B) |
| `docs/FRONTEND_READINESS.md` | Pre-frontend Go/No-Go gate (T84B) |
| `docs/FRONTEND_ARCHITECTURE.md` | Frontend stack/auth/routes SSOT |
| `docs/T85_IMPLEMENTATION_PLAN.md` | 8-phase frontend delivery plan |
| `docs/PM_REBASE_T70_T82.md` | PM workbook alignment for T70–T82 |
| `docs/progress/T84A.md` | OpenAPI + Swagger UI deliverable |
| `docs/progress/T84B.md` | API contract audit deliverable |
| `docs/progress/T84C.md` | Frontend foundation readiness audit |
| `docs/5068.pdf` | Zanzibar/ReBAC reference — cited in `backend/crates/api/src/rbac/*.rs` |
| `docs/GMRAG2_Project_Management.xlsx` | PM source of truth |

### Tests (all active — classify only, do not remove)

**API integration tests** (`backend/crates/api/tests/` — 24 files):

- Schema guards: `schema_documents.rs`, `schema_graph.rs`, `schema_chat.rs`, `schema_acl.rs`, `schema_system.rs`, `schema_llm.rs`, `schema_rebac.rs`
- RLS / pool: `rls_isolation.rs`, `pool_role.rs`
- Seed: `seed_verify.rs`
- Route suites: `tenant_routes.rs`, `workspace_routes.rs`, `document_routes.rs`, `documents_acl.rs`, `graph_routes.rs`, `chat_routes.rs`, `metering.rs`, `metering_routes.rs`, `acl_routes.rs`, `settings_routes.rs`, `openapi.rs`
- ReBAC: `rbac_check.rs`, `rebac_e2e.rs`

**Worker integration tests** (`backend/crates/worker/tests/` — 4 files + 3 PDF fixtures):

- `select_embedder.rs`, `select_graph_extractor.rs`, `process_job_retry.rs`, `qdrant_writer.rs`
- Fixtures: `fixtures/sample.pdf`, `fixtures/scanned.pdf`, `fixtures/text_rich.pdf` — used by `pdf_parser.rs` via `include_bytes!`

**Inline unit tests:** ~30 `#[cfg(test)]` modules across `api`, `worker`, and `core` crates.

**Frontend tests:** None (expected pre-T85).

### Frontend (scaffold, not clutter)

| Path | Role |
|------|------|
| `frontend/components/AclShareDialog.tsx` | T84 ACL UI — unmounted, needed for T85 |
| `frontend/lib/acl.ts` | T84 ACL client — imported by `AclShareDialog.tsx` |
| `frontend/app/` | Next.js 16 skeleton (T8) |

---

## Historical Files

**Class B — important; possible archive.**

### Progress task reports (~71 files)

All `docs/progress/T*.md` files document completed TDD work (T1–T69, T83–T85, plus combined reports `T1_T4.md`, `T5_T7.md`, `T52_T54.md`). They are unreferenced by application code but serve as a valuable audit trail. **Forbidden to delete** per hygiene rules.

Notable subsets:

- **T1–T60:** Backend, infra, ingestion, and document pipeline tasks
- **T61–T69:** Chat, graph, ReBAC, settings, metering
- **T83–T85:** ReBAC integration, frontend ACL component, ReBAC E2E pentest
- **T84A/B/C:** Pre-frontend audit trilogy (also Class A critical)

### Incident and prep documents

| File | Purpose | Cross-references |
|------|---------|------------------|
| `docs/progress/HOTFIX_BATCH2B.md` | Post-Batch 2B incident log (3 broken states) | `T26.md`, `T52_T54.md`, `HOTFIX_PRE_BATCH5.md` |
| `docs/progress/HOTFIX_PRE_BATCH5.md` | Pre-Batch 5 clippy/DB password fix | `T39.md`, `T40.md`, `T43.md` |
| `docs/progress/PRE_BATCH2.md` | Pre-Batch 2 prep (Next.js bump, CVE) | None |
| `docs/progress/TECH_DEBT_PRE_SPRINT7.md` | Tech debt payoff DEBT-1..4 | None |
| `docs/progress/T84.md` | Frontend ACL component (T84 task) | None — **naming collision** with T84A/B/C trilogy |

### Non-markdown historical assets

| File | Purpose |
|------|---------|
| `docs/5068.pdf` | ReBAC design reference (also Class A — code-cited) |
| `docs/GMRAG2_Project_Management.xlsx` | PM workbook (also Class A) |

**Recommended disposition:** Keep in place. Optionally index from `docs/archive/README.md` without moving `T*.md` files.

---

## Obsolete Files

**Class C — likely stale; not safe to delete without review.**

| File | Issue | Why not safe-delete |
|------|-------|---------------------|
| `docs/progress/handoff.md` | Self-marked ARCHIVED/STALE; header says HEAD `3120147` but body still claims `5517cb6` and uncommitted ReBAC work; references missing `rebac_authorization_(t61+)_55f68080.plan.md` | Referenced by `docs/AUDIT_PRE_FRONTEND.md` (C13) and `docs/AUDIT_ACTION_ITEMS.md` |
| `docs/progress/docker-compose-config.txt` | Generated `docker compose config` dump (408 lines) with Windows absolute paths | Referenced by `docs/progress/T1_T4.md` and `docs/progress/T5_T7.md` |
| `docs/progress/AUDIT_PRE_FRONTEND.md` | **Already deleted** in working tree | Superseded by `docs/AUDIT_PRE_FRONTEND.md`; T61/T68/T69 cite ambiguous path (audit C4) |
| `docs/progress/BATCH1_SUMMARY.md` | **Already deleted** in working tree | Superseded by individual `T*.md`; 0 repo references |
| `docs/progress/BATCH2A_SUMMARY.md` | **Already deleted** | Same |
| `docs/progress/BATCH2B_SUMMARY.md` | **Already deleted** | Same |
| `docs/progress/BATCH3_SUMMARY.md` | **Already deleted** | Same |
| `docs/progress/BATCH4_SUMMARY.md` | **Already deleted** | Same |
| `docs/progress/BATCH5A_SUMMARY.md` | **Already deleted** | Same |
| `docs/progress/BATCH5B_SUMMARY.md` | **Already deleted** | Same |
| `docs/progress/BATCH5C_SUMMARY.md` | **Already deleted** | Same |
| `docs/progress/BATCH6A_SUMMARY.md` | **Already deleted** | Same |
| `docs/progress/BATCH6B_SUMMARY.md` | **Already deleted** | Same |
| `docs/progress/BATCH6C_SUMMARY.md` | **Already deleted** | Same |

---

## Duplicate Files

No byte-identical duplicate files were found. The following are **naming collisions** (different content, same prefix):

| Collision | Files | Resolution |
|-----------|-------|------------|
| T84 vs T84A/B/C | `docs/progress/T84.md` (ACL UI component) vs `T84A.md` / `T84B.md` / `T84C.md` (pre-frontend audit trilogy) | Keep all; disambiguate in navigation/docs index |
| T85 vs T85 plan | `docs/progress/T85.md` (ReBAC E2E pentest — backend) vs `docs/T85_IMPLEMENTATION_PLAN.md` (frontend phases) | Keep both; different workstreams |
| Audit path | Old `docs/progress/AUDIT_PRE_FRONTEND.md` vs new `docs/AUDIT_PRE_FRONTEND.md` | Fix C4 refs in T61/T68/T69/handoff (edit paths, do not delete) |

---

## Unused Tests

**None identified.**

All 28 backend integration test files cover active behavior:

| Overlap | Explanation |
|---------|-------------|
| `document_routes.rs` + `documents_acl.rs` | Intentional layering: legacy visibility vs ReBAC grants |
| `metering.rs` + `metering_routes.rs` | Library write path vs HTTP read routes |
| `qdrant_writer.rs` | Env-dependent (requires live Qdrant at `localhost:6334`) — valid, not obsolete |

Frontend has zero test files — expected before T85 frontend implementation begins.

**Policy:** Do not remove tests solely because coverage overlaps.

---

## Stale Documents

| Document | Staleness | Recommended action (Pass 2+) |
|----------|-----------|------------------------------|
| `docs/progress/handoff.md` | Header partially updated; body contradicts (uncommitted work, old HEAD) | Archive or rewrite per audit C13 |
| `docs/progress/T61.md` | References `AUDIT_PRE_FRONTEND.md` without `docs/` prefix | Update to `docs/AUDIT_PRE_FRONTEND.md` (C4) |
| `docs/progress/T68.md` | Same path ambiguity | Update path (C4) |
| `docs/progress/T69.md` | Same path ambiguity | Update path (C4) |
| `README.md` | Only mentions `docs/progress/`; omits new critical docs | Add links to audit/architecture docs |
| Missing T70–T82 progress docs | 13 PM workbook tasks have no `T70.md`…`T82.md` | **Gap, not clutter** — create during frontend sprint |
| `docs/progress/T8.md` | Describes frontend as skeleton | Still accurate — no action |

---

## Dead Scripts

| Item | Status |
|------|--------|
| `scripts/` directory | **Never created** (noted in `docs/AUDIT_PRE_FRONTEND.md`) |
| `infra/minio/init.sh` | Exists, documented in `README.md`, **not wired in compose** — operational debt, not a dead file |
| `frontend/package.json` scripts | Active: `dev`, `build`, `start`, `lint` |
| `keycloak-js`, `next-auth` (npm deps) | **Zero imports** in TS/TSX — remove during T70 auth phase; not a file deletion |

No orphaned shell/Python scripts were found anywhere in the repository.

---

## Candidate Archives

Recommended structure for a future archive pass (requires explicit approval — **not executed in Phase 1**):

```
docs/archive/
├── progress-hotfix/
│   ├── HOTFIX_BATCH2B.md
│   ├── HOTFIX_PRE_BATCH5.md
│   ├── PRE_BATCH2.md
│   └── TECH_DEBT_PRE_SPRINT7.md
├── progress-evidence/
│   └── docker-compose-config.txt   # move only after updating T1_T4/T5_T7 refs
└── handoff/
    └── handoff-rebac-2026-06-22.md  # renamed archived copy of handoff.md
```

**Rules:**

- Keep all `T*.md` files in `docs/progress/` (forbidden to delete)
- Keep all `AUDIT*.md`, architecture docs, and implementation plans at current paths
- Add `docs/archive/README.md` index if archive directory is created

---

## Safe Deletion Candidates

Strict rules applied — a file may only be marked safe if it is:

- Not referenced, imported, linked, or documented elsewhere
- Not required by tests, CI, Docker, or migrations

### Files meeting all criteria

Only the **12 already-deleted** files in the working tree (pending git commit):

| File | Status |
|------|--------|
| `docs/progress/BATCH1_SUMMARY.md` | Deleted; 0 references; superseded by `T*.md` |
| `docs/progress/BATCH2A_SUMMARY.md` | Same |
| `docs/progress/BATCH2B_SUMMARY.md` | Same |
| `docs/progress/BATCH3_SUMMARY.md` | Same |
| `docs/progress/BATCH4_SUMMARY.md` | Same |
| `docs/progress/BATCH5A_SUMMARY.md` | Same |
| `docs/progress/BATCH5B_SUMMARY.md` | Same |
| `docs/progress/BATCH5C_SUMMARY.md` | Same |
| `docs/progress/BATCH6A_SUMMARY.md` | Same |
| `docs/progress/BATCH6B_SUMMARY.md` | Same |
| `docs/progress/BATCH6C_SUMMARY.md` | Same |
| `docs/progress/AUDIT_PRE_FRONTEND.md` (old path) | Deleted; superseded by `docs/AUDIT_PRE_FRONTEND.md` |

See [`DELETE_CANDIDATES.md`](./DELETE_CANDIDATES.md) for the full table with risk and confidence scores.

### Files explicitly NOT safe to delete

| File | Failing criterion |
|------|-------------------|
| `docs/progress/docker-compose-config.txt` | Linked from `T1_T4.md`, `T5_T7.md` |
| `docs/progress/handoff.md` | Linked from AUDIT docs |
| All `T*.md` progress files | Forbidden category |
| All `AUDIT*.md`, architecture, plan docs | Forbidden category |
| All backend tests, fixtures, migrations, `.sqlx/` | Required by runtime/build/tests |

---

## Risk Assessment

| Risk level | Item | Mitigation |
|------------|------|------------|
| **HIGH** | Deleting any `T*.md` or migration file | Forbidden unless extreme evidence — none found |
| **HIGH** | Deleting `handoff.md` without archive | Loses ReBAC decision context; archive first |
| **MEDIUM** | Deleting `docker-compose-config.txt` | Breaks T1_T4/T5_T7 evidence links; update refs first |
| **MEDIUM** | Removing `keycloak-js`/`next-auth` before T70 | Premature; wait for auth implementation |
| **LOW** | Committing already-deleted BATCH summaries | Safe — finalize git deletion only |
| **LOW** | Creating `docs/archive/` and moving HOTFIX docs | No code impact |

### Estimated cleanup impact

| Metric | Estimate |
|--------|----------|
| Disk recoverable | < 1 MB |
| File count (deletion commit) | −12 files |
| File count (archive pass) | −5 more if moved |
| Clarity gain | **High** — removes stale/conflicting docs from active navigation |
| Code/tests/infra changes | **None needed** — source trees already clean |
| `docs/progress/` volume after cleanup | ~73 markdown files remain (by design — TDD audit trail) |

---

## Pass 2 Summary (No Deletions Performed)

### 1. Files safe to delete immediately

Only the 12 already-deleted files pending git commit (listed in [Safe Deletion Candidates](#safe-deletion-candidates)).

### 2. Files recommended for archive

- `docs/progress/handoff.md`
- `docs/progress/docker-compose-config.txt` (after updating T1_T4/T5_T7 refs)
- `docs/progress/HOTFIX_BATCH2B.md`
- `docs/progress/HOTFIX_PRE_BATCH5.md`
- `docs/progress/PRE_BATCH2.md`
- `docs/progress/TECH_DEBT_PRE_SPRINT7.md`

### 3. Files requiring manual review

- Path drift: `docs/progress/T61.md`, `T68.md`, `T69.md` (audit C4 references)
- `README.md` (missing links to new critical docs)
- `frontend/package.json` unused deps (`keycloak-js`, `next-auth`) — dependency cleanup, not file deletion
- `infra/minio/init.sh` wiring — operational fix, not deletion
- T84 vs T84A/B/C naming confusion — documentation clarity only

### 4. Estimated repository cleanup impact

Minimal disk savings (< 1 MB); primary benefit is **navigation clarity**, not storage. Backend, frontend source, tests, migrations, and infra require no hygiene changes.

---

*Phase 1 complete. No files were deleted or moved during this audit. See [`DELETE_CANDIDATES.md`](./DELETE_CANDIDATES.md) for the deletion candidate table.*
