-- T41: GraphRAG idempotency + custom LLM endpoint overrides
-- =========================================================
-- 1. Add workspace_id to graph_nodes so nodes are scoped per workspace
--    (matching the Qdrant graph_{tenant_id} payload schema and RLS policy).
-- 2. Add UNIQUE(tenant_id, workspace_id, label, kind) for upsert-on-retry.
-- 3. Add llm_model + llm_base_url to tenant_llm_config for tenant-level
--    LLM overrides (DeepSeek / OpenAI BYOK for graph extraction / chat).
-- =========================================================

ALTER TABLE graph_nodes
    ADD COLUMN workspace_id UUID NULL REFERENCES workspaces(id) ON DELETE CASCADE;

CREATE INDEX idx_graph_nodes_workspace ON graph_nodes (workspace_id);

ALTER TABLE graph_nodes
    ADD CONSTRAINT graph_nodes_unique_workspace_label_kind
    UNIQUE (tenant_id, workspace_id, label, kind);

ALTER TABLE tenant_llm_config
    ADD COLUMN llm_model TEXT NULL,
    ADD COLUMN llm_base_url TEXT NULL;
