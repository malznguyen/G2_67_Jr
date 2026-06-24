# Delete Candidates

**Audit date:** 2026-06-23  
**Phase:** 1 — Audit only  
**Policy:** Conservative. Only files meeting **all** safe-deletion rules are listed as LOW-risk delete candidates. Archive and manual-review items are listed separately.

**Safe-deletion rules:** Not referenced · not imported · not linked · not documented · not required by tests · not required by CI · not required by Docker · not required by migrations.

**Forbidden to delete (unless extreme evidence):** migrations · `T*.md` · `AUDIT*.md` · OpenAPI docs · architecture docs · implementation plans · docker files · CI configs · `.sqlx/` · backup scripts · deployment docs.

**No deletions were performed during this audit.**

---

## Safe Deletion Candidates

These files are **already deleted** in the working tree. Committing the deletion is the only recommended action.

| File | Reason | Risk | Confidence |
|------|--------|------|------------|
| `docs/progress/BATCH1_SUMMARY.md` | Already deleted; 0 repo references; superseded by individual `T*.md` progress reports | LOW | 95% |
| `docs/progress/BATCH2A_SUMMARY.md` | Already deleted; 0 repo references; superseded by individual `T*.md` progress reports | LOW | 95% |
| `docs/progress/BATCH2B_SUMMARY.md` | Already deleted; 0 repo references; superseded by individual `T*.md` progress reports | LOW | 95% |
| `docs/progress/BATCH3_SUMMARY.md` | Already deleted; 0 repo references; superseded by individual `T*.md` progress reports | LOW | 95% |
| `docs/progress/BATCH4_SUMMARY.md` | Already deleted; 0 repo references; superseded by individual `T*.md` progress reports | LOW | 95% |
| `docs/progress/BATCH5A_SUMMARY.md` | Already deleted; 0 repo references; superseded by individual `T*.md` progress reports | LOW | 95% |
| `docs/progress/BATCH5B_SUMMARY.md` | Already deleted; 0 repo references; superseded by individual `T*.md` progress reports | LOW | 95% |
| `docs/progress/BATCH5C_SUMMARY.md` | Already deleted; 0 repo references; superseded by individual `T*.md` progress reports | LOW | 95% |
| `docs/progress/BATCH6A_SUMMARY.md` | Already deleted; 0 repo references; superseded by individual `T*.md` progress reports | LOW | 95% |
| `docs/progress/BATCH6B_SUMMARY.md` | Already deleted; 0 repo references; superseded by individual `T*.md` progress reports | LOW | 95% |
| `docs/progress/BATCH6C_SUMMARY.md` | Already deleted; 0 repo references; superseded by individual `T*.md` progress reports | LOW | 95% |
| `docs/progress/AUDIT_PRE_FRONTEND.md` | Already deleted; superseded by `docs/AUDIT_PRE_FRONTEND.md` at new path | LOW | 90% |

---

## Archive Candidates (Not Safe to Delete)

Move to `docs/archive/` after explicit approval. Do not delete.

| File | Reason | Risk | Confidence |
|------|--------|------|------------|
| `docs/progress/handoff.md` | Self-marked ARCHIVED/STALE; body contradicts header; referenced by AUDIT docs (C13) | MEDIUM | 85% |
| `docs/progress/docker-compose-config.txt` | Generated compose config dump with machine-specific paths; linked from `T1_T4.md` and `T5_T7.md` | MEDIUM | 80% |
| `docs/progress/HOTFIX_BATCH2B.md` | Historical incident log; cross-referenced by other progress docs | LOW | 75% |
| `docs/progress/HOTFIX_PRE_BATCH5.md` | Historical incident log; cross-referenced by T39/T40/T43 | LOW | 75% |
| `docs/progress/PRE_BATCH2.md` | Historical prep notes; no code references | LOW | 70% |
| `docs/progress/TECH_DEBT_PRE_SPRINT7.md` | Historical tech-debt record; no code references | LOW | 70% |

---

## Manual Review Required (Do Not Delete)

| File / Item | Reason | Risk | Confidence |
|-------------|--------|------|------------|
| `docs/progress/T61.md` | References `AUDIT_PRE_FRONTEND.md` without path prefix — ambiguous after audit move (C4) | MEDIUM | 90% |
| `docs/progress/T68.md` | Same C4 path ambiguity | MEDIUM | 90% |
| `docs/progress/T69.md` | Same C4 path ambiguity | MEDIUM | 90% |
| `README.md` | Omits links to new critical docs (`AUDIT_*`, `FRONTEND_*`, `T85_IMPLEMENTATION_PLAN`) | LOW | 85% |
| `frontend/package.json` — `keycloak-js`, `next-auth` | Zero imports in TS/TSX; dependency cleanup deferred to T70 | MEDIUM | 80% |
| `infra/minio/init.sh` | Documented but not wired in compose — operational fix, not deletion | MEDIUM | 75% |
| `docs/progress/T84.md` vs `T84A/B/C.md` | Naming collision — documentation clarity, not deletion | LOW | 70% |

---

## Explicitly Forbidden / Not Candidates

The following were audited and **must not be deleted**:

| Category | Examples |
|----------|----------|
| Migrations | `backend/migrations/*.sql` (14 files) |
| Progress reports | All `docs/progress/T*.md` (~71 files) |
| Audit & architecture | `docs/AUDIT_*.md`, `docs/FRONTEND_*.md`, `docs/T85_IMPLEMENTATION_PLAN.md`, `docs/API_INVENTORY.md`, `docs/PM_REBASE_T70_T82.md` |
| OpenAPI | `backend/crates/api/src/openapi/` |
| Docker / infra | `infra/docker-compose.yml`, `infra/*.Dockerfile`, `infra/postgres/*` |
| SQLx offline cache | `backend/.sqlx/*.json` |
| Tests & fixtures | All `backend/crates/*/tests/` files and PDF fixtures |
| Code-cited reference | `docs/5068.pdf` |
| PM workbook | `docs/GMRAG2_Project_Management.xlsx` |

---

## Summary

| Category | Count | Action |
|----------|-------|--------|
| Safe to delete (commit pending deletions) | 12 | Finalize git commit when approved |
| Archive candidates | 6 | Move to `docs/archive/` after approval |
| Manual review | 7 | Edit paths / deps / docs — do not delete |
| Forbidden | All migrations, T*.md, AUDIT*, architecture, infra | Keep |

See [`REPOSITORY_HYGIENE_AUDIT.md`](./REPOSITORY_HYGIENE_AUDIT.md) for the full classification report.
