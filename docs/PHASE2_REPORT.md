# PHASE 2 — Sweeper Lease-Safe Fixes for GMRAG v2

**Status:** ✅ Complete — all three sweeper bugs fixed via TDD (RED → GREEN), full
`gmrag-worker` crate green with zero warnings, Phase 1 retry tests still green.
**Date:** 2026-07-06
**Scope:** `worker/src/sweeper.rs` only (+ new integration test file
`worker/tests/sweeper.rs`).
**Out of scope (explicitly untouched):** Phase 1's
`run_dispatcher`/concurrency logic in `worker/src/lib.rs` and
`process_job_with_retry`'s Phase 1 retry-budget logic in `worker/src/job.rs`.

---

## 1. What changed (`worker/src/sweeper.rs`)

`requeue_stuck_jobs_with_limit` was rewritten so the **claim, the attempts
increment, the `claimed_at`/status update, and (when exhausted) the terminal
`failed` marking all happen inside one transaction**, and the Redis `LPUSH`
only happens **after** that transaction commits. The old code did none of this.

### Bug 1 — Duplicate requeue (no row locking)

**Before:** the stuck-job selection was a plain
```sql
SELECT id, document_id, tenant_id, attempts
FROM ingest_jobs
WHERE status IN ('pending','processing')
  AND (claimed_at IS NULL OR claimed_at < now() - ($1 || ' seconds')::interval)
ORDER BY created_at
LIMIT $2
```
inside a tx. A plain `SELECT` in `READ COMMITTED` takes **no row lock**, so two
concurrent sweeper ticks (two replicas, or two overlapping ticks on a slow DB)
both read the same stuck row, both `LPUSH` it, and both `UPDATE` it — the job is
double-pushed to Redis and its `attempts` is bumped twice per sweep.

**After:** the same query now ends with
```sql
FOR UPDATE SKIP LOCKED
```
`FOR UPDATE` acquires a row-level lock on each selected row for the duration of
the tx; `SKIP LOCKED` makes a second tx that encounters an already-locked row
**skip it** instead of blocking. Two concurrent sweeps can therefore never
select the same row — the second sweep simply sees fewer (or zero) rows this
tick. The lock is held until `tx.commit()`, which now happens only after every
selected row has been claimed/failed inside the tx (see Bug 2).

### Bug 2 — `LPUSH` before DB commit (orphan Redis entries on crash)

**Before:** for each row the old code did, in order:
1. `queue.lpush(INGEST_JOBS_KEY, bytes).await?`  ← Redis first
2. `UPDATE ingest_jobs SET status='pending', claimed_at=NULL, attempts=$1 …`
3. `tx.commit()`

If the process crashed or the `UPDATE`/commit failed **after** the `LPUSH`, the
job was already enqueued in Redis but the DB row still looked stuck — the next
sweeper tick would requeue it **again**, compounding duplication. Additionally,
if the `LPUSH` itself failed, the `?` propagated the error immediately, the
`UPDATE` never ran, the tx rolled back, and the worker loop received an `Err`.

**After:** the order is inverted. For each selected row the sweeper now, **inside
the tx**:
- (cap branch) if `row.attempts + 1 > MAX_ATTEMPTS` → `UPDATE … SET
  status='failed', claimed_at=NULL, last_error=…` (+ mark the document
  `failed`), `continue` — **no `LPUSH` at all** (see Bug 3);
- (requeue branch) re-fetch the document row, build the payload, then
  `UPDATE ingest_jobs SET status='pending', claimed_at=now(), attempts=$1 …`
  and stash the serialized payload in a `pending_pushes: Vec`.

Only **after** `tx.commit()` succeeds does the sweeper iterate `pending_pushes`
and `LPUSH` each one. So:
- If the commit fails → no `LPUSH` happens. The job remains in its prior (stuck)
  state for the next sweeper tick to reconsider. From the DB's perspective a job
  that failed to be claimed this tick looks exactly like a job that was never
  picked up this tick (the tx rolled back, no partial writes).
- The requeue-branch `UPDATE` deliberately sets `claimed_at = now()` (NOT NULL)
  so the row is **not** immediately re-eligible for sweeping. This closes the
  post-commit-pre-`LPUSH` window.

**Task 2 tradeoff — `LPUSH` fails *after* a successful commit.** Two options were
listed: (a) leave the DB row claimed/requeued-looking and let a future sweeper
tick re-claim it once the lease looks stale again, or (b) revert the DB claim in
a compensating update so it is immediately eligible again.

**Picked: option (a).** Reason: it requires the smaller code change given the
existing lease/`claimed_at` model. The claim `UPDATE` already sets
`claimed_at = now()`; option (b) would need a *second* transaction issuing a
compensating `UPDATE … SET claimed_at = NULL` (more code, a second tx, and a new
failure mode of its own — what if the compensating update also fails?). With
option (a) the already-committed row is safe and self-healing: the failed
`LPUSH` is logged, the sweeper returns `Ok` (it must not abort the worker loop
on a Redis blip), and the row becomes re-eligible only after `claimed_at` ages
past the lease, at which point a future sweeper tick re-claims it with `FOR
UPDATE SKIP LOCKED` and re-pushes. That is safe recovery, not duplication — the
row lock prevents a concurrent double-claim, and the lease gap prevents an
immediate re-claim. The only cost is up to one lease interval of delay for that
one job, which is acceptable for a best-effort recovery path.

### Bug 3 — Sweeper requeues jobs already at/past `MAX_ATTEMPTS`

**Before:** the old code unconditionally did
`attempts = row.attempts + 1` and `LPUSH`ed, even when `row.attempts` was already
`>= MAX_ATTEMPTS`. It relied entirely on Phase 1's fail-fast-on-pickup
(`process_job_with_retry` marks the job `failed` without running it when
`start_attempts >= MAX_ATTEMPTS`) to catch exhausted jobs. That is a correct
safety net, but the sweeper was doing pointless work — a Redis push, a DB round
trip, and log noise — for a job it already knew was exhausted.

**After:** before requeuing, the sweeper checks
`if row_attempts + 1 > MAX_ATTEMPTS` (i.e. this requeue would push it over
budget). If so it marks the job `failed` **directly inside the claim
transaction** —
```sql
UPDATE ingest_jobs SET status='failed', claimed_at=NULL, last_error=$1, updated_at=now()
```
with `last_error =
"exhausted retries while stuck/sweeper-claimed (attempts=N, max=MAX_ATTEMPTS)"`,
also marks the document `failed` (so it is not orphaned in `processing`), and
does **not** `LPUSH`. No pointless Redis work for an already-exhausted job.

### Cleanup — unused import

The Phase 2 implementation had added
`use gmrag_core::status::{document as doc_status, ingest_job as job_status};`
but the fail/requeue branches used raw `'failed'`/`'pending'` string literals
(matching the sweeper's pre-existing literal style and the requeue branch), so
the import was unused and produced an `unused_imports` warning. Phase 1 achieved
zero warnings, so the unused import was **removed** to keep the crate
warning-free. No behavior change. (The canonical `status::*` constants are still
used by `job.rs::process_job_with_retry` and by the integration tests; the
sweeper's own SQL keeps its existing literal style.)

### Preserved semantic — "requeue consumes one attempt"

A normal requeue still sets `attempts = row.attempts + 1`, exactly as before.
A job that was merely stuck (worker crashed mid-processing, lease expired)
still consumes one unit of its retry budget for that cycle. Phase 1's
`process_job_with_retry` honors whatever `attempts` value it is handed as
`start_attempts`, so the sweeper-set `attempts` and Phase 1's remaining-budget
computation still line up (verified by the interaction test, §4 Test 5).

**Flag (not changed — product decision):** whether a *stuck-but-not-really-run*
cycle should consume an attempt is a defensible question — a job whose worker
crashed before doing any real work arguably "didn't attempt" anything. This
phase explicitly does **not** change that semantic (per the requirement). If the
team wants stuck-only cycles to be free, that is a separate product decision
affecting the retry-budget definition and should be made deliberately, not
mid-implementation.

---

## 2. Tests added (`worker/tests/sweeper.rs`)

All written first, watched RED against the old code, then GREEN against the fix.

1. `sweeper_concurrent_no_duplicate_requeue` — **Bug 1**. Seeds one stuck job,
   runs two sweeps concurrently with `tokio::join!` (each with its own
   `MockQueue`), asserts the two queues received **exactly one** `LPUSH` total
   and `n1 + n2 == 1`, and the DB reflects a single requeue (`attempts == 1`).
2. `sweeper_lpush_failure_leaves_row_claimed_for_next_tick` — **Bug 2 / Task 2
   tradeoff**. Uses a `FailingQueue` whose `LPUSH` always errors; asserts the
   sweeper returns `Ok` with `n == 0` (does not abort the worker loop) and the
   DB row is committed as `status='pending'`, `claimed_at IS NOT NULL`,
   `attempts == 1` — i.e. **option (a)**: claimed, not immediately re-eligible,
   re-swept after the lease.
3. `sweeper_requeues_job_one_below_cap` — **Bug 3 boundary**. `attempts ==
   MAX_ATTEMPTS - 1` → one more requeue is allowed; asserts `n == 1`, one
   `LPUSH`, payload `attempts == MAX_ATTEMPTS`, DB `attempts == MAX_ATTEMPTS`,
   `status == 'pending'`.
4. `sweeper_marks_failed_at_cap_without_pushing` — **Bug 3**. `attempts ==
   MAX_ATTEMPTS` (already exhausted) → sweeper marks `failed` directly, asserts
   `n == 0`, **zero** `LPUSH` calls, `status == 'failed'`, `attempts ==
   MAX_ATTEMPTS` (not incremented past the cap), `last_error` contains
   `"exhausted"`, `claimed_at IS NULL`, and the document is `failed`.
5. `sweeper_then_retry_total_attempts_never_exceeds_max` — **sweeper↔retry
   interaction**. Seeds a stuck job at `attempts == 1`; runs one sweeper requeue
   (→ `attempts == 2`, pushed) then feeds the pushed payload through
   `process_job_with_retry` with an always-failing runner; asserts the runner is
   invoked **exactly once** (`MAX_ATTEMPTS - 2 == 1` remaining), and the final DB
   state is `failed` with `attempts == MAX_ATTEMPTS` (2 from the sweeper + 1 from
   the retry session == `MAX_ATTEMPTS`). Confirms the sweeper-set `attempts` and
   Phase 1's `start_attempts` consumption line up — total real attempts never
   exceed `MAX_ATTEMPTS`.

---

## 3. Before / after test output

### Before — RED phase (new sweeper tests run against the *old* `sweeper.rs`)

The old (HEAD) `sweeper.rs` was temporarily restored (`git restore
--source=HEAD --worktree -- backend/crates/worker/src/sweeper.rs`) and the new
test binary compiled + run against it:
```
running 5 tests
test sweeper_lpush_failure_leaves_row_claimed_for_next_tick ... FAILED
test sweeper_concurrent_no_duplicate_requeue ... FAILED
test sweeper_marks_failed_at_cap_without_pushing ... FAILED
test sweeper_requeues_job_one_below_cap ... ok
test sweeper_then_retry_total_attempts_never_exceeds_max ... ok

failures:

---- sweeper_lpush_failure_leaves_row_claimed_for_next_tick stdout ----
thread 'sweeper_lpush_failure_leaves_row_claimed_for_next_tick' panicked at crates\worker\tests\sweeper.rs:169:10:
sweeper must return Ok even when LPUSH fails (option a): sweeper lpush: simulated redis unavailable

---- sweeper_concurrent_no_duplicate_requeue stdout ----
thread 'sweeper_concurrent_no_duplicate_requeue' panicked at crates\worker\tests\sweeper.rs:128:5:
assertion `left == right` failed: two concurrent sweeps must not double-push the same job (got 2 pushes, n1=1, n2=1)
  left: 2
 right: 1

---- sweeper_marks_failed_at_cap_without_pushing stdout ----
thread 'sweeper_marks_failed_at_cap_without_pushing' panicked at crates\worker\tests\sweeper.rs:226:5:
assertion `left == right` failed: exhausted job must not be re-enqueued
  left: 1
 right: 0

test result: FAILED. 2 passed; 3 failed; 0 ignored; 0 measured; 0 filtered out
```
This reproduces all three bugs exactly:
- **Bug 1** — two concurrent sweeps double-pushed the same job (`2 pushes,
  n1=1, n2=1`) because the plain `SELECT` took no row lock.
- **Bug 2** — on `LPUSH` failure the old code propagated the error (`?`) and
  rolled back, so the row was *not* committed as claimed and the sweeper did not
  return `Ok` (no option-a recovery).
- **Bug 3** — an exhausted job (`attempts == MAX_ATTEMPTS`) was re-enqueued
  anyway (`left: 1` push vs `right: 0` expected) instead of being marked failed.

(The two passing tests are the boundaries that behave identically under the old
and new code: one-below-cap requeues normally, and the single-threaded
sweeper→retry interaction is unaffected by the locking change.)

### After — GREEN (fixed `sweeper.rs`), `cargo test -p gmrag-worker`

```
test result: ok. 58 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out   (lib unit tests, incl. sweeper::tests::sweeper_payload_roundtrips_ingest_job)
test result: ok. 0 passed; 0 failed                                            (unittests src/main.rs)
test result: ok. 3 passed; 0 failed                                            (tests/concurrency.rs — Phase 1, no regression)
test result: ok. 8 passed; 0 failed; finished in 9.40s                         (tests/process_job_retry.rs — Phase 1, no regression)
test result: ok. 3 passed; 0 failed                                            (tests/qdrant_writer.rs)
test result: ok. 1 passed; 0 failed                                            (tests/relay.rs)
test result: ok. 5 passed; 0 failed                                            (tests/retention.rs)
test result: ok. 9 passed; 0 failed                                            (tests/select_embedder.rs)
test result: ok. 7 passed; 0 failed                                            (tests/select_graph_extractor.rs)

     Running tests\sweeper.rs
running 5 tests
test sweeper_lpush_failure_leaves_row_claimed_for_next_tick ... ok
test sweeper_then_retry_total_attempts_never_exceeds_max ... ok
test sweeper_requeues_job_one_below_cap ... ok
test sweeper_marks_failed_at_cap_without_pushing ... ok
test sweeper_concurrent_no_duplicate_requeue ... ok
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.47s

   Doc-tests gmrag_worker
test result: ok. 0 passed; 0 failed
```
**Worker crate total: 99 passed; 0 failed; 0 warnings.**

The Phase 1 retry tests (`process_job_retry.rs`, 8 passed) and concurrency
tests (`concurrency.rs`, 3 passed) are unchanged and green — confirming the
sweeper's `attempts` accounting and Phase 1's `start_attempts` consumption still
interlock correctly (also proven directly by Test 5).

### After — full workspace (`cargo test --workspace`)

`cargo test --workspace` against the live stack (`DATABASE_URL` →
`localhost:5432`, `SQLX_OFFLINE` **not** set, `--no-run` **not** used):
**390 passed; 0 failed; 0 warnings** across 40 test binaries. Phase 1 reported
385; the +5 are exactly the new Phase 2 sweeper tests — no regressions anywhere
in the workspace (api authz/openfga cutover tests included). Full per-binary
breakdown in §6.

---

## 4. Confirmation — sweeper + retry interaction (Test 5)

`sweeper_then_retry_total_attempts_never_exceeds_max` runs a job through one
sweeper requeue **then** through `process_job_with_retry`:

1. Stuck job seeded at `attempts == 1`, `status='processing'`, stale
   `claimed_at`.
2. Sweeper requeues it → `attempts` `1 → 2`, `status='pending'`,
   `claimed_at=now()`, one `LPUSH`; the pushed payload carries `attempts == 2`.
3. The worker pops that payload and runs `process_job_with_retry` with an
   always-failing runner. Phase 1 bounds the in-memory loop by
   `MAX_ATTEMPTS - start_attempts = 3 - 2 = 1` more try, so the runner is
   invoked **exactly once**, then the job is marked `failed` with persisted
   `attempts == MAX_ATTEMPTS` (`2` from the sweeper + `1` from the retry session).

Asserted and passing: runner invoked exactly once; final `status == 'failed'`;
final `attempts == MAX_ATTEMPTS`; document `failed`. **Total real attempts
across the sweeper requeue + the retry session never exceed `MAX_ATTEMPTS`.**

---

## 5. Rules compliance

- ✅ Code changes confined to `worker/src/sweeper.rs` (+ the new
  `worker/tests/sweeper.rs`). `worker/src/job.rs` was **not** modified —
  `process_job_with_retry`'s Phase 1 logic is untouched.
- ✅ `run_dispatcher`/concurrency logic in `worker/src/lib.rs` untouched.
- ✅ The "requeue consumes one attempt" semantic is **preserved**; the question
  of whether it *should* change is **flagged in §1**, not changed.
- ✅ No rate limiting, metrics, or cross-system (OpenFGA/Qdrant) reconciler
  logic added — that is Phase 3+.
- ✅ Tests run against the **live** Docker stack (`gmrag-postgres16`,
  `gmrag-redis`, …) on `localhost:5432` / `localhost:6379`; `DATABASE_URL`
  exported so `#[sqlx::test]` migrations run; `SQLX_OFFLINE` **not** set;
  `--no-run` **not** used for the final verification.
- ✅ Zero compiler warnings in the worker crate (unused import removed).

---

## 6. Full workspace results (`cargo test --workspace`)

Run: `DATABASE_URL=postgres://gmrag:…@localhost:5432/gmrag cargo test --workspace`
(live Docker stack: `gmrag-postgres16`, `gmrag-redis`, `gmrag-qdrant`,
`gmrag-minio`, `gmrag-ollama`, `gmrag-keycloak`, `gmrag-openfga` all healthy).

Result — **390 passed; 0 failed; 0 warnings** across 40 test binaries (api +
core + worker; unit + integration + doc-tests). No `FAILED`, `error[`, or
`panicked` lines anywhere in the output. Representative tail:
```
     Running tests\sweeper.rs (target\debug\deps\sweeper-ed7342a63532fb10.exe)
running 5 tests
test sweeper_requeues_job_one_below_cap ... ok
test sweeper_lpush_failure_leaves_row_claimed_for_next_tick ... ok
test sweeper_then_retry_total_attempts_never_exceeds_max ... ok
test sweeper_marks_failed_at_cap_without_pushing ... ok
test sweeper_concurrent_no_duplicate_requeue ... ok
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.42s

   Doc-tests gmrag_api      test result: ok. 0 passed; 0 failed
   Doc-tests gmrag_core     test result: ok. 0 passed; 0 failed
   Doc-tests gmrag_worker   test result: ok. 0 passed; 0 failed
```
Per-crate breakdown: `gmrag-api` 291 passed (103 lib unit + 188 integration
across 24 binaries), `gmrag-worker` 99 passed (incl. the 5 new sweeper tests),
doc-tests 0. Phase 1's 385 → 390 (+5 = the new sweeper tests); every previously
passing test still passes.

