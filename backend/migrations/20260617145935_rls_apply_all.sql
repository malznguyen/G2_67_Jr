-- =========================================================
-- T25: Enable + FORCE RLS and add isolation policies on every
-- tenant-scoped table created in T19-T24.
--
-- Policy: tenant_id = gmrag_current_tenant()
--   - gmrag_current_tenant() reads app.tenant_id (set by rls_middleware
--     via SET LOCAL app.tenant_id = '<uuid>').
--   - When app.tenant_id is unset/empty → returns NULL → no rows match.
--
-- FORCE ROW LEVEL SECURITY ensures even table OWNERS are subject to RLS.
-- The app pool (init_app_pool) downgrades to gmrag_app via after_connect,
-- so RLS is enforced at runtime. Tests use SET LOCAL ROLE gmrag_app.
--
-- NOTE: tenants + tenant_members already have RLS from T12/T15 — NOT
-- touched here (per R4 decision: only add policies for new tables).
-- =========================================================

-- ---------- T19: workspaces + workspace_members ----------
ALTER TABLE workspaces ENABLE ROW LEVEL SECURITY;
ALTER TABLE workspaces FORCE ROW LEVEL SECURITY;
CREATE POLICY workspaces_isolation ON workspaces
    USING (tenant_id = gmrag_current_tenant());

ALTER TABLE workspace_members ENABLE ROW LEVEL SECURITY;
ALTER TABLE workspace_members FORCE ROW LEVEL SECURITY;
CREATE POLICY workspace_members_isolation ON workspace_members
    USING (tenant_id = gmrag_current_tenant());

-- ---------- T20: documents + document_chunks ----------
ALTER TABLE documents ENABLE ROW LEVEL SECURITY;
ALTER TABLE documents FORCE ROW LEVEL SECURITY;
CREATE POLICY documents_isolation ON documents
    USING (tenant_id = gmrag_current_tenant());

ALTER TABLE document_chunks ENABLE ROW LEVEL SECURITY;
ALTER TABLE document_chunks FORCE ROW LEVEL SECURITY;
CREATE POLICY document_chunks_isolation ON document_chunks
    USING (tenant_id = gmrag_current_tenant());

-- ---------- T21: graph_nodes + graph_edges ----------
ALTER TABLE graph_nodes ENABLE ROW LEVEL SECURITY;
ALTER TABLE graph_nodes FORCE ROW LEVEL SECURITY;
CREATE POLICY graph_nodes_isolation ON graph_nodes
    USING (tenant_id = gmrag_current_tenant());

ALTER TABLE graph_edges ENABLE ROW LEVEL SECURITY;
ALTER TABLE graph_edges FORCE ROW LEVEL SECURITY;
CREATE POLICY graph_edges_isolation ON graph_edges
    USING (tenant_id = gmrag_current_tenant());

-- ---------- T22: chat_sessions + chat_messages ----------
ALTER TABLE chat_sessions ENABLE ROW LEVEL SECURITY;
ALTER TABLE chat_sessions FORCE ROW LEVEL SECURITY;
CREATE POLICY chat_sessions_isolation ON chat_sessions
    USING (tenant_id = gmrag_current_tenant());

ALTER TABLE chat_messages ENABLE ROW LEVEL SECURITY;
ALTER TABLE chat_messages FORCE ROW LEVEL SECURITY;
CREATE POLICY chat_messages_isolation ON chat_messages
    USING (tenant_id = gmrag_current_tenant());

-- ---------- T23: resource_acl + invitations ----------
ALTER TABLE resource_acl ENABLE ROW LEVEL SECURITY;
ALTER TABLE resource_acl FORCE ROW LEVEL SECURITY;
CREATE POLICY resource_acl_isolation ON resource_acl
    USING (tenant_id = gmrag_current_tenant());

ALTER TABLE invitations ENABLE ROW LEVEL SECURITY;
ALTER TABLE invitations FORCE ROW LEVEL SECURITY;
CREATE POLICY invitations_isolation ON invitations
    USING (tenant_id = gmrag_current_tenant());

-- ---------- T24: tenant_quotas + usage_events + audit_log + ingest_jobs ----------
ALTER TABLE tenant_quotas ENABLE ROW LEVEL SECURITY;
ALTER TABLE tenant_quotas FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_quotas_isolation ON tenant_quotas
    USING (tenant_id = gmrag_current_tenant());

ALTER TABLE usage_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE usage_events FORCE ROW LEVEL SECURITY;
CREATE POLICY usage_events_isolation ON usage_events
    USING (tenant_id = gmrag_current_tenant());

ALTER TABLE audit_log ENABLE ROW LEVEL SECURITY;
ALTER TABLE audit_log FORCE ROW LEVEL SECURITY;
CREATE POLICY audit_log_isolation ON audit_log
    USING (tenant_id = gmrag_current_tenant());

ALTER TABLE ingest_jobs ENABLE ROW LEVEL SECURITY;
ALTER TABLE ingest_jobs FORCE ROW LEVEL SECURITY;
CREATE POLICY ingest_jobs_isolation ON ingest_jobs
    USING (tenant_id = gmrag_current_tenant());
