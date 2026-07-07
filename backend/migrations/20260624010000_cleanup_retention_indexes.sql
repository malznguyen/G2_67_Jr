-- =========================================================
-- Phase 0 (TASK-P0-04): retention support indexes.
--
-- The worker retention loop deletes dispatched `ingest_outbox` rows
-- older than the configured retention window with:
--
--   WHERE status='dispatched'
--     AND dispatched_at IS NOT NULL
--     AND dispatched_at < now() - interval '<days> days'
--   ORDER BY dispatched_at
--   LIMIT $batch
--   FOR UPDATE SKIP LOCKED
--
-- `usage_events(created_at)` and `audit_log(created_at)` indexes already
-- exist (idx_usage_events_created / idx_audit_log_created in migration
-- 20260617145246). `ingest_outbox` has idx_ingest_outbox_status_created on
-- (status, created_at), but the retention hot path filters on
-- `dispatched_at`, not `created_at`, so add a partial index on
-- `dispatched_at` restricted to dispatched rows.
--
-- Forward-only, idempotent (CREATE INDEX IF NOT EXISTS).
-- =========================================================

CREATE INDEX IF NOT EXISTS idx_ingest_outbox_dispatched_at
    ON ingest_outbox (dispatched_at)
    WHERE status = 'dispatched' AND dispatched_at IS NOT NULL;
