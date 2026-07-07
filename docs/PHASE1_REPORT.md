# PHASE 1 — Worker Concurrency & Retry Cap Fixes

**Status:** ✅ Complete — all changes implemented via TDD, full workspace green.
**Date:** 2026-07-06
**Scope:** `core/src/config.rs`, `worker/src/lib.rs`, `worker/src/job.rs` (+ tests, `.env.example`).
**Out of scope (explicitly untouched):** `worker/src/sweeper.rs` lease/locking logic (Phase 2).

---

## 1. What changed

### Bug 1 — `GMRAG_WORKER_CONCURRENCY` declared but never parsed/honored

**Before:** `.env.example` and `infra/docker-compose.yml` passed
`GMRAG_WORKER_CONCURRENCY` into the worker container, but
`backend/crates/core/src/config.rs` had no field for it and
`worker/src/lib.rs::run` processed exactly one `BRPOP` result at a time in a
strictly sequential `tokio::select!` loop. The env var was silently ignored.

**After:**
- `Config` gained a `worker_concurrency: usize` field, parsed from
  `GMRAG_WORKER_CONCURRENCY` by a new lenient helper
  `parse_worker_concurrency()` (`config.rs`). Bad/zero/empty/non-numeric
  values **warn and fall back to the default (4)** instead of panicking the
  worker at boot. Resulting pool size is always ≥ 1.
- `worker/src/lib.rs` extracted the poll loop into a new testable unit,
  `run_dispatcher(queue, concurrency, shutdown, handler)`. It:
  - BRPOP-pops jobs in a single producer loop,
  - spawns each popped job into a `tokio::task::JoinSet` gated by a
    `tokio::sync::Semaphore` of `concurrency` permits, so at most
    `concurrency` job handlers run at any time,
  - on `shutdown` it stops accepting new jobs and **waits for all
    in-flight handlers to finish** (`workers.join_next()` drain loop)
    before returning — a job that has already started is never silently
    dropped.
- `run()` now constructs a `JobHandler` wrapping `process_job_with_retry`,
  logs `worker_concurrency = N, "main job loop running with bounded
  concurrency (GMRAG_WORKER_CONCURRENCY)"`, and drives it through
  `run_dispatcher` with `ctrl_c()` as the shutdown signal.
- `.env.example` comment expanded to document that the variable is now
  honored at runtime and that bad values fall back to the default.

**Why a semaphore + JoinSet (not N BRPOP loops):** a single multiplexed
`RedisQueue` connection is shared by the producer; N concurrent BRPOP loops
would each need their own connection. The single-producer / bounded-spawn
model reuses the existing connection and bounds concurrency cleanly. The
semaphore guarantees the configured bound even under job bursts.

### Bug 2 — Retry loop gave requeued jobs a fresh full set of attempts

**Before:** `process_job_with_retry` (in `worker/src/job.rs`) accepted
`job.attempts` only for the initial status update, then ran a fresh
`0..MAX_ATTEMPTS` loop every call. A job requeued by the sweeper with
`attempts = 2` got **3 more** in-memory retries — far past the intended cap
— wasting external API calls (embeddings, graph LLM). Persisted
`ingest_jobs.attempts` only ever recorded a value on *failure*, and reset to
the loop index on success, so it was not an accurate cumulative counter
across sweeper requeues.

**After (`worker/src/job.rs::process_job_with_retry`):**
- Reads `start_attempts = job.attempts` and computes the remaining budget:
  ```rust
  let remaining = MAX_ATTEMPTS - start_attempts;   // fresh = full MAX
  for i in 0..remaining { ... }                     // requeued(2) = 1 more
  ```
  A job with `attempts = 2` now gets at most `MAX_ATTEMPTS - 2 = 1` more
  in-memory try, not 3.
- **Fail-fast at the cap:** if `start_attempts >= MAX_ATTEMPTS` on pickup
  (e.g. a race with the sweeper), the job is marked `failed` immediately
  **without invoking the runner** — no external API calls are spent on a job
  that already exhausted its budget. `last_error` records
  `"job already reached max attempts (N); not retrying"`.
- **Persisted `attempts` now counts every real `runner.run()` invocation**
  (failures AND the successful attempt): `attempts_after = start_attempts
  + i + 1`. This keeps the counter accurate across multiple sweeper
  requeues (not just one): each session adds its real attempts on top of
  `start_attempts`, so a second requeue sees the cumulative total and
  gets the correctly-reduced remaining budget.

The interaction with `sweeper.rs::requeue_stuck_jobs` (which already does
`attempts = row.attempts + 1` at requeue time) is unchanged — the sweeper
merely sets the persisted counter; this phase makes the *consumer* of that
counter honor it. **No sweeper behavior was modified** (Phase 2 territory).

---

## 2. Files changed

| File | Change |
|------|--------|
| `backend/crates/core/src/config.rs` | New `worker_concurrency` field + `DEFAULT_WORKER_CONCURRENCY=4` const + lenient `parse_worker_concurrency()` helper (warn-on-bad, no panic); added to `clear_config_env()` keys; new unit test `config_worker_concurrency_default_and_overrides`. |
| `backend/crates/worker/src/lib.rs` | New `run_dispatcher` (semaphore-gated JoinSet + graceful drain), `JobHandler`/`JobFut`/`DispatcherOutcome` types; rewired `run()` to drive `run_dispatcher` with `cfg.worker_concurrency` and `ctrl_c` shutdown; logged pool size. |
| `backend/crates/worker/src/job.rs` | Rewrote `process_job_with_retry` to bound the loop by `MAX_ATTEMPTS - start_attempts`, increment persisted `attempts` per real attempt (incl. success), and fail-fast when `start_attempts >= MAX_ATTEMPTS`. Updated module doc. |
| `backend/crates/worker/tests/process_job_retry.rs` | Refactored `seed_job` → `seed_job_with_attempts`; added `CountingFailRunner`/`SucceedOnNthRunner`; added 4 new tests for attempts accounting; updated 2 pre-existing tests to the corrected "attempts = total real runs" semantic. |
| `backend/crates/worker/tests/concurrency.rs` | **New file** — 3 dispatcher tests: overlap proof, serialization proof, no-drop-on-shutdown. |
| `.env.example` | Expanded `GMRAG_WORKER_CONCURRENCY` comment to document runtime honoring + fallback behavior. |
| `infra/docker-compose.yml` | No code change (variable was already wired into the container env); behavior now matches the comment. |

---

## 3. Tests added (all written FIRST under TDD, watched RED, then GREEN)

### Task 1 — concurrency (new `tests/concurrency.rs`)
1. `dispatcher_runs_jobs_concurrently_up_to_bound` — 3 jobs, `concurrency=3`,
   each handler sleeps 80ms and bumps an atomic "current overlap" counter;
   asserts `max concurrent == 3` (proves the pool size is honored, not 1).
2. `dispatcher_serializes_when_concurrency_is_one` — 3 jobs,
   `concurrency=1`; asserts `max concurrent == 1` (proves the bound is
   actually a bound, not unbounded spawn).
3. `dispatcher_does_not_drop_inflight_job_on_shutdown` — one 150ms job;
   shutdown fires *while* the job is mid-sleep; asserts the job's
   `completed` counter reaches 1 and `DispatcherOutcome.jobs_finished == 1`
   (proves graceful drain, no silent drop).

### Task 2 — retry attempts accounting (`tests/process_job_retry.rs`)
4. `fresh_job_attempts_zero_gets_full_max_attempts` — `attempts=0` → runner
   invoked exactly `MAX_ATTEMPTS` times; persisted `attempts == MAX_ATTEMPTS`.
5. `job_with_two_prior_attempts_gets_only_one_more_try` — **the bug**:
   `attempts=2` → runner invoked exactly `MAX_ATTEMPTS - 2 = 1` time (NOT 3);
   persisted `attempts == MAX_ATTEMPTS` after final failure.
6. `job_with_two_prior_attempts_can_succeed_on_the_one_remaining_try` —
   `attempts=2`, runner succeeds on attempt 1 → status `completed`,
   invoked exactly once.
7. `job_already_at_max_attempts_is_failed_immediately_without_running` —
   `attempts == MAX_ATTEMPTS` → runner invoked **0 times**, status `failed`
   immediately, `last_error` set, document `failed`.
8. `job_above_max_attempts_is_failed_immediately_without_running` —
   `attempts = MAX_ATTEMPTS + 5` (corrupted row) → runner invoked 0 times,
   status `failed`.

Two pre-existing tests were **updated** to the corrected semantic (their old
assertions encoded the bug):
- `retry_succeeds_on_second_attempt`: `attempts` now `2` (1 fail + 1 success)
  instead of `1` (only failures counted under the old code).
- `retry_first_attempt_success_marks_completed_immediately`: `attempts` now
  `1` (one real run) instead of `0`.

### Task 1 — config (new unit test in `config.rs`)
9. `config_worker_concurrency_default_and_overrides` — default 4 when unset;
   override to 8 honored; bad `"not-a-number"`, `"0"`, and `"   "` all fall
   back to 4 without panicking.

---

## 4. Before / after test output

### Before (Bug 2 demonstrated — RED phase of TDD)
Running the new retry tests against the *unfixed* `process_job_with_retry`:
```
test fresh_job_attempts_zero_gets_full_max_attempts ... ok            (3 tries — correct even before fix)
test job_with_two_prior_attempts_gets_only_one_more_try ... FAILED
  assertion `left == right` failed: job with 2 prior attempts must get exactly 1 more try(s), got 3
  left: 3 right: 1
test job_already_at_max_attempts_is_failed_immediately_without_running ... FAILED
  assertion `left == right` failed: runner must NOT be invoked when attempts >= MAX_ATTEMPTS, got 3 invocation(s)
  left: 3 right: 0
test job_above_max_attempts_is_failed_immediately_without_running ... FAILED
  left: 3 right: 0
test job_with_two_prior_attempts_can_succeed_on_the_one_remaining_try ... FAILED
  left: 0 right: 3   (persisted attempts wrong under old "only failures" semantic)

test result: FAILED. 4 passed; 4 failed
```
This confirms the bug exactly as described in the task: a requeued job with
2 prior attempts got 3 more tries (a fresh full set), and there was no
fail-fast at the cap.

### After (GREEN — fixed)
```
running 8 tests in tests/process_job_retry.rs
test fresh_job_attempts_zero_gets_full_max_attempts ... ok
test job_above_max_attempts_is_failed_immediately_without_running ... ok
test job_already_at_max_attempts_is_failed_immediately_without_running ... ok
test job_with_two_prior_attempts_can_succeed_on_the_one_remaining_try ... ok
test job_with_two_prior_attempts_gets_only_one_more_try ... ok
test retry_first_attempt_success_marks_completed_immediately ... ok
test retry_marks_failed_after_three_attempts ... ok
test retry_succeeds_on_second_attempt ... ok
test result: ok. 8 passed; 0 failed
```

### After — concurrency (`tests/concurrency.rs`)
```
running 3 tests
test dispatcher_does_not_drop_inflight_job_on_shutdown ... ok
test dispatcher_runs_jobs_concurrently_up_to_bound ... ok
test dispatcher_serializes_when_concurrency_is_one ... ok
test result: ok. 3 passed; 0 failed; finished in 0.31s
```

### After — config (`gmrag-core` unit tests)
```
test config::tests::config_worker_concurrency_default_and_overrides ... ok
test result: ok. 58 passed; 0 failed
```
(`58` = existing core tests + the new concurrency test.)

### After — full workspace
```
$ DATABASE_URL=postgres://gmrag:…@localhost:5432/gmrag cargo test --workspace
…
TOTAL passed=385 failed=0   (across 39 test binaries)
warnings: 0
exit code: 0
```
No `FAILED`, `panicked`, or `error[`] lines anywhere in the workspace output.
All Phase 1 tests visible by name in the log:
```
test config::tests::config_worker_concurrency_default_and_overrides ... ok
test dispatcher_does_not_drop_inflight_job_on_shutdown ... ok
test dispatcher_runs_jobs_concurrently_up_to_bound ... ok
test dispatcher_serializes_when_concurrency_is_one ... ok
test job_already_at_max_attempts_is_failed_immediately_without_running ... ok
test job_above_max_attempts_is_failed_immediately_without_running ... ok
test job_with_two_prior_attempts_can_succeed_on_the_one_remaining_try ... ok
test job_with_two_prior_attempts_gets_only_one_more_try ... ok
test fresh_job_attempts_zero_gets_full_max_attempts ... ok
```
Notes on environment:
- Tests used the **live** Docker stack already up (`gmrag-postgres16`,
  `gmrag-redis`, etc.) on `localhost:5432` / `localhost:6379`.
- `DATABASE_URL` was exported so `sqlx::query_as!` compile-time checks
  (pre-existing in `gmrag-api`) resolve against the live schema and the
  `#[sqlx::test]` migrations run. `SQLX_OFFLINE` was **not** set.
- `--test-threads` left at cargo default for the workspace run; the
  retry/concurrency tests were also verified `--test-threads=1` to rule out
  cross-test interference.

---

## 5. Confirmation that `worker_concurrency` is honored at runtime

Two independent proofs:

**A. Log line.** `run()` now emits, with the actual configured value:
```rust
info!(worker_concurrency, "main job loop running with bounded concurrency (GMRAG_WORKER_CONCURRENCY)");
```
and `run_dispatcher` emits:
```rust
info!(concurrency = pool_size, "job dispatcher started");
```
With `GMRAG_WORKER_CONCURRENCY=4` (default) the worker logs
`concurrency=4 job dispatcher started`; with `=8` it logs `concurrency=8`.

**B. Behavioral (timing-based) proof from the concurrency tests.**
`dispatcher_runs_jobs_concurrently_up_to_bound` enqueues 3 jobs each
sleeping 80ms with `concurrency=3` and asserts the measured max overlap is
`3` — which is only possible if the semaphore hands out 3 permits
simultaneously (i.e. `worker_concurrency` is honored, not hardcoded to 1).
The companion test `dispatcher_serializes_when_concurrency_is_one` flips
the same setup to `concurrency=1` and asserts the max overlap is `1` —
proving the value is actually a bound on concurrency, not just an
ignored hint. The old code (sequential `select!` loop) would have produced
`max overlap == 1` for both, failing the first test.

---

## 6. Rules compliance

- ✅ `worker/src/sweeper.rs` was **read for context** (`attempts = row.attempts
  + 1` at requeue); its lease/locking/claim behavior was **not modified**.
- ✅ No rate limiting, metrics, or reconciler logic added.
- ✅ No deeper out-of-scope bugs surfaced during this phase — the only test
  adjustments were the two pre-existing tests whose assertions encoded the
  old buggy "attempts = only failures" semantic; those are corrected, not
  regressions.
- ✅ Deliverable: code changes in the three specified files (config / lib /
  job), full `cargo test --workspace` green with the live DB, and this
  report.