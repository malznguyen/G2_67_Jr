-- =========================================================
-- T15: Enable RLS on the tenants table.
-- Only members of the current tenant can see its row.
-- FORCE ROW LEVEL SECURITY ensures even table owners are subject to RLS.
-- =========================================================

ALTER TABLE tenants ENABLE ROW LEVEL SECURITY;
ALTER TABLE tenants FORCE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON tenants
    USING (id = gmrag_current_tenant());
