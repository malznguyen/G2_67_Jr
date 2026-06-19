-- =========================================================
-- T45: encrypted BYOK keys for tenant_llm_config.
--
-- Existing `api_key` remains as a legacy plaintext fallback so current
-- worker selectors continue to work until they are migrated. New API-side
-- resolution prefers encrypted fields when present.
-- =========================================================

ALTER TABLE tenant_llm_config
    ADD COLUMN api_key_ciphertext BYTEA NULL,
    ADD COLUMN api_key_nonce BYTEA NULL;

ALTER TABLE tenant_llm_config
    ADD CONSTRAINT tenant_llm_config_encrypted_key_pair
    CHECK (
        (api_key_ciphertext IS NULL AND api_key_nonce IS NULL)
        OR
        (api_key_ciphertext IS NOT NULL AND api_key_nonce IS NOT NULL)
    );
