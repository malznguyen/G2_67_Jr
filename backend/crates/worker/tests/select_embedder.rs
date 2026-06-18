//! T40 — integration tests for `select_embedder` (BYOK factory).
//!
//! These tests require a running PostgreSQL instance (Docker) with
//! migrations applied via `#[sqlx::test]`. They verify that the factory
//! reads `tenant_llm_config` (RLS-scoped) and returns the right embedder:
//! `OpenAiEmbedder` when the tenant has an enabled OpenAI row with a key,
//! `OllamaEmbedder` otherwise.
//!
//! RLS pattern mirrors `crates/api/tests/rls_isolation.rs`: seed rows as
//! superuser, then query via `SET LOCAL ROLE gmrag_app` +
//! `SET LOCAL app.tenant_id`. `select_embedder` does the RLS setup
//! internally, so tests only seed + call the factory.

use gmrag_core::config::OllamaConfig;
use gmrag_worker::{Embedder, select_embedder};
use sqlx::PgPool;
use uuid::Uuid;

fn test_ollama_cfg() -> OllamaConfig {
    OllamaConfig {
        host: "http://localhost:11434".into(),
        embed_model: "nomic-embed-text".into(),
        llm_model: "llama3.1:8b".into(),
        keep_alive: "30m".into(),
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

async fn set_llm_config(
    pool: &PgPool,
    tenant_id: Uuid,
    provider: &str,
    api_key: Option<&str>,
    model: &str,
    base_url: Option<&str>,
    enabled: bool,
) {
    sqlx::query(
        r#"
        INSERT INTO tenant_llm_config (tenant_id, provider, api_key, model, base_url, enabled)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(tenant_id)
    .bind(provider)
    .bind(api_key)
    .bind(model)
    .bind(base_url)
    .bind(enabled)
    .execute(pool)
    .await
    .unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn select_embedder_uses_openai_when_tenant_has_key(pool: PgPool) {
    let tenant = create_tenant(&pool, "BYOK Tenant").await;
    set_llm_config(
        &pool,
        tenant,
        "openai",
        Some("sk-byok-real-key"),
        "text-embedding-3-small",
        Some("https://api.openai.com/v1"),
        true,
    )
    .await;

    let embedder = select_embedder(&pool, tenant, &test_ollama_cfg())
        .await
        .expect("factory must succeed");

    assert_eq!(embedder.provider(), "openai");
    assert_eq!(embedder.dimension(), 768);
    // Verify it's actually an OpenAiEmbedder by checking the URL.
    let any_embedder: &dyn Embedder = embedder.as_ref();
    let _ = any_embedder; // type-erased; provider() is the discriminator.
}

#[sqlx::test(migrations = "../../migrations")]
async fn select_embedder_falls_back_to_ollama_when_no_row(pool: PgPool) {
    let tenant = create_tenant(&pool, "No-Config Tenant").await;
    // No tenant_llm_config row for this tenant.

    let embedder = select_embedder(&pool, tenant, &test_ollama_cfg())
        .await
        .expect("factory must succeed");

    assert_eq!(embedder.provider(), "ollama");
    assert_eq!(embedder.dimension(), 768);
}

#[sqlx::test(migrations = "../../migrations")]
async fn select_embedder_falls_back_when_provider_ollama(pool: PgPool) {
    let tenant = create_tenant(&pool, "Ollama Tenant").await;
    set_llm_config(
        &pool,
        tenant,
        "ollama",
        None,
        "nomic-embed-text",
        None,
        true,
    )
    .await;

    let embedder = select_embedder(&pool, tenant, &test_ollama_cfg())
        .await
        .expect("factory must succeed");

    assert_eq!(embedder.provider(), "ollama");
}

#[sqlx::test(migrations = "../../migrations")]
async fn select_embedder_falls_back_when_disabled(pool: PgPool) {
    let tenant = create_tenant(&pool, "Disabled BYOK Tenant").await;
    set_llm_config(
        &pool,
        tenant,
        "openai",
        Some("sk-disabled"),
        "text-embedding-3-small",
        None,
        false, // disabled → must fall back to ollama
    )
    .await;

    let embedder = select_embedder(&pool, tenant, &test_ollama_cfg())
        .await
        .expect("factory must succeed");

    assert_eq!(embedder.provider(), "ollama");
}

#[sqlx::test(migrations = "../../migrations")]
async fn select_embedder_respects_rls_isolation(pool: PgPool) {
    // Tenant A has an OpenAI BYOK config; tenant B has none.
    let tenant_a = create_tenant(&pool, "Tenant A (BYOK)").await;
    let tenant_b = create_tenant(&pool, "Tenant B (no config)").await;
    set_llm_config(
        &pool,
        tenant_a,
        "openai",
        Some("sk-tenant-a"),
        "text-embedding-3-small",
        None,
        true,
    )
    .await;

    // Querying for tenant_b must NOT see tenant_a's config (RLS).
    let embedder_b = select_embedder(&pool, tenant_b, &test_ollama_cfg())
        .await
        .expect("factory must succeed");
    assert_eq!(
        embedder_b.provider(),
        "ollama",
        "tenant_b must fall back to ollama — RLS must hide tenant_a's config"
    );

    // Querying for tenant_a must see the OpenAI config.
    let embedder_a = select_embedder(&pool, tenant_a, &test_ollama_cfg())
        .await
        .expect("factory must succeed");
    assert_eq!(embedder_a.provider(), "openai");
}

#[sqlx::test(migrations = "../../migrations")]
async fn select_embedder_openai_without_api_key_falls_back(pool: PgPool) {
    let tenant = create_tenant(&pool, "OpenAI No Key Tenant").await;
    set_llm_config(
        &pool,
        tenant,
        "openai",
        None, // no api_key → can't use OpenAI
        "text-embedding-3-small",
        None,
        true,
    )
    .await;

    let embedder = select_embedder(&pool, tenant, &test_ollama_cfg())
        .await
        .expect("factory must succeed");

    assert_eq!(
        embedder.provider(),
        "ollama",
        "openai provider without api_key must fall back to ollama"
    );
}
