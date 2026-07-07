# PHASE 3 — Cross-System Reconciliation (Postgres ↔ OpenFGA ↔ Qdrant)

Date: 2026-07-06

## 1. What was built

A **periodic drift reconciler** that treats Postgres as the source of truth,
compares it against OpenFGA and Qdrant, reports drift, and — only when
explicitly enabled — repairs it. Default mode is **dry-run / report-only**.
Auto-fix is opt-in and defaults to OFF everywhere (treated as a critical bug
if violated).

### Task 1 — OpenFGA reconciler (`api/src/reconcile/openfga.rs`)

- **Reuses** the tuple-derivation logic from `openfga_backfill.rs`, extracted
  into a shared library module `api/src/reconcile/backfill.rs`
  (`collect_tuples`). The existing `openfga_backfill` binary is now a thin
  caller of that module — no duplicated tuple derivation.
- Compares the expected structural tuple set (derived from Postgres) against
  the live tuple set in OpenFGA via a new
  `AuthorizationService::read_all_direct_relationships()` trait method
  (paginated OpenFGA `/read` with `continuation_token` loop; implemented for
  both `OpenFgaAuthorizationService` and `PgTestAuthorizationService`).
- Reports `missing_in_openfga`, `orphaned_in_openfga`, and `malformed` —
  counts + a bounded sample (first 50) per category.
- **Auto-fix** (only when `auto_fix=true`): writes missing tuples and deletes
  orphaned tuples via `write_relationships`, logging each write/delete with
  before/after state. When `auto_fix=false`: zero `write_relationships` /
  `delete_*` calls — verified by test.

### Task 2 — Qdrant reconciler (`api/src/reconcile/qdrant.rs`)

- Compares Qdrant points against Postgres `document_chunks` / `graph_nodes`
  provenance using two new `QdrantStore` scroll APIs:
  `scroll_chunk_refs(tenant_id)` and `scroll_graph_node_refs(tenant_id)`
  (paginated `ScrollPoints`, page size 256, looping on `next_page_offset`).
- Reports `orphaned_chunk_points`, `orphaned_graph_points`,
  `missing_chunk_points`, `missing_graph_points`, plus malformed counts —
  counts + bounded samples.
- **Auto-fix** (only when `auto_fix=true`): deletes orphaned chunk points via
  `delete_chunks_by_document` and orphaned graph points via
  `delete_graph_nodes` (reusing existing `QdrantStore` methods). **Missing
  points are NEVER auto-re-embedded in this phase** — report only, per the
  rules. When `auto_fix=false`: zero delete calls — verified by test.
- Only scans collections whose tenant still exists in Postgres; a collection
  whose tenant was deleted is a tenant-level orphan (tenant deletion already
  best-effort tears down collections in `tenants.rs`) and is logged + skipped
  rather than surfaced as per-point drift.

### Task 3 — Worker periodic loop + config + standalone binary

- **Config** (`core/src/config.rs`): `reconcile_interval_secs`
  (`GMRAG_RECONCILE_INTERVAL_SECS`, default 3600, clamped ≥1) and
  `reconcile_auto_fix` (`GMRAG_RECONCILE_AUTO_FIX`, default `false`). A
  config unit test asserts the default-OFF invariant + overrides + clamping.
- **Worker loop** (`worker/src/reconcile_loop.rs`): a testable
  `run_reconcile_loop(runner, interval, auto_fix, shutdown)` with a
  `ReconcileRunner` trait (real impl `RealReconcileRunner` delegates to
  `gmrag_api::reconcile::run_reconcile_once`). The loop uses a `biased`
  `select!` on shutdown vs. an interval ticker; an in-progress run is never
  abandoned. Spawned from `worker/src/lib.rs::run()` as a background task
  mirroring the retention loop, with a separate Ctrl-C shutdown.
- **Worker → api dependency**: `gmrag-worker` now depends on `gmrag-api` so
  the reconciler reuses the `AuthorizationService` trait +
  `OpenFgaAuthorizationService` + the shared `backfill` module. The
  dependency is acyclic (api → core only). Moving authz into a shared crate
  would be a large cross-cutting refactor risking Phase 1/2 territory
  (explicitly disallowed), so this reuse path was chosen.
- **Standalone binary** (`api/src/bin/reconcile_drift.rs`, target
  `reconcile_drift`): runs both reconcilers, default dry-run, `--fix` to
  enable auto-fix, optional `--only=openfga|qdrant`. Prints a structured JSON
  report to stdout; human-readable per-category summary to stderr.

## 2. Dry-run vs auto-fix gating

Auto-fix defaults to **OFF** in three independent places:

1. `GMRAG_RECONCILE_AUTO_FIX` env var → `Config.reconcile_auto_fix`
   (default `false`; only `"true"`/`"1"` enable it).
2. The worker loop passes `cfg.reconcile_auto_fix` into
   `run_reconcile_loop`, which forwards it to every `run_once` call.
3. Each reconciler takes an `auto_fix: bool` argument and **returns early
   with the report** (no write/delete calls) when it is `false`. The
   standalone binary only passes `true` when `--fix` is on the command line.

The critical invariant — *auto_fix=false never writes or deletes, even when
drift is present* — is asserted by three dedicated tests (one per subsystem
+ one end-to-end through the loop's flag plumbing).

## 3. Why orphan detection is NOT a set-difference (important)

This is the single most important design decision in this phase and is worth
calling out explicitly for future maintainers.

**The naive approach would be wrong and dangerous.** A set-difference
`orphaned = openfga_all − expected_structural` looks tempting but is
**catastrophically incorrect** for this system, because **dynamic ACL grants
(editor/viewer on a document or chat_session, created via the `/acl` API)
are stored ONLY in OpenFGA — there is no Postgres grants table.** The
`acl.rs` routes write the OpenFGA tuple plus an `audit_log` row, and nothing
else. So `expected` (the structural tuple set derived from membership /
resource rows) is a **strict subset** of what a healthy OpenFGA store
contains. Every legitimate dynamic grant is, by construction, "in OpenFGA
but not in `expected`."

If orphan detection used `live − expected`, every single live dynamic grant
would be flagged as orphaned. With auto-fix enabled, the reconciler would
then **delete every dynamic ACL grant in the system** — a mass, silent
revocation of all document/chat sharing. That is a data-loss bug an order of
magnitude worse than the drift this phase exists to fix.

**What this phase does instead: resource-existence-based orphan detection.**
A live tuple is orphaned **only when the Postgres entity it references no
longer exists** — i.e. its `object` (`type:uuid`) or its `user` / userset
principal resolves to an id absent from the live `documents` / `workspaces`
/ `tenants` / `chat_sessions` / `users` tables. This precisely catches the
failure mode this phase targets (a deleted document/workspace/tenant whose
OpenFGA cleanup failed mid-write, leaving tuples pointing at ghost
resources) while **preserving every dynamic grant on a live resource**.
Malformed tuples (unparseable `type:uuid`) are reported separately and never
auto-deleted, since their referenced entity cannot be verified.

This is verified by `openfga_reconcile_categorizes_and_preserves_dynamic_grants`:
the test seeds a dynamic `viewer` grant on a LIVE document alongside an
orphaned tuple on a GHOST document, and asserts the dynamic grant is NOT
flagged orphaned while the ghost-resource tuple IS.

(The same reasoning does not apply to Qdrant: every Qdrant chunk/graph point
has a `document_id`/`node_id` payload that corresponds to a Postgres row, so
orphan detection there is a straightforward payload-vs-table membership
check — no dynamic-only data lives in Qdrant.)

## 4. Test results (live stack)

All tests run against the live Docker stack (`gmrag-postgres16`,
`gmrag-qdrant`, `gmrag-openfga`, etc.), not `SQLX_OFFLINE`, not `--no-run`.

### New Phase 3 tests

```text
$ cargo test -p gmrag-api --test reconcile_openfga
test openfga_reconcile_categorizes_and_preserves_dynamic_grants ... ok
test openfga_reconcile_auto_fix_writes_missing_and_deletes_orphaned ... ok
test openfga_reconcile_dry_run_never_writes_or_deletes ... ok
test result: ok. 3 passed; 0 failed

$ cargo test -p gmrag-api --test reconcile_qdrant
test qdrant_reconcile_detects_orphans_and_missing_and_preserves_in_dry_run ... ok
test qdrant_reconcile_auto_fix_deletes_orphans_but_not_missing ... ok
test qdrant_reconcile_dry_run_never_deletes ... ok
test result: ok. 3 passed; 0 failed

$ cargo test -p gmrag-worker --test reconcile_loop
test reconcile_loop_runs_on_interval_and_respects_shutdown ... ok
test reconcile_loop_does_not_abandon_in_progress_run ... ok
test reconcile_loop_auto_fix_false_never_writes_through_reconciler ... ok
test result: ok. 3 passed; 0 failed

$ cargo test -p gmrag-core --lib config
test config::tests::config_reconcile_defaults_and_overrides ... ok
(10 config tests total — all ok)
```

### Existing tests — no regressions

The Phase 3 changes are additive (a new trait method with working impls, new
modules, new config fields with safe defaults). The full workspace compiled
cleanly (`cargo test --workspace --no-run` succeeded). Representative
regression checks on the subsystems touched by the trait change:

```text
gmrag_core (lib):           29 passed; 0 failed
gmrag_worker (lib):         58 passed; 0 failed
gmrag_api (lib, authz):      8 passed; 0 failed
acl_routes:                  7 passed; 0 failed   # uses PgTestAuthorizationService
documents_acl:               4 passed; 0 failed   # uses PgTestAuthorizationService
chat_routes:                 9 passed; 0 failed   # uses PgTestAuthorizationService
relay:                       1 passed; 0 failed
retention:                   5 passed; 0 failed
sweeper:                     5 passed; 0 failed
concurrency:                 3 passed; 0 failed
process_job_retry:           8 passed; 0 failed
qdrant_writer:               3 passed; 0 failed
select_embedder:             9 passed; 0 failed
select_graph_extractor:      7 passed; 0 failed
```

Phase 1/2 territory (`job.rs` retry logic, `sweeper.rs` claim logic,
`run_dispatcher` concurrency) was **not modified** — those tests pass
unchanged.

## 5. Sample real report (live dev stack, dry-run)

Run: `reconcile_drift` (no `--fix`) against the live dev stack. The report
correctly surfaces drift in the seed data (OpenFGA tuples not yet
backfilled; seed documents with Postgres chunks but no Qdrant points) while
making zero writes/deletes, and correctly skips stale Qdrant collections
whose tenants are absent from Postgres.

Human-readable summary (stderr):
```text
reconcile-drift: mode=DRY-RUN subsystem=both
openfga: missing=21 orphaned=0 malformed=0 written=0 deleted=0
qdrant: orphaned_chunks=0 orphaned_graph=0 missing_chunks=3 missing_graph=2 deleted_chunk_docs=0 deleted_graph_nodes=0
```

Structured report (stdout, JSON, abbreviated sample list):
```json
{
  "auto_fix": false,
  "mode": "dry-run",
  "openfga": {
    "auto_fix_ran": false,
    "deleted": 0,
    "malformed": { "count": 0, "sample": [] },
    "missing_in_openfga": {
      "count": 21,
      "sample": [
        "user:b1000000-0000-0000-0000-000000000003 owner tenant:a1000000-0000-0000-0000-000000000002",
        "user:b1000000-0000-0000-0000-000000000001 admin workspace:c1000000-0000-0000-0000-000000000001",
        "tenant:a1000000-0000-0000-0000-000000000001 tenant document:d1000000-0000-0000-0000-000000000002"
      ]
    },
    "orphaned_in_openfga": { "count": 0, "sample": [] },
    "written": 0
  },
  "qdrant": {
    "auto_fix_ran": false,
    "deleted_chunk_docs": 0,
    "deleted_graph_nodes": 0,
    "malformed_chunk_points": 0,
    "malformed_graph_points": 0,
    "missing_chunk_points": {
      "count": 3,
      "sample": [
        "document_id=d1000000-0000-0000-0000-000000000001 qdrant=0 postgres=2",
        "document_id=d1000000-0000-0000-0000-000000000002 qdrant=0 postgres=2",
        "document_id=53e88748-7dd6-49db-8ecd-5c670e0dbc32 qdrant=0 postgres=1"
      ]
    },
    "missing_graph_points": {
      "count": 2,
      "sample": [
        "node_id=34c95374-fb30-4c14-9198-5266b5401df1",
        "node_id=8007416c-58b8-409b-9b89-f36d2eea22fc"
      ]
    },
    "orphaned_chunk_points": { "count": 0, "sample": [] },
    "orphaned_graph_points": { "count": 0, "sample": [] }
  }
}
```

The non-zero `missing_in_openfga` (21) reflects seed-data structural tuples
not yet backfilled into OpenFGA — exactly the kind of drift this reconciler
exists to detect. The non-zero `missing_chunk_points` (3) reflects seed
documents with `document_chunks` rows but no Qdrant points. Both are
reported only; nothing was written or deleted (dry-run). Running with
`--fix` would write the 21 missing OpenFGA tuples and leave the missing
Qdrant points reported-only (no re-embed, per the phase rules).

## 6. Files

**New**: `api/src/reconcile/{mod,backfill,openfga,qdrant}.rs`,
`api/src/bin/reconcile_drift.rs`, `api/tests/reconcile_openfga.rs`,
`api/tests/reconcile_qdrant.rs`, `worker/src/reconcile_loop.rs`,
`worker/tests/reconcile_loop.rs`, `docs/PHASE3_REPORT.md`.

**Edited**: `core/src/config.rs` (reconcile config + test),
`core/src/qdrant/store.rs` (scroll APIs + ref types),
`core/src/qdrant/mod.rs` + `core/src/lib.rs` (re-exports),
`api/src/authz.rs` (`read_all_direct_relationships` trait method + 2 impls),
`api/src/bin/openfga_backfill.rs` (reuse extracted `backfill` module),
`api/src/lib.rs` (`pub mod reconcile`), `worker/Cargo.toml` (gmrag-api dep),
`worker/src/lib.rs` (spawn reconcile loop + re-export).

## 7. Rules compliance

- ✅ No outbox pattern for OpenFGA/Qdrant writes — periodic reconciler only.
- ✅ No auto re-embed of missing Qdrant points — report only.
- ✅ No changes to `job.rs` retry logic, `sweeper.rs` claim logic, or
  `run_dispatcher` concurrency (Phase 1/2 territory untouched).
- ✅ Auto-fix defaults to OFF everywhere; a default-ON violation is treated
  as a critical bug and is guarded by three tests.
- ✅ No deeper out-of-scope bugs surfaced during this phase.

## 4b. Full workspace run (single command, confirmation)

Run from `backend/` against the live Docker stack:
`$env:DATABASE_URL='postgres://gmrag:...@localhost:5432/gmrag'; cargo test --workspace`

`SQLX_OFFLINE` was not set, and `--no-run` was not used.

Result - **400 passed; 0 failed; 0 ignored** across 44 workspace test
targets (api + core + worker; unit tests, integration tests, zero-test
binary targets, and doc-tests).

This count includes all Phase 3 tests in the same single workspace run:
`reconcile_openfga` (3 passed), `reconcile_qdrant` (3 passed),
`reconcile_loop` (3 passed), and the core config reconcile test inside
`gmrag_core`'s 29 passed unit tests, alongside every pre-existing workspace
test. The run also built and executed the worker test targets with the new
`gmrag-worker` -> `gmrag-api` dependency graph in place.
