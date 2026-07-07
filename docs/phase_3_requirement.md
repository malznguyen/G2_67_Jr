# PHASE 3 — Cross-System Reconciliation for GMRAG v2 (Postgres ↔ OpenFGA ↔ Qdrant)

## Context
Phases 0–2 are complete and green (see PHASE0_VERIFICATION_REPORT.md,
PHASE0_5_REPORT.md, PHASE1_REPORT.md, PHASE2_REPORT.md if present). Worker
concurrency, retry caps, and sweeper lease-safety are fixed. This phase
addresses a different class of problem: Postgres, OpenFGA, and Qdrant are
written to as separate systems with no cross-system transaction, so a crash
or partial failure mid-write can leave them inconsistent (documented as P1
risks in the original handoff: "OpenFGA/Postgres consistency is not
transactional" and "Qdrant/Postgres dual-write orphan risk").

## Design decision (already made — implement this, do not re-litigate it)
Build a **periodic reconciler**, NOT a full outbox pattern for OpenFGA/Qdrant
writes. Postgres remains the source of truth. The reconciler compares
Postgres against OpenFGA and against Qdrant, reports drift, and — only when
explicitly enabled — repairs it. This is deliberately simpler than extending
the existing `ingest_outbox` pattern to cover OpenFGA/Qdrant; that tradeoff
has already been decided and should not be revisited in this phase.

Default mode is **dry-run/report-only**. Auto-fix must be opt-in via
explicit config/flag, never on by default.

## Task 1 — OpenFGA reconciler

- Reuse the tuple-construction logic already in
  `backend/crates/api/src/bin/openfga_backfill.rs` (it already knows how to
  derive expected tuples from tenants, workspace members, documents, chat
  sessions) rather than reimplementing tuple derivation from scratch.
- Add a comparison mode: for each expected tuple derived from current
  Postgres state, check whether it exists in OpenFGA (`list_objects` /
  batch `check` as appropriate); also detect tuples that exist in OpenFGA
  but have no corresponding live Postgres row (orphaned tuples — e.g. from
  a deleted document/workspace/tenant whose OpenFGA cleanup failed).
- Report format: counts of `missing_in_openfga`, `orphaned_in_openfga`, and
  a sample list (bounded, e.g. first 50) of specific tuple keys for each
  category, written to structured logs (and returned in the binary's
  output).
- When auto-fix is enabled: write missing tuples, delete orphaned tuples.
  Log every write/delete individually with before/after state.

## Task 2 — Qdrant reconciler

- Compare Postgres `document_chunks` / `graph_nodes` (with their
  provenance) against Qdrant points:
  - **Orphaned points**: Qdrant points whose `document_id` (or equivalent
    payload field) has no corresponding live document row in Postgres —
    these are candidates for deletion.
  - **Missing points**: documents with `status = 'indexed'` in Postgres
    that have fewer Qdrant points than expected chunks/graph nodes — these
    are candidates for re-ingestion (flag for now; actual re-embed trigger
    can just call into the existing worker ingestion path, or in this
    phase you can simply report them and mark them for manual/future
    re-ingest — do not build a new embedding pipeline here).
- Report format: same structure as Task 1 — counts + bounded sample list
  per category.
- When auto-fix is enabled: delete orphaned Qdrant points. For missing
  points, do NOT auto re-embed in this phase (that's a heavier, riskier
  write path) — just report them clearly so a human or a follow-up job can
  act.

## Task 3 — Wire into worker as a periodic background loop

- Add a new loop in `backend/crates/worker/src/lib.rs`, following the same
  pattern as the existing `retention.rs` background loop (periodic tick,
  graceful shutdown, logged start/end of each run).
- New config values in `core/src/config.rs`:
  - `RECONCILE_INTERVAL_SECONDS` (e.g. default 3600 — hourly)
  - `RECONCILE_AUTO_FIX` (boolean, default `false` — dry-run/report-only
    unless explicitly set true)
- Each tick runs both the OpenFGA and Qdrant reconcilers and logs a
  structured summary (counts per category, whether auto-fix ran). If
  `RECONCILE_AUTO_FIX=false`, the loop must only log/report — it must not
  write or delete anything, ever, in that mode.
- Also expose the reconcilers as standalone ops binaries (e.g.
  `reconcile-openfga`, `reconcile-qdrant`, or one combined
  `reconcile-drift` binary with subcommands) so they can be run manually
  with an explicit `--fix` flag, independent of the worker's periodic loop.

## Tests to add

- OpenFGA reconciler: unit/integration test with a seeded Postgres state
  and a mocked/test OpenFGA backend where some expected tuples are
  deliberately missing and some extra orphaned tuples exist; assert the
  report correctly categorizes both, and (separately) assert that with
  auto-fix enabled the missing tuples get written and orphaned ones get
  deleted, and with auto-fix disabled NOTHING is written or deleted.
- Qdrant reconciler: similar test with seeded Postgres + Qdrant test
  collection containing orphaned points and a document with intentionally
  fewer points than expected; assert correct categorization and the same
  auto-fix on/off behavior.
- Background loop test: confirm the loop runs on the configured interval,
  respects graceful shutdown (does not abandon an in-progress reconcile
  run), and that `RECONCILE_AUTO_FIX=false` never results in a write/delete
  call to OpenFGA or Qdrant even when drift is present in the test setup.

## Deliverable

- Code in `core/src/config.rs`, `worker/src/lib.rs`, a new reconciler
  module (e.g. `worker/src/reconcile/` or similar, your call on
  organization), and `api/src/bin/` (or `worker/src/bin/`) for the
  standalone ops binary/binaries. Reuse `openfga_backfill.rs` logic rather
  than duplicating tuple derivation.
- All new and existing tests passing: `cargo test --workspace` against the
  live stack (not `SQLX_OFFLINE`, not `--no-run`).
- A `PHASE3_REPORT.md` with: what was built, how dry-run vs auto-fix is
  gated, before/after test output, and a sample real report run against
  the current live dev stack (even if it shows zero drift — show the
  report format working end-to-end).

## Rules
- Do NOT build a full outbox pattern for OpenFGA/Qdrant writes — that
  tradeoff is already decided against, per the design decision above.
- Do NOT auto re-embed missing Qdrant points in this phase — report only.
- Do NOT change anything in Phase 1/2 territory (`job.rs` retry logic,
  `sweeper.rs` claim logic, `run_dispatcher` concurrency).
- Auto-fix must default to OFF. If this default is ever violated anywhere
  in the implementation, treat it as a critical bug in this phase, not a
  minor detail.
- If a test reveals a deeper bug outside this phase's scope, document it
  in the report instead of fixing it inline.