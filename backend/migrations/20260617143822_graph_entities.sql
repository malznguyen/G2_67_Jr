-- =========================================================
-- T21: graph_nodes + graph_edges.
-- Knowledge graph entities for a tenant. Nodes represent concepts or
-- document-derived entities; edges represent typed relationships.
-- RLS policies are applied in T25 (rls_apply_all), NOT here.
-- =========================================================

-- ---------- graph_nodes ----------
CREATE TABLE graph_nodes (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    kind        TEXT        NOT NULL,
    label       TEXT        NOT NULL,
    properties  JSONB       NOT NULL DEFAULT '{}'::jsonb,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_graph_nodes_tenant ON graph_nodes (tenant_id);
CREATE INDEX idx_graph_nodes_kind   ON graph_nodes (kind);

-- ---------- graph_edges ----------
CREATE TABLE graph_edges (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    src_node_id UUID        NOT NULL REFERENCES graph_nodes(id) ON DELETE CASCADE,
    dst_node_id UUID        NOT NULL REFERENCES graph_nodes(id) ON DELETE CASCADE,
    kind        TEXT        NOT NULL,
    weight      REAL        NOT NULL DEFAULT 1.0,
    properties  JSONB       NOT NULL DEFAULT '{}'::jsonb,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (src_node_id, dst_node_id, kind)
);

CREATE INDEX idx_graph_edges_tenant    ON graph_edges (tenant_id);
CREATE INDEX idx_graph_edges_src       ON graph_edges (src_node_id);
CREATE INDEX idx_graph_edges_dst       ON graph_edges (dst_node_id);

-- ---------- grants to the RLS-enforced app role ----------
GRANT SELECT, INSERT, UPDATE, DELETE ON graph_nodes TO gmrag_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON graph_edges TO gmrag_app;
