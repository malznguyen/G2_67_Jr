-- =========================================================
-- G2_67_Jr — PostgreSQL bootstrap (init.sql)
-- Runs once on first container start (docker-entrypoint-initdb.d).
-- Creates roles, RLS scaffolding, and extension prerequisites.
-- Tenant data tables are created by sqlx migrations (later tasks).
-- =========================================================

-- Required extensions
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";
CREATE EXTENSION IF NOT EXISTS "pgcrypto";
CREATE EXTENSION IF NOT EXISTS "vector"; -- pgvector; installed if image supports it; no-op otherwise

-- OpenFGA uses a separate logical database in the same local Postgres service.
SELECT 'CREATE DATABASE openfga OWNER gmrag'
WHERE NOT EXISTS (SELECT 1 FROM pg_database WHERE datname = 'openfga')\gexec

-- Application role (least privilege for runtime)
DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'gmrag_app') THEN
    CREATE ROLE gmrag_app LOGIN PASSWORD 'gmrag_app_change_me';
  END IF;
END
$$;

GRANT CONNECT ON DATABASE gmrag TO gmrag_app;
GRANT USAGE ON SCHEMA public TO gmrag_app;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO gmrag_app;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT USAGE, SELECT ON SEQUENCES TO gmrag_app;

-- Helper: enforce tenant_id from current_setting('app.tenant_id')
-- Called by RLS policies on tenant-scoped tables.
CREATE OR REPLACE FUNCTION gmrag_current_tenant()
RETURNS uuid
LANGUAGE sql
STABLE
AS $$
  SELECT NULLIF(current_setting('app.tenant_id', true), '')::uuid
$$;

-- Reserved app settings namespace marker (used by backend TenantContext)
DO $$
BEGIN
  PERFORM 1 FROM pg_db_role_setting
   WHERE setdatabase = (SELECT oid FROM pg_database WHERE datname = current_database())
     AND setrole = 0
     AND setconfig::text LIKE '%app.tenant_id%';
EXCEPTION WHEN OTHERS THEN
  -- ignore; namespaces are created lazily on first SET
  NULL;
END
$$;

COMMENT ON FUNCTION gmrag_current_tenant() IS
  'Returns the tenant UUID bound to the current session by the backend TenantContext. RLS policies MUST use this.';
