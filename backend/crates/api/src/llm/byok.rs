//! Tenant BYOK resolution for API-side LLM providers.
//!
//! Callers pass the request-scoped RLS connection from `SharedConnection`.
//! This function does not create a new transaction; it relies on the caller's
//! existing `SET LOCAL app.tenant_id` context.

use gmrag_core::config::{DeepSeekConfig, OllamaConfig};
use gmrag_core::crypto::{decrypt_with_aad, CryptoError};
use sqlx::PgConnection;
use thiserror::Error;
use uuid::Uuid;

use super::provider::{
    EmbeddingProviderConfig, ProviderConfig, DEFAULT_OPENAI_BASE_URL, DEFAULT_OPENAI_EMBED_MODEL,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmConfigSource {
    Global,
    TenantEncrypted,
    TenantPlaintext,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLlmConfig {
    pub source: LlmConfigSource,
    pub provider: ProviderConfig,
}

#[derive(Debug, Error)]
pub enum ByokError {
    #[error("byok database error: {0}")]
    Db(String),
    #[error("tenant BYOK key is encrypted but GMRAG_TENANT_KEY_ENCRYPTION_KEY is not configured")]
    MissingEncryptionKey,
    #[error("tenant BYOK encrypted nonce must be 12 bytes, got {0}")]
    InvalidNonce(usize),
    #[error("tenant BYOK decrypt failed")]
    Decrypt,
    #[error("tenant BYOK plaintext is not valid UTF-8")]
    Utf8,
}

#[derive(Debug, sqlx::FromRow)]
struct TenantLlmRow {
    provider: String,
    api_key: Option<String>,
    api_key_ciphertext: Option<Vec<u8>>,
    api_key_nonce: Option<Vec<u8>>,
    model: String,
    base_url: Option<String>,
    enabled: bool,
    llm_model: Option<String>,
    llm_base_url: Option<String>,
}

/// Resolve API-side LLM configuration for a tenant.
///
/// Fallback is deliberately narrow:
/// - no row / disabled row / no key fields: global DeepSeek + global Ollama;
/// - encrypted fields present: decrypt or fail clearly;
/// - plaintext `api_key`: legacy fallback only when encrypted fields are null.
pub async fn resolve_llm_config(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    deepseek: &DeepSeekConfig,
    ollama: &OllamaConfig,
    tenant_key_encryption_key: Option<&[u8; 32]>,
) -> Result<ResolvedLlmConfig, ByokError> {
    let global = || ResolvedLlmConfig {
        source: LlmConfigSource::Global,
        provider: ProviderConfig::from_global(deepseek, ollama),
    };

    let row = sqlx::query_as::<_, TenantLlmRow>(
        r#"
        SELECT provider, api_key, api_key_ciphertext, api_key_nonce,
               model, base_url, enabled, llm_model, llm_base_url
        FROM tenant_llm_config
        WHERE tenant_id = $1 AND enabled = true
        "#,
    )
    .bind(tenant_id)
    .fetch_optional(conn)
    .await
    .map_err(|e| ByokError::Db(e.to_string()))?;

    let Some(row) = row else {
        return Ok(global());
    };
    if !row.enabled {
        return Ok(global());
    }

    let (api_key, source) = match (
        row.api_key_ciphertext.as_deref(),
        row.api_key_nonce.as_deref(),
    ) {
        (Some(ciphertext), Some(nonce)) => {
            let key = tenant_key_encryption_key.ok_or(ByokError::MissingEncryptionKey)?;
            let decrypted = decrypt_with_aad(ciphertext, nonce, key, tenant_id.as_bytes())
                .map_err(map_crypto_error)?;
            (decrypted, LlmConfigSource::TenantEncrypted)
        }
        (None, None) => match row.api_key.filter(|v| !v.trim().is_empty()) {
            Some(key) => (key, LlmConfigSource::TenantPlaintext),
            None => return Ok(global()),
        },
        _ => return Err(ByokError::Decrypt),
    };

    let embedding = if row.provider == "openai" {
        EmbeddingProviderConfig::OpenAi {
            api_key: api_key.clone(),
            base_url: row
                .base_url
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_OPENAI_BASE_URL.to_string()),
            model: if row.model.trim().is_empty() {
                DEFAULT_OPENAI_EMBED_MODEL.to_string()
            } else {
                row.model
            },
        }
    } else {
        EmbeddingProviderConfig::Ollama {
            host: ollama.host.clone(),
            model: ollama.embed_model.clone(),
        }
    };

    let mut provider = ProviderConfig::from_global(deepseek, ollama);
    provider.embedding = embedding;

    if let Some(model) = row.llm_model.filter(|v| !v.trim().is_empty()) {
        provider.chat_api_key = Some(api_key);
        provider.chat_model = model;
        provider.chat_base_url = row
            .llm_base_url
            .filter(|v| !v.trim().is_empty())
            .or_else(|| {
                if row.provider == "openai" {
                    Some(DEFAULT_OPENAI_BASE_URL.to_string())
                } else {
                    Some(deepseek.base_url.clone())
                }
            })
            .unwrap_or_else(|| deepseek.base_url.clone());
    }

    Ok(ResolvedLlmConfig { source, provider })
}

fn map_crypto_error(e: CryptoError) -> ByokError {
    match e {
        CryptoError::InvalidNonceLen(n) => ByokError::InvalidNonce(n),
        CryptoError::Decrypt => ByokError::Decrypt,
        CryptoError::Utf8 => ByokError::Utf8,
        CryptoError::Encrypt => ByokError::Decrypt,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::pool::PoolConnection;
    use sqlx::{Executor as _, PgPool, Postgres};

    fn deepseek_cfg() -> DeepSeekConfig {
        DeepSeekConfig {
            api_key: Some("sk-global".into()),
            base_url: "https://api.deepseek.com/v1".into(),
            model: "deepseek-v4-flash".into(),
            timeout_s: 60,
        }
    }

    fn ollama_cfg() -> OllamaConfig {
        OllamaConfig {
            host: "http://ollama:11434".into(),
            embed_model: "nomic-embed-text".into(),
            llm_model: "llama3.1:8b".into(),
            keep_alive: "30m".into(),
        }
    }

    async fn tenant(pool: &PgPool, name: &str) -> Uuid {
        sqlx::query_scalar("INSERT INTO tenants (name) VALUES ($1) RETURNING id")
            .bind(name)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn rls_conn(pool: &PgPool, tenant_id: Uuid) -> PoolConnection<Postgres> {
        let mut conn = pool.acquire().await.unwrap();
        conn.execute("BEGIN").await.unwrap();
        conn.execute("SET LOCAL ROLE gmrag_app").await.unwrap();
        sqlx::query(&format!("SET LOCAL app.tenant_id = '{tenant_id}'"))
            .execute(&mut *conn)
            .await
            .unwrap();
        conn
    }

    fn encrypt(tenant_id: Uuid, key: &[u8; 32], plaintext: &str) -> (Vec<u8>, Vec<u8>) {
        gmrag_core::crypto::encrypt_with_aad(plaintext, key, tenant_id.as_bytes()).unwrap()
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn no_row_falls_back_to_global(pool: PgPool) {
        let tenant_id = tenant(&pool, "fallback").await;
        let mut conn = rls_conn(&pool, tenant_id).await;

        let resolved =
            resolve_llm_config(&mut conn, tenant_id, &deepseek_cfg(), &ollama_cfg(), None)
                .await
                .unwrap();

        assert_eq!(resolved.source, LlmConfigSource::Global);
        assert_eq!(resolved.provider.chat_model, "deepseek-v4-flash");
        assert!(matches!(
            resolved.provider.embedding,
            EmbeddingProviderConfig::Ollama { .. }
        ));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn encrypted_key_decrypts_and_selects_tenant_provider(pool: PgPool) {
        let tenant_id = tenant(&pool, "encrypted").await;
        let key = [7_u8; 32];
        let (ciphertext, nonce) = encrypt(tenant_id, &key, "sk-tenant");
        sqlx::query(
            r#"
            INSERT INTO tenant_llm_config
                (tenant_id, provider, api_key_ciphertext, api_key_nonce,
                 model, base_url, enabled, llm_model, llm_base_url)
            VALUES ($1, 'openai', $2, $3,
                    'text-embedding-3-small', 'https://custom.openai/v1', true,
                    'gpt-4o-mini', 'https://chat.openai/v1')
            "#,
        )
        .bind(tenant_id)
        .bind(ciphertext)
        .bind(nonce)
        .execute(&pool)
        .await
        .unwrap();

        let mut conn = rls_conn(&pool, tenant_id).await;
        let resolved = resolve_llm_config(
            &mut conn,
            tenant_id,
            &deepseek_cfg(),
            &ollama_cfg(),
            Some(&key),
        )
        .await
        .unwrap();

        assert_eq!(resolved.source, LlmConfigSource::TenantEncrypted);
        assert_eq!(resolved.provider.chat_model, "gpt-4o-mini");
        assert_eq!(resolved.provider.chat_api_key.as_deref(), Some("sk-tenant"));
        assert_eq!(resolved.provider.chat_base_url, "https://chat.openai/v1");
        assert_eq!(
            resolved.provider.embedding,
            EmbeddingProviderConfig::OpenAi {
                api_key: "sk-tenant".into(),
                base_url: "https://custom.openai/v1".into(),
                model: "text-embedding-3-small".into(),
            }
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn legacy_plaintext_key_is_used_only_without_encrypted_fields(pool: PgPool) {
        let tenant_id = tenant(&pool, "legacy").await;
        sqlx::query(
            r#"
            INSERT INTO tenant_llm_config
                (tenant_id, provider, api_key, model, base_url, enabled, llm_model)
            VALUES ($1, 'openai', 'sk-legacy', 'text-embedding-3-small',
                    NULL, true, 'gpt-4o-mini')
            "#,
        )
        .bind(tenant_id)
        .execute(&pool)
        .await
        .unwrap();

        let mut conn = rls_conn(&pool, tenant_id).await;
        let resolved =
            resolve_llm_config(&mut conn, tenant_id, &deepseek_cfg(), &ollama_cfg(), None)
                .await
                .unwrap();

        assert_eq!(resolved.source, LlmConfigSource::TenantPlaintext);
        assert_eq!(resolved.provider.chat_api_key.as_deref(), Some("sk-legacy"));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn rls_hides_other_tenant_config(pool: PgPool) {
        let tenant_a = tenant(&pool, "tenant-a").await;
        let tenant_b = tenant(&pool, "tenant-b").await;
        sqlx::query(
            r#"
            INSERT INTO tenant_llm_config
                (tenant_id, provider, api_key, model, enabled, llm_model)
            VALUES ($1, 'openai', 'sk-a', 'text-embedding-3-small', true, 'gpt-4o-mini')
            "#,
        )
        .bind(tenant_a)
        .execute(&pool)
        .await
        .unwrap();

        let mut conn = rls_conn(&pool, tenant_b).await;
        let resolved =
            resolve_llm_config(&mut conn, tenant_a, &deepseek_cfg(), &ollama_cfg(), None)
                .await
                .unwrap();

        assert_eq!(resolved.source, LlmConfigSource::Global);
        assert_eq!(resolved.provider.chat_api_key.as_deref(), Some("sk-global"));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn encrypted_config_fails_without_encryption_key(pool: PgPool) {
        let tenant_id = tenant(&pool, "missing-key").await;
        let key = [7_u8; 32];
        let (ciphertext, nonce) = encrypt(tenant_id, &key, "sk-tenant");
        sqlx::query(
            r#"
            INSERT INTO tenant_llm_config
                (tenant_id, provider, api_key_ciphertext, api_key_nonce,
                 model, enabled, llm_model)
            VALUES ($1, 'openai', $2, $3, 'text-embedding-3-small', true, 'gpt-4o-mini')
            "#,
        )
        .bind(tenant_id)
        .bind(ciphertext)
        .bind(nonce)
        .execute(&pool)
        .await
        .unwrap();

        let mut conn = rls_conn(&pool, tenant_id).await;
        let err = resolve_llm_config(&mut conn, tenant_id, &deepseek_cfg(), &ollama_cfg(), None)
            .await
            .unwrap_err();

        assert!(matches!(err, ByokError::MissingEncryptionKey));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn corrupted_encrypted_config_fails_without_plaintext_fallback(pool: PgPool) {
        let tenant_id = tenant(&pool, "corrupt").await;
        sqlx::query(
            r#"
            INSERT INTO tenant_llm_config
                (tenant_id, provider, api_key, api_key_ciphertext, api_key_nonce,
                 model, enabled, llm_model)
            VALUES ($1, 'openai', 'sk-legacy', $2, $3,
                    'text-embedding-3-small', true, 'gpt-4o-mini')
            "#,
        )
        .bind(tenant_id)
        .bind(vec![1_u8, 2, 3])
        .bind(vec![9_u8; 12])
        .execute(&pool)
        .await
        .unwrap();

        let mut conn = rls_conn(&pool, tenant_id).await;
        let err = resolve_llm_config(
            &mut conn,
            tenant_id,
            &deepseek_cfg(),
            &ollama_cfg(),
            Some(&[7_u8; 32]),
        )
        .await
        .unwrap_err();

        assert!(matches!(err, ByokError::Decrypt));
    }
}
