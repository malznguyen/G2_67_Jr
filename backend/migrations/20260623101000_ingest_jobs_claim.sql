-- =========================================================
-- T84D Phase 1.2 — Job recovery sweeper.
--
-- P1: a worker can crash while a job is in `processing`, leaving it
-- stuck forever. The sweeper re-enqueues jobs whose `claimed_at` has
-- not been refreshed within the lease window. This migration adds the
-- `claimed_at` column the worker stamps every time it transitions a
-- job to `processing`, plus a partial index over the in-flight rows
-- (the only ones the sweeper scans).
-- =========================================================

ALTER TABLE ingest_jobs ADD COLUMN claimed_at TIMESTAMPTZ NULL;

CREATE INDEX idx_ingest_jobs_claim
    ON ingest_jobs (claimed_at)
    WHERE status = 'processing';