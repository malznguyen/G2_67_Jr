-- =========================================================
-- T24: tenant_quotas + usage_events + audit_log + ingest_jobs.
-- System-tracking tables for quotas, usage metering, audit trail, and
-- background ingestion job queue. All tenant-scoped (RLS in T25).
-- =========================================================

-- ---------- tenant_quotas ----------
-- One row per tenant; stores soft limits enforced by the application.
CREATE TABLE tenant_quotas (
    tenant_id         UUID        PRIMARY KEY REFERENCES tenants(id) ON DELETE CASCADE,
    max_documents     INT         NOT NULL DEFAULT 100,
    max_workspaces    INT         NOT NULL DEFAULT 10,
    max_storage_bytes BIGINT      NOT NULL DEFAULT 10737418240,  -- 10 GiB
    max_members       INT         NOT NULL DEFAULT 50,
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ---------- usage_events ----------
-- Append-only metering events (one row per unit of consumption).
CREATE TABLE usage_events (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    metric      TEXT        NOT NULL,
    delta       BIGINT      NOT NULL DEFAULT 1,
    metadata    JSONB           NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_usage_events_tenant    ON usage_events (tenant_id);
CREATE INDEX idx_usage_events_metric    ON usage_events (metric);
CREATE INDEX idx_usage_events_created   ON usage_events (created_at);

-- ---------- audit_log ----------
-- Immutable audit trail of tenant-scoped actions.
CREATE TABLE audit_log (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    actor_id    UUID            NULL REFERENCES users(id),
    action      TEXT        NOT NULL,
    resource_type TEXT          NULL,
    resource_id UUID            NULL,
    metadata    JSONB           NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_audit_log_tenant   ON audit_log (tenant_id);
CREATE INDEX idx_audit_log_actor    ON audit_log (actor_id);
CREATE INDEX idx_audit_log_created  ON audit_log (created_at);

-- ---------- ingest_jobs ----------
-- Background ingestion job queue (consumed by gmrag-worker).
CREATE TABLE ingest_jobs (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    document_id  UUID        NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    status       TEXT        NOT NULL DEFAULT 'pending',
    attempts     INT         NOT NULL DEFAULT 0,
    last_error   TEXT            NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_ingest_jobs_tenant  ON ingest_jobs (tenant_id);
CREATE INDEX idx_ingest_jobs_status  ON ingest_jobs (status);
CREATE INDEX idx_ingest_jobs_doc     ON ingest_jobs (document_id);

-- ---------- grants to the RLS-enforced app role ----------
GRANT SELECT, INSERT, UPDATE, DELETE ON tenant_quotas  TO gmrag_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON usage_events   TO gmrag_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON audit_log      TO gmrag_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON ingest_jobs    TO gmrag_app;
