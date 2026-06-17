-- =========================================================
-- T20: documents + document_chunks.
-- Documents are tenant-scoped file metadata; vector content lives in
-- Qdrant. document_chunks holds only the qdrant_point_id reference.
-- RLS policies are applied in T25 (rls_apply_all), NOT here.
-- =========================================================

-- ---------- documents ----------
CREATE TABLE documents (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    workspace_id UUID             NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    owner_id     UUID        NOT NULL REFERENCES users(id),
    title        TEXT        NOT NULL,
    status       TEXT        NOT NULL DEFAULT 'uploaded',
    visibility   TEXT        NOT NULL DEFAULT 'private',
    share_token  UUID            NULL,
    mime_type    TEXT            NULL,
    byte_size    BIGINT      NOT NULL DEFAULT 0,
    s3_key       TEXT            NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_documents_tenant    ON documents (tenant_id);
CREATE INDEX idx_documents_workspace ON documents (workspace_id);

-- ---------- document_chunks ----------
-- qdrant_point_id references a Qdrant vector point; the actual embedding
-- vector is stored in Qdrant, NOT in PostgreSQL.
CREATE TABLE document_chunks (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    document_id     UUID        NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    chunk_index     INT         NOT NULL,
    content         TEXT        NOT NULL,
    qdrant_point_id UUID        NOT NULL,
    token_count     INT            NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (document_id, chunk_index)
);

CREATE INDEX idx_document_chunks_tenant ON document_chunks (tenant_id);
CREATE INDEX idx_document_chunks_doc    ON document_chunks (document_id);

-- ---------- grants to the RLS-enforced app role ----------
GRANT SELECT, INSERT, UPDATE, DELETE ON documents        TO gmrag_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON document_chunks  TO gmrag_app;
