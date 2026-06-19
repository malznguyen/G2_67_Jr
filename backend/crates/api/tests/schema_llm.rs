//! Schema verification tests for Sprint 6A LLM/BYOK additions.

use sqlx::PgPool;

#[sqlx::test(migrations = "../../migrations")]
async fn tenant_llm_config_has_encrypted_key_columns(pool: PgPool) {
    let cols: Vec<(String, String)> = sqlx::query_as(
        "SELECT column_name, data_type
         FROM information_schema.columns
         WHERE table_name = 'tenant_llm_config'
           AND column_name IN ('api_key_ciphertext', 'api_key_nonce')
         ORDER BY column_name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    assert_eq!(cols.len(), 2, "missing encrypted BYOK columns: {cols:?}");
    for (name, data_type) in cols {
        assert_eq!(data_type, "bytea", "{name} must be BYTEA");
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn tenant_llm_config_encrypted_key_pair_constraint_exists(pool: PgPool) {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)
         FROM pg_constraint
         WHERE conrelid = 'tenant_llm_config'::regclass
           AND conname = 'tenant_llm_config_encrypted_key_pair'
           AND contype = 'c'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(count, 1, "encrypted key pair CHECK constraint must exist");
}
