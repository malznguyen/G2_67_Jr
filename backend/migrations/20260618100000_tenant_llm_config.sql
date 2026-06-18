-- =========================================================
-- T40: tenant_llm_config — per-tenant BYOK embedding provider config.
--
-- One row per tenant (PK = tenant_id). Stores the embedding provider
-- ('ollama' | 'openai'), API key, model name, base URL, and output
-- dimensions. The worker's `select_embedder` reads this table (with
-- `SET LOCAL app.tenant_id`) to pick the right embedder per job.
--
-- RLS: tenant-scoped via `tenant_id = gmrag_current_tenant()` — same
-- uniform policy as every other tenant-scoped table (T25 pattern).
-- FORCE RLS so even the table owner is subject to the policy.
--
-- MVP: api_key is stored as plaintext. Follow-up: encrypt at rest with
-- pgcrypto `pgp_sym_encrypt` + env `GMRAG_TENANT_KEY_ENCRYPTION_KEY`.
-- =========================================================

CREATE TABLE tenant_llm_config (
    tenant_id   UUID        PRIMARY KEY REFERENCES tenants(id) ON DELETE CASCADE,
    provider    TEXT        NOT NULL,          -- 'ollama' | 'openai'
    api_key     TEXT            NULL,          -- plaintext MVP; NULL when provider='ollama'
    model       TEXT        NOT NULL,          -- 'text-embedding-3-small' / 'nomic-embed-text'
    base_url    TEXT            NULL,          -- OpenAI base URL; NULL = default
    dimensions  INT         NOT NULL DEFAULT 768,
    enabled     BOOLEAN     NOT NULL DEFAULT true,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ---------- RLS ----------
ALTER TABLE tenant_llm_config ENABLE ROW LEVEL SECURITY;
ALTER TABLE tenant_llm_config FORCE ROW LEVEL SECURITY;

CREATE POLICY tenant_llm_config_isolation ON tenant_llm_config
    USING (tenant_id = gmrag_current_tenant());

-- ---------- grants to the RLS-enforced app role ----------
GRANT SELECT, INSERT, UPDATE, DELETE ON tenant_llm_config TO gmrag_app;
