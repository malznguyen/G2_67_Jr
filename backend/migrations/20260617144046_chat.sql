-- =========================================================
-- T22: chat_sessions + chat_messages.
-- Conversation history for RAG chat. Sessions are tenant-scoped and
-- optionally tied to a workspace. Messages carry role + content + token
-- accounting. RLS policies are applied in T25 (rls_apply_all), NOT here.
-- =========================================================

-- ---------- chat_sessions ----------
CREATE TABLE chat_sessions (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    workspace_id UUID            NULL REFERENCES workspaces(id) ON DELETE SET NULL,
    user_id      UUID        NOT NULL REFERENCES users(id),
    title        TEXT        NOT NULL DEFAULT '',
    model        TEXT            NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_chat_sessions_tenant    ON chat_sessions (tenant_id);
CREATE INDEX idx_chat_sessions_workspace ON chat_sessions (workspace_id);

-- ---------- chat_messages ----------
CREATE TABLE chat_messages (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    session_id  UUID        NOT NULL REFERENCES chat_sessions(id) ON DELETE CASCADE,
    role        TEXT        NOT NULL,
    content     TEXT        NOT NULL,
    token_count INT            NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_chat_messages_tenant  ON chat_messages (tenant_id);
CREATE INDEX idx_chat_messages_session ON chat_messages (session_id);

-- ---------- grants to the RLS-enforced app role ----------
GRANT SELECT, INSERT, UPDATE, DELETE ON chat_sessions  TO gmrag_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON chat_messages  TO gmrag_app;
