//! T41 — integration tests for `select_graph_extractor` (BYOK factory).
//!
//! Mirrors `tests/select_embedder.rs`: seeds `tenant_llm_config` rows and
//! verifies the 2-layer fallback (tenant BYOK → global DeepSeek).

use gmrag_core::config::DeepSeekConfig;
use gmrag_core::crypto::encrypt_with_aad;
use gmrag_worker::{select_graph_extractor, DeepSeekGraphExtractor};
use sqlx::PgPool;
use uuid::Uuid;

fn global_deepseek_cfg() -> DeepSeekConfig {
    DeepSeekConfig {
        api_key: Some("sk-global".into()),
        base_url: "https://api.deepseek.com/v1".into(),
        model: "deepseek-v4-flash".into(),
        timeout_s: 60,
    }
}

async fn create_tenant(pool: &PgPool, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, $2)")
        .bind(id)
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    id
}

#[allow(clippy::too_many_arguments)]
async fn set_llm_config_full(
    pool: &PgPool,
    tenant_id: Uuid,
    provider: &str,
    api_key: Option<&str>,
    model: &str,
    llm_model: Option<&str>,
    llm_base_url: Option<&str>,
    enabled: bool,
) {
    sqlx::query(
        r#"
        INSERT INTO tenant_llm_config
            (tenant_id, provider, api_key, model, base_url, enabled, llm_model, llm_base_url)
        VALUES ($1, $2, $3, $4, NULL, $5, $6, $7)
        "#,
    )
    .bind(tenant_id)
    .bind(provider)
    .bind(api_key)
    .bind(model)
    .bind(enabled)
    .bind(llm_model)
    .bind(llm_base_url)
    .execute(pool)
    .await
    .unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn select_graph_extractor_uses_byok_openai_when_configured(pool: PgPool) {
    let tenant = create_tenant(&pool, "BYOK Graph Tenant").await;
    set_llm_config_full(
        &pool,
        tenant,
        "openai",
        Some("sk-byok-chat"),
        "text-embedding-3-small",
        Some("gpt-4o-mini"),
        Some("https://api.openai.com/v1"),
        true,
    )
    .await;

    let ext = select_graph_extractor(&pool, tenant, &global_deepseek_cfg(), None)
        .await
        .expect("factory must succeed");

    // BYOK OpenAI extractor must point at the OpenAI chat endpoint.
    assert!(ext.url().contains("api.openai.com"));
    assert!(ext.url().ends_with("/chat/completions"));
    assert_eq!(ext.model(), "gpt-4o-mini");
}

#[sqlx::test(migrations = "../../migrations")]
async fn select_graph_extractor_falls_back_to_global_when_no_row(pool: PgPool) {
    let tenant = create_tenant(&pool, "No-Config Graph Tenant").await;

    let ext = select_graph_extractor(&pool, tenant, &global_deepseek_cfg(), None)
        .await
        .expect("factory must succeed");

    assert!(ext.url().contains("api.deepseek.com"));
    assert_eq!(ext.model(), "deepseek-v4-flash");
}

#[sqlx::test(migrations = "../../migrations")]
async fn select_graph_extractor_falls_back_when_disabled(pool: PgPool) {
    let tenant = create_tenant(&pool, "Disabled Graph Tenant").await;
    set_llm_config_full(
        &pool,
        tenant,
        "openai",
        Some("sk-disabled"),
        "text-embedding-3-small",
        Some("gpt-4o-mini"),
        None,
        false,
    )
    .await;

    let ext = select_graph_extractor(&pool, tenant, &global_deepseek_cfg(), None)
        .await
        .expect("factory must succeed");
    assert!(
        ext.url().contains("api.deepseek.com"),
        "disabled BYOK must fall back"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn select_graph_extractor_respects_rls_isolation(pool: PgPool) {
    let tenant_a = create_tenant(&pool, "Tenant A (BYOK chat)").await;
    let tenant_b = create_tenant(&pool, "Tenant B (no config)").await;
    set_llm_config_full(
        &pool,
        tenant_a,
        "openai",
        Some("sk-tenant-a"),
        "text-embedding-3-small",
        Some("gpt-4o-mini"),
        Some("https://api.openai.com/v1"),
        true,
    )
    .await;

    let ext_b = select_graph_extractor(&pool, tenant_b, &global_deepseek_cfg(), None)
        .await
        .expect("factory must succeed");
    assert!(
        ext_b.url().contains("api.deepseek.com"),
        "tenant_b must fall back — RLS hides tenant_a config"
    );

    let ext_a = select_graph_extractor(&pool, tenant_a, &global_deepseek_cfg(), None)
        .await
        .expect("factory must succeed");
    assert!(ext_a.url().contains("api.openai.com"));
}

// Compile-time check: the global fallback returns a DeepSeekGraphExtractor.
#[test]
fn global_extractor_is_deepseek() {
    let cfg = global_deepseek_cfg();
    let _ = DeepSeekGraphExtractor::new(&cfg);
}

#[sqlx::test(migrations = "../../migrations")]
async fn select_graph_extractor_decrypts_encrypted_byok_key(pool: PgPool) {
    let tenant = create_tenant(&pool, "Encrypted Graph BYOK Tenant").await;
    let enc_key: [u8; 32] = [7_u8; 32];
    let (ciphertext, nonce) =
        encrypt_with_aad("sk-encrypted-chat", &enc_key, tenant.as_bytes()).unwrap();

    sqlx::query(
        r#"
        INSERT INTO tenant_llm_config
            (tenant_id, provider, api_key_ciphertext, api_key_nonce,
             model, enabled, llm_model, llm_base_url)
        VALUES ($1, 'openai', $2, $3,
                'text-embedding-3-small', true,
                'gpt-4o-mini', 'https://api.openai.com/v1')
        "#,
    )
    .bind(tenant)
    .bind(ciphertext)
    .bind(nonce)
    .execute(&pool)
    .await
    .unwrap();

    let ext = select_graph_extractor(&pool, tenant, &global_deepseek_cfg(), Some(&enc_key))
        .await
        .expect("factory must decrypt and succeed");

    assert!(ext.url().contains("api.openai.com"));
    assert!(ext.url().ends_with("/chat/completions"));
    assert_eq!(ext.model(), "gpt-4o-mini");
}

#[sqlx::test(migrations = "../../migrations")]
async fn select_graph_extractor_encrypted_without_enc_key_returns_error(pool: PgPool) {
    let tenant = create_tenant(&pool, "Encrypted Graph No Key Tenant").await;
    let enc_key: [u8; 32] = [7_u8; 32];
    let (ciphertext, nonce) =
        encrypt_with_aad("sk-encrypted", &enc_key, tenant.as_bytes()).unwrap();

    sqlx::query(
        r#"
        INSERT INTO tenant_llm_config
            (tenant_id, provider, api_key_ciphertext, api_key_nonce,
             model, enabled, llm_model)
        VALUES ($1, 'openai', $2, $3,
                'text-embedding-3-small', true, 'gpt-4o-mini')
        "#,
    )
    .bind(tenant)
    .bind(ciphertext)
    .bind(nonce)
    .execute(&pool)
    .await
    .unwrap();

    let result = select_graph_extractor(&pool, tenant, &global_deepseek_cfg(), None).await;
    match result {
        Err(e) => assert!(
            e.to_string().contains("decrypt failed"),
            "must fail with decrypt error, got: {e}"
        ),
        Ok(_) => panic!("expected decrypt error, got success"),
    }
}
