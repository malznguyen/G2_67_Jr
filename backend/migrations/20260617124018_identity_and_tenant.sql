-- =========================================================
-- T12: Identity & tenant domain tables.
-- Tables: users, tenants, tenant_members, platform_admins.
-- RLS is applied on tenant-scoped tables using gmrag_current_tenant().
-- =========================================================

-- ---------- RLS helper function ----------
-- Must exist before any RLS policy references it.
-- Also defined in infra/postgres/init.sql for Docker; repeated here
-- so that sqlx::test databases (which skip init.sql) work correctly.
CREATE EXTENSION IF NOT EXISTS "pgcrypto";

-- App role (idempotent — already exists in Docker via init.sql).
DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'gmrag_app') THEN
    CREATE ROLE gmrag_app NOLOGIN;
  END IF;
END
$$;

CREATE OR REPLACE FUNCTION gmrag_current_tenant()
RETURNS uuid
LANGUAGE sql
STABLE
AS $$
  SELECT NULLIF(current_setting('app.tenant_id', true), '')::uuid
$$;

-- ---------- users ----------
CREATE TABLE IF NOT EXISTS users (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    email       TEXT        NOT NULL UNIQUE,
    name        TEXT        NOT NULL DEFAULT '',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_users_email ON users (email);

-- ---------- tenants ----------
CREATE TABLE IF NOT EXISTS tenants (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    name        TEXT        NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ---------- tenant_members ----------
-- Maps users to tenants with a role (e.g. 'owner', 'admin', 'member', 'viewer').
CREATE TABLE IF NOT EXISTS tenant_members (
    tenant_id   UUID    NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    user_id     UUID    NOT NULL REFERENCES users(id)   ON DELETE CASCADE,
    role        TEXT    NOT NULL DEFAULT 'member',
    PRIMARY KEY (tenant_id, user_id)
);

CREATE INDEX IF NOT EXISTS idx_tenant_members_user ON tenant_members (user_id);

-- ---------- platform_admins ----------
-- Super-users who can manage all tenants. Not tenant-scoped.
CREATE TABLE IF NOT EXISTS platform_admins (
    user_id     UUID        PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    granted_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ---------- RLS on tenant-scoped tables ----------
-- Enable RLS and add policies that filter by gmrag_current_tenant().

ALTER TABLE tenant_members ENABLE ROW LEVEL SECURITY;

-- Policy: users can only see members of their current tenant.
CREATE POLICY tenant_members_isolation ON tenant_members
    USING (tenant_id = gmrag_current_tenant());

-- Grant table-level permissions to the app role.
GRANT SELECT, INSERT, UPDATE, DELETE ON users          TO gmrag_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON tenants        TO gmrag_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON tenant_members TO gmrag_app;
GRANT SELECT, INSERT, DELETE         ON platform_admins TO gmrag_app;
