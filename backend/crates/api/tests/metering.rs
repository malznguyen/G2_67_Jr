//! Integration tests for usage_events metering (T51).

use gmrag_api::metering::{
    record_embedding_usage, record_llm_usage, METRIC_EMBEDDING_TOKENS, METRIC_LLM_TOKENS,
};
use sqlx::PgPool;
use uuid::Uuid;

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

async fn begin_rls_tx(pool: &PgPool) -> sqlx::Transaction<'static, sqlx::Postgres> {
    let mut tx = pool.begin().await.unwrap();
    sqlx::Executor::execute(&mut *tx, "SET LOCAL ROLE gmrag_app")
        .await
        .unwrap();
    tx
}

async fn set_tenant(tx: &mut sqlx::Transaction<'static, sqlx::Postgres>, tenant_id: Uuid) {
    sqlx::query(&format!("SET LOCAL app.tenant_id = '{tenant_id}'"))
        .execute(&mut **tx)
        .await
        .unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn record_embedding_usage_inserts_row(pool: PgPool) {
    let tenant = create_tenant(&pool, "Meter Tenant").await;
    let mut tx = begin_rls_tx(&pool).await;
    set_tenant(&mut tx, tenant).await;

    let tokens = record_embedding_usage(&mut tx, tenant, "hello retrieval query", "ollama")
        .await
        .expect("record embed");

    assert!(tokens > 0);

    let row: (String, i64) = sqlx::query_as(
        "SELECT metric, delta FROM usage_events WHERE tenant_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(tenant)
    .fetch_one(&mut *tx)
    .await
    .expect("row");

    assert_eq!(row.0, METRIC_EMBEDDING_TOKENS);
    assert_eq!(row.1, tokens as i64);

    tx.rollback().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn record_llm_usage_inserts_row(pool: PgPool) {
    let tenant = create_tenant(&pool, "Chat Tenant").await;
    let mut tx = begin_rls_tx(&pool).await;
    set_tenant(&mut tx, tenant).await;

    let tokens = record_llm_usage(
        &mut tx,
        tenant,
        "system prompt and user question",
        "assistant answer text",
        "deepseek-v4-flash",
    )
    .await
    .expect("record llm");

    assert!(tokens > 0);

    let row: (String, i64) = sqlx::query_as(
        "SELECT metric, delta FROM usage_events WHERE tenant_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(tenant)
    .fetch_one(&mut *tx)
    .await
    .expect("row");

    assert_eq!(row.0, METRIC_LLM_TOKENS);
    assert_eq!(row.1, tokens as i64);

    tx.rollback().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn usage_events_rls_isolates_tenants(pool: PgPool) {
    let tenant_a = create_tenant(&pool, "Tenant A").await;
    let tenant_b = create_tenant(&pool, "Tenant B").await;

    {
        let mut tx = begin_rls_tx(&pool).await;
        set_tenant(&mut tx, tenant_a).await;
        record_embedding_usage(&mut tx, tenant_a, "tenant a query", "ollama")
            .await
            .expect("record a");
        tx.commit().await.unwrap();
    }

    let mut tx = begin_rls_tx(&pool).await;
    set_tenant(&mut tx, tenant_b).await;

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM usage_events")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(count, 0, "tenant_b must not see tenant_a usage events");

    tx.rollback().await.unwrap();
}
