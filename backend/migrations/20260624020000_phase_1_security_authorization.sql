-- =========================================================
-- Phase 1: Security and Authorization.
--
-- - Validate historical workspace creator tenant membership.
-- - Backfill creator workspace membership as privileged owner/admin.
-- - FORCE RLS on tenant_members.
-- - Add explicit tenant WITH CHECK expressions to every tenant-scoped policy.
-- =========================================================

DO $$
DECLARE
    invalid_count bigint;
BEGIN
    SELECT COUNT(*)
    INTO invalid_count
    FROM workspaces w
    WHERE NOT EXISTS (
        SELECT 1
        FROM tenant_members tm
        WHERE tm.tenant_id = w.tenant_id
          AND tm.user_id = w.created_by
    );

    IF invalid_count > 0 THEN
        RAISE EXCEPTION
            'Phase 1 workspace creator backfill blocked: % workspace row(s) have created_by users that are not tenant_members of the same tenant; manually repair workspaces.created_by or tenant_members before migrating',
            invalid_count;
    END IF;
END $$;

INSERT INTO workspace_members (workspace_id, tenant_id, user_id, role)
SELECT w.id, w.tenant_id, w.created_by, 'owner'
FROM workspaces w
ON CONFLICT (workspace_id, user_id) DO UPDATE
SET role = CASE
    WHEN workspace_members.role IN ('owner', 'admin') THEN workspace_members.role
    ELSE 'owner'
END;

ALTER TABLE tenant_members FORCE ROW LEVEL SECURITY;

ALTER POLICY tenant_isolation ON tenants
    USING (id = gmrag_current_tenant())
    WITH CHECK (id = gmrag_current_tenant());

ALTER POLICY tenant_members_isolation ON tenant_members
    USING (tenant_id = gmrag_current_tenant())
    WITH CHECK (tenant_id = gmrag_current_tenant());

ALTER POLICY workspaces_isolation ON workspaces
    USING (tenant_id = gmrag_current_tenant())
    WITH CHECK (tenant_id = gmrag_current_tenant());

ALTER POLICY workspace_members_isolation ON workspace_members
    USING (tenant_id = gmrag_current_tenant())
    WITH CHECK (tenant_id = gmrag_current_tenant());

ALTER POLICY documents_isolation ON documents
    USING (tenant_id = gmrag_current_tenant())
    WITH CHECK (tenant_id = gmrag_current_tenant());

ALTER POLICY document_chunks_isolation ON document_chunks
    USING (tenant_id = gmrag_current_tenant())
    WITH CHECK (tenant_id = gmrag_current_tenant());

ALTER POLICY graph_nodes_isolation ON graph_nodes
    USING (tenant_id = gmrag_current_tenant())
    WITH CHECK (tenant_id = gmrag_current_tenant());

ALTER POLICY graph_edges_isolation ON graph_edges
    USING (tenant_id = gmrag_current_tenant())
    WITH CHECK (tenant_id = gmrag_current_tenant());

ALTER POLICY graph_node_documents_isolation ON graph_node_documents
    USING (tenant_id = gmrag_current_tenant())
    WITH CHECK (tenant_id = gmrag_current_tenant());

ALTER POLICY chat_sessions_isolation ON chat_sessions
    USING (tenant_id = gmrag_current_tenant())
    WITH CHECK (tenant_id = gmrag_current_tenant());

ALTER POLICY chat_messages_isolation ON chat_messages
    USING (tenant_id = gmrag_current_tenant())
    WITH CHECK (tenant_id = gmrag_current_tenant());

ALTER POLICY resource_acl_isolation ON resource_acl
    USING (tenant_id = gmrag_current_tenant())
    WITH CHECK (tenant_id = gmrag_current_tenant());

ALTER POLICY invitations_isolation ON invitations
    USING (tenant_id = gmrag_current_tenant())
    WITH CHECK (tenant_id = gmrag_current_tenant());

ALTER POLICY tenant_quotas_isolation ON tenant_quotas
    USING (tenant_id = gmrag_current_tenant())
    WITH CHECK (tenant_id = gmrag_current_tenant());

ALTER POLICY usage_events_isolation ON usage_events
    USING (tenant_id = gmrag_current_tenant())
    WITH CHECK (tenant_id = gmrag_current_tenant());

ALTER POLICY audit_log_isolation ON audit_log
    USING (tenant_id = gmrag_current_tenant())
    WITH CHECK (tenant_id = gmrag_current_tenant());

ALTER POLICY ingest_jobs_isolation ON ingest_jobs
    USING (tenant_id = gmrag_current_tenant())
    WITH CHECK (tenant_id = gmrag_current_tenant());

ALTER POLICY tenant_llm_config_isolation ON tenant_llm_config
    USING (tenant_id = gmrag_current_tenant())
    WITH CHECK (tenant_id = gmrag_current_tenant());

ALTER POLICY ingest_outbox_isolation ON ingest_outbox
    USING (tenant_id = gmrag_current_tenant())
    WITH CHECK (tenant_id = gmrag_current_tenant());
