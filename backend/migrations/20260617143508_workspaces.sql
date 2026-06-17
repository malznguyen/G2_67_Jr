-- =========================================================
-- T19: workspaces + workspace_members.
-- A workspace is a tenant-scoped collaboration unit grouping documents.
-- RLS policies are applied in T25 (rls_apply_all), NOT here.
-- =========================================================

-- ---------- workspaces ----------
CREATE TABLE workspaces (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    name        TEXT        NOT NULL,
    slug        TEXT        NOT NULL,
    created_by  UUID        NOT NULL REFERENCES users(id),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, slug)
);

CREATE INDEX idx_workspaces_tenant ON workspaces (tenant_id);

-- ---------- workspace_members ----------
-- tenant_id is denormalized here so the uniform RLS policy
--   tenant_id = gmrag_current_tenant()
-- applies identically to every tenant-scoped table in T25.
CREATE TABLE workspace_members (
    workspace_id UUID        NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    tenant_id    UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    user_id      UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role         TEXT        NOT NULL DEFAULT 'member',
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, user_id)
);

CREATE INDEX idx_workspace_members_user ON workspace_members (user_id);

-- ---------- grants to the RLS-enforced app role ----------
GRANT SELECT, INSERT, UPDATE, DELETE ON workspaces         TO gmrag_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON workspace_members  TO gmrag_app;
