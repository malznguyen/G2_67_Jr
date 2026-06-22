//! Tenant LLM / BYOK settings routes (T68).
//!
//! Owner-only GET/PUT for per-tenant LLM configuration. API keys are encrypted
//! with AES-256-GCM on write; GET returns a masked key never the raw secret.

use axum::extract::{Extension, Path};
use axum::response::IntoResponse;
use axum::Json;
use gmrag_core::crypto::{decrypt_with_aad, encrypt_with_aad};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::extractor::AuthUser;
use crate::auth::tenant::TenantContext;
use crate::error::ApiError;
use crate::middleware::rls::SharedConnection;
use crate::routes::chat::LlmRuntime;
use crate::routes::tenants::{ensure_path_matches_context, require_owner};

const PROVIDER_OLLAMA: &str = "ollama";
const PROVIDER_OPENAI: &str = "openai";
const DEFAULT_DIMENSIONS: i32 = 768;

#[derive(Deserialize)]
pub struct PutLlmSettingsBody {
    pub provider: String,
    pub model: String,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub dimensions: Option<i32>,
    pub enabled: Option<bool>,
    pub llm_model: Option<String>,
    pub llm_base_url: Option<String>,
}

#[derive(Serialize)]
struct LlmSettingsResponse {
    configured: bool,
    provider: Option<String>,
    model: Option<String>,
    base_url: Option<String>,
    dimensions: Option<i32>,
    enabled: Option<bool>,
    llm_model: Option<String>,
    llm_base_url: Option<String>,
    has_api_key: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    api_key_masked: Option<String>,
}

#[derive(sqlx::FromRow)]
struct TenantLlmConfigRow {
    provider: String,
    model: String,
    base_url: Option<String>,
    dimensions: i32,
    enabled: bool,
    llm_model: Option<String>,
    llm_base_url: Option<String>,
    api_key: Option<String>,
    api_key_ciphertext: Option<Vec<u8>>,
    api_key_nonce: Option<Vec<u8>>,
}

/// `GET /tenants/{tid}/settings/llm` — read BYOK config with masked API key.
pub async fn get_llm_settings(
    Path(tid): Path<Uuid>,
    Extension(ctx): Extension<TenantContext>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
    Extension(llm_runtime): Extension<LlmRuntime>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;
    require_owner(&conn, auth_user.user_id).await?;

    let mut guard = conn.lock().await;
    let row = sqlx::query_as::<_, TenantLlmConfigRow>(
        r#"
        SELECT provider, model, base_url, dimensions, enabled, llm_model, llm_base_url,
               api_key, api_key_ciphertext, api_key_nonce
        FROM tenant_llm_config
        WHERE tenant_id = $1
        "#,
    )
    .bind(tid)
    .fetch_optional(&mut *guard)
    .await
    .map_err(|e| ApiError::Internal(format!("db error: {e}")))?;
    drop(guard);

    let Some(row) = row else {
        return Ok(Json(LlmSettingsResponse {
            configured: false,
            provider: None,
            model: None,
            base_url: None,
            dimensions: None,
            enabled: None,
            llm_model: None,
            llm_base_url: None,
            has_api_key: false,
            api_key_masked: None,
        }));
    };

    let (has_api_key, api_key_masked) =
        resolve_masked_key(tid, &row, llm_runtime.tenant_key_encryption_key)?;

    Ok(Json(LlmSettingsResponse {
        configured: true,
        provider: Some(row.provider),
        model: Some(row.model),
        base_url: row.base_url,
        dimensions: Some(row.dimensions),
        enabled: Some(row.enabled),
        llm_model: row.llm_model,
        llm_base_url: row.llm_base_url,
        has_api_key,
        api_key_masked,
    }))
}

/// `PUT /tenants/{tid}/settings/llm` — upsert BYOK config with encrypted API key.
pub async fn put_llm_settings(
    Path(tid): Path<Uuid>,
    Extension(ctx): Extension<TenantContext>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(conn): Extension<SharedConnection>,
    Extension(llm_runtime): Extension<LlmRuntime>,
    Json(body): Json<PutLlmSettingsBody>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_path_matches_context(tid, &ctx)?;
    require_owner(&conn, auth_user.user_id).await?;

    let provider = body.provider.trim();
    if provider != PROVIDER_OLLAMA && provider != PROVIDER_OPENAI {
        return Err(ApiError::BadRequest(
            "provider must be 'ollama' or 'openai'".into(),
        ));
    }

    let model = body.model.trim();
    if model.is_empty() {
        return Err(ApiError::BadRequest("model must not be empty".into()));
    }

    let base_url = body
        .base_url
        .as_ref()
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .map(str::to_string);

    let api_key = body
        .api_key
        .as_ref()
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .map(str::to_string);

    if provider == PROVIDER_OPENAI && api_key.is_none() {
        return Err(ApiError::BadRequest(
            "api_key is required when provider is openai".into(),
        ));
    }

    let dimensions = body.dimensions.unwrap_or(DEFAULT_DIMENSIONS);
    let enabled = body.enabled.unwrap_or(true);

    let llm_model = body
        .llm_model
        .as_ref()
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .map(str::to_string);

    let llm_base_url = body
        .llm_base_url
        .as_ref()
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .map(str::to_string);

    let (ciphertext, nonce): (Option<Vec<u8>>, Option<Vec<u8>>) = match &api_key {
        Some(key) => {
            let enc_key = llm_runtime
                .tenant_key_encryption_key
                .as_ref()
                .ok_or_else(|| {
                    ApiError::BadRequest(
                        "GMRAG_TENANT_KEY_ENCRYPTION_KEY is not configured; cannot store API key"
                            .into(),
                    )
                })?;
            let (ct, n) = encrypt_with_aad(key, enc_key, tid.as_bytes()).map_err(|_| {
                ApiError::Internal("failed to encrypt tenant API key".into())
            })?;
            (Some(ct), Some(n))
        }
        None => (None, None),
    };

    let mut guard = conn.lock().await;
    sqlx::query(
        r#"
        INSERT INTO tenant_llm_config
            (tenant_id, provider, api_key, api_key_ciphertext, api_key_nonce,
             model, base_url, dimensions, enabled, llm_model, llm_base_url, updated_at)
        VALUES ($1, $2, NULL, $3, $4, $5, $6, $7, $8, $9, $10, now())
        ON CONFLICT (tenant_id) DO UPDATE SET
            provider = EXCLUDED.provider,
            api_key = NULL,
            api_key_ciphertext = EXCLUDED.api_key_ciphertext,
            api_key_nonce = EXCLUDED.api_key_nonce,
            model = EXCLUDED.model,
            base_url = EXCLUDED.base_url,
            dimensions = EXCLUDED.dimensions,
            enabled = EXCLUDED.enabled,
            llm_model = EXCLUDED.llm_model,
            llm_base_url = EXCLUDED.llm_base_url,
            updated_at = now()
        "#,
    )
    .bind(tid)
    .bind(provider)
    .bind(ciphertext)
    .bind(nonce)
    .bind(model)
    .bind(base_url.as_deref())
    .bind(dimensions)
    .bind(enabled)
    .bind(llm_model.as_deref())
    .bind(llm_base_url.as_deref())
    .execute(&mut *guard)
    .await
    .map_err(|e| ApiError::Internal(format!("db error: {e}")))?;
    drop(guard);

    get_llm_settings(
        Path(tid),
        Extension(ctx),
        Extension(auth_user),
        Extension(conn),
        Extension(llm_runtime),
    )
    .await
}

fn resolve_masked_key(
    tenant_id: Uuid,
    row: &TenantLlmConfigRow,
    encryption_key: Option<[u8; 32]>,
) -> Result<(bool, Option<String>), ApiError> {
    let plaintext = match (
        row.api_key_ciphertext.as_deref(),
        row.api_key_nonce.as_deref(),
    ) {
        (Some(ciphertext), Some(nonce)) => {
            let key = encryption_key.ok_or_else(|| {
                ApiError::Internal(
                    "encrypted API key present but GMRAG_TENANT_KEY_ENCRYPTION_KEY is not configured"
                        .into(),
                )
            })?;
            decrypt_with_aad(ciphertext, nonce, &key, tenant_id.as_bytes())
                .map_err(|_| ApiError::Internal("failed to decrypt tenant API key".into()))?
        }
        (None, None) => match row.api_key.as_deref().filter(|v| !v.trim().is_empty()) {
            Some(key) => key.to_string(),
            None => return Ok((false, None)),
        },
        _ => {
            return Err(ApiError::Internal(
                "tenant_llm_config has invalid encrypted key pair".into(),
            ))
        }
    };

    Ok((true, Some(mask_api_key(&plaintext))))
}

fn mask_api_key(key: &str) -> String {
    let trimmed = key.trim();
    if trimmed.len() <= 4 {
        return "***".to_string();
    }
    let suffix: String = trimmed.chars().rev().take(4).collect::<String>().chars().rev().collect();
    if trimmed.starts_with("sk-") {
        format!("sk-***{suffix}")
    } else {
        format!("***{suffix}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_api_key_sk_prefix() {
        assert_eq!(mask_api_key("sk-proj-abc123xyz789"), "sk-***z789");
    }

    #[test]
    fn mask_api_key_generic() {
        assert_eq!(mask_api_key("mysecretkey1234"), "***1234");
    }

    #[test]
    fn mask_api_key_short() {
        assert_eq!(mask_api_key("ab"), "***");
    }
}
