//! Usage metering: append-only `usage_events` rows (T51).

use sqlx::PgConnection;
use thiserror::Error;
use tiktoken_rs::cl100k_base;
use uuid::Uuid;

pub const METRIC_LLM_TOKENS: &str = "llm_tokens";
pub const METRIC_EMBEDDING_TOKENS: &str = "embedding_tokens";

#[derive(Debug, Error)]
pub enum MeteringError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("tokenizer error: {0}")]
    Tokenizer(String),
}

/// Count tokens with cl100k (same family as worker chunking).
pub fn count_tokens(text: &str) -> Result<u32, MeteringError> {
    let bpe = cl100k_base().map_err(|e| MeteringError::Tokenizer(e.to_string()))?;
    Ok(bpe.encode_with_special_tokens(text).len() as u32)
}

/// Record one append-only usage event under the current RLS tenant context.
pub async fn record_usage_event(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    metric: &str,
    delta: i64,
    metadata: Option<serde_json::Value>,
) -> Result<(), MeteringError> {
    sqlx::query(
        r#"
        INSERT INTO usage_events (tenant_id, metric, delta, metadata)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(tenant_id)
    .bind(metric)
    .bind(delta)
    .bind(metadata)
    .execute(conn)
    .await?;
    Ok(())
}

/// Record embedding token usage after a query embed call.
pub async fn record_embedding_usage(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    query: &str,
    model: &str,
) -> Result<u32, MeteringError> {
    let tokens = count_tokens(query)? as i64;
    record_usage_event(
        conn,
        tenant_id,
        METRIC_EMBEDDING_TOKENS,
        tokens,
        Some(serde_json::json!({
            "operation": "embed",
            "model": model,
        })),
    )
    .await?;
    Ok(tokens as u32)
}

/// Record LLM token usage after a chat completion (input + output).
pub async fn record_llm_usage(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    input_text: &str,
    output_text: &str,
    model: &str,
) -> Result<u32, MeteringError> {
    let input = count_tokens(input_text)? as i64;
    let output = count_tokens(output_text)? as i64;
    let total = input + output;
    record_usage_event(
        conn,
        tenant_id,
        METRIC_LLM_TOKENS,
        total,
        Some(serde_json::json!({
            "operation": "chat",
            "model": model,
            "input_tokens": input,
            "output_tokens": output,
        })),
    )
    .await?;
    Ok(total as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_tokens_non_empty() {
        let n = count_tokens("hello world").expect("count");
        assert!(n > 0);
    }

    #[test]
    fn count_tokens_empty_is_zero() {
        assert_eq!(count_tokens("").expect("count"), 0);
    }
}
