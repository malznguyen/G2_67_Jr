-- =========================================================
-- T84D Phase 1.1 — Ingest outbox: transactional enqueue.
--
-- Race fix (P0): the RLS middleware owns BEGIN/COMMIT, so the upload
-- handler cannot LPUSH after commit. Instead the handler inserts a row
-- into `ingest_outbox` inside the same tx (atomic with documents /
-- ingest_jobs), and a worker relay task drains pending rows post-COMMIT
-- and LPUSHes them onto Redis (`gmrag:ingest_jobs`).
--
-- RLS mirrors the rest of the tenant-scoped tables (T25-style): the
-- `gmrag_app` role is enforced via FORCE ROW LEVEL SECURITY and the
-- policy filters by `gmrag_current_tenant()`.
-- =========================================================

CREATE TABLE ingest_outbox (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    document_id   UUID        NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    payload       JSONB       NOT NULL,
    status        TEXT        NOT NULL DEFAULT 'pending',
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    dispatched_at TIMESTAMPTZ NULL
);

-- Relay drains pending rows ordered by created_at; index supports the
-- `WHERE status='pending' ORDER BY created_at` hot path.
CREATE INDEX idx_ingest_outbox_status_created ON ingest_outbox (status, created_at);

ALTER TABLE ingest_outbox ENABLE ROW LEVEL SECURITY;
ALTER TABLE ingest_outbox FORCE  ROW LEVEL SECURITY;

CREATE POLICY ingest_outbox_isolation
    ON ingest_outbox
    USING (tenant_id = gmrag_current_tenant());

GRANT SELECT, INSERT, UPDATE, DELETE ON ingest_outbox TO gmrag_app;