-- =========================================================
-- T23: resource_acl + invitations.
-- resource_acl: polymorphic ACL entries (resource_type + resource_id
--   paired with principal_type + principal_id + permission).
-- invitations: tenant/workspace invite tokens with status + expiry.
-- RLS policies are applied in T25 (rls_apply_all), NOT here.
-- =========================================================

-- ---------- resource_acl ----------
CREATE TABLE resource_acl (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    resource_type   TEXT        NOT NULL,
    resource_id     UUID        NOT NULL,
    principal_type  TEXT        NOT NULL,
    principal_id    UUID        NOT NULL,
    permission      TEXT        NOT NULL DEFAULT 'read',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (resource_type, resource_id, principal_type, principal_id, permission)
);

CREATE INDEX idx_resource_acl_tenant           ON resource_acl (tenant_id);
CREATE INDEX idx_resource_acl_resource         ON resource_acl (resource_type, resource_id);
CREATE INDEX idx_resource_acl_principal        ON resource_acl (principal_type, principal_id);

-- ---------- invitations ----------
CREATE TABLE invitations (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    workspace_id  UUID            NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    email         TEXT        NOT NULL,
    role          TEXT        NOT NULL DEFAULT 'member',
    token         UUID        NOT NULL DEFAULT gen_random_uuid(),
    status        TEXT        NOT NULL DEFAULT 'pending',
    invited_by    UUID        NOT NULL REFERENCES users(id),
    expires_at    TIMESTAMPTZ     NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    accepted_at   TIMESTAMPTZ     NULL
);

CREATE INDEX idx_invitations_tenant    ON invitations (tenant_id);
CREATE INDEX idx_invitations_token     ON invitations (token);
CREATE INDEX idx_invitations_email     ON invitations (email);
CREATE INDEX idx_invitations_workspace ON invitations (workspace_id);

-- ---------- grants to the RLS-enforced app role ----------
GRANT SELECT, INSERT, UPDATE, DELETE ON resource_acl  TO gmrag_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON invitations   TO gmrag_app;
