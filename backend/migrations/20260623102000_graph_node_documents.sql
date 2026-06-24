-- =========================================================
-- T84D Phase 2.1 — graph_node_documents (graph provenance + ACL).
--
-- SEC-1 (P0): graph_nodes are deduped per (tenant, workspace, label,
-- kind) and SHARED across documents, so a single `document_id` column
-- on `graph_nodes` would not fit. A node is ACL-visible to a caller iff
-- ANY of its source documents is in the caller's
-- `accessible_document_ids`. This join table records that provenance:
-- every dual-write ingestion, after upserting each node, also inserts
-- `(node_id, document_id)` here (`ON CONFLICT DO NOTHING`) inside the
-- same Postgres tx — so provenance stays atomic with the node upsert.
--
-- RLS mirrors the rest of the tenant-scoped tables (T25-style): the
-- `gmrag_app` role is enforced via FORCE ROW LEVEL SECURITY and the
-- policy filters by `gmrag_current_tenant()`.
-- =========================================================

CREATE TABLE graph_node_documents (
    node_id     UUID        NOT NULL REFERENCES graph_nodes(id) ON DELETE CASCADE,
    document_id UUID        NOT NULL REFERENCES documents(id)  ON DELETE CASCADE,
    tenant_id   UUID        NOT NULL REFERENCES tenants(id)    ON DELETE CASCADE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (node_id, document_id)
);

CREATE INDEX idx_graph_node_documents_doc  ON graph_node_documents (document_id);
CREATE INDEX idx_graph_node_documents_node ON graph_node_documents (node_id);

ALTER TABLE graph_node_documents ENABLE ROW LEVEL SECURITY;
ALTER TABLE graph_node_documents FORCE  ROW LEVEL SECURITY;

CREATE POLICY graph_node_documents_isolation
    ON graph_node_documents
    USING (tenant_id = gmrag_current_tenant());

GRANT SELECT, INSERT, UPDATE, DELETE ON graph_node_documents TO gmrag_app;