//! Phase 0 (TASK-P0-04) — retention task integration tests.
//!
//! Verifies the bounded retention deletes:
//! - dispatched `ingest_outbox` rows older than the window are deleted;
//! - recent dispatched outbox rows are preserved;
//! - pending outbox rows are preserved regardless of age;
//! - old `usage_events` are deleted, recent ones preserved;
//! - old `audit_log` are deleted, recent ones preserved;
//! - the batch limit is respected (no unbounded DELETE).
//!
//! `#[sqlx::test]` cases need a running PostgreSQL instance; when it is
//! unavailable the binary is skipped by the sqlx harness (environmental
//! blocker, not a code failure).

use chrono::{Duration, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use gmrag_worker::retention::{
    delete_audit_older_than, delete_dispatched_outbox_older_than, delete_usage_older_than,
    run_retention_once,
};

async fn insert_tenant(pool: &PgPool, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, $2)")
        .bind(id)
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn insert_user(pool: &PgPool, email: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, email, name) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(email)
        .bind(email)
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn insert_document(pool: &PgPool, tenant: Uuid) -> Uuid {
    let owner = insert_user(pool, "o@ret.com").await;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO documents (id, tenant_id, owner_id, title, status)
         VALUES ($1, $2, $3, 'd', 'indexed')",
    )
    .bind(id)
    .bind(tenant)
    .bind(owner)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn insert_outbox(
    pool: &PgPool,
    tenant: Uuid,
    doc: Uuid,
    status: &str,
    dispatched_at: Option<chrono::DateTime<Utc>>,
) {
    sqlx::query(
        "INSERT INTO ingest_outbox (tenant_id, document_id, payload, status, dispatched_at)
         VALUES ($1, $2, '{}'::jsonb, $3, $4)",
    )
    .bind(tenant)
    .bind(doc)
    .bind(status)
    .bind(dispatched_at)
    .execute(pool)
    .await
    .unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn retention_deletes_old_dispatched_outbox_preserves_recent_and_pending(pool: PgPool) {
    let tenant = insert_tenant(&pool, "ret-out").await;
    let doc = insert_document(&pool, tenant).await;

    // Old dispatched (40 days ago) — should be deleted with a 30-day window.
    let old_dispatched = Utc::now() - Duration::days(40);
    insert_outbox(&pool, tenant, doc, "dispatched", Some(old_dispatched)).await;
    // Recent dispatched (1 day ago) — preserved.
    insert_outbox(
        &pool,
        tenant,
        doc,
        "dispatched",
        Some(Utc::now() - Duration::days(1)),
    )
    .await;
    // Old but PENDING — preserved regardless of age (dispatched_at NULL).
    insert_outbox(&pool, tenant, doc, "pending", Some(old_dispatched)).await;

    let deleted = delete_dispatched_outbox_older_than(&pool, 30, 1000)
        .await
        .unwrap();
    assert_eq!(deleted, 1, "only the old dispatched row is deleted");

    let dispatched_n: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM ingest_outbox WHERE status='dispatched'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(dispatched_n, 1, "recent dispatched row preserved");

    let pending_n: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM ingest_outbox WHERE status='pending'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(pending_n, 1, "pending row preserved regardless of age");
}

#[sqlx::test(migrations = "../../migrations")]
async fn retention_deletes_old_usage_preserves_recent(pool: PgPool) {
    let tenant = insert_tenant(&pool, "ret-usage").await;

    sqlx::query(
        "INSERT INTO usage_events (tenant_id, metric, delta, created_at)
         VALUES ($1, 'llm_tokens', 1, $2)",
    )
    .bind(tenant)
    .bind(Utc::now() - Duration::days(100))
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO usage_events (tenant_id, metric, delta, created_at)
         VALUES ($1, 'llm_tokens', 1, $2)",
    )
    .bind(tenant)
    .bind(Utc::now() - Duration::days(10))
    .execute(&pool)
    .await
    .unwrap();

    let deleted = delete_usage_older_than(&pool, 90, 1000).await.unwrap();
    assert_eq!(deleted, 1);

    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM usage_events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 1, "recent usage row preserved");
}

#[sqlx::test(migrations = "../../migrations")]
async fn retention_deletes_old_audit_preserves_recent(pool: PgPool) {
    let tenant = insert_tenant(&pool, "ret-audit").await;
    let actor = insert_user(&pool, "actor@ret.com").await;

    sqlx::query(
        "INSERT INTO audit_log (tenant_id, actor_id, action, created_at)
         VALUES ($1, $2, 'x', $3)",
    )
    .bind(tenant)
    .bind(actor)
    .bind(Utc::now() - Duration::days(400))
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO audit_log (tenant_id, actor_id, action, created_at)
         VALUES ($1, $2, 'y', $3)",
    )
    .bind(tenant)
    .bind(actor)
    .bind(Utc::now() - Duration::days(10))
    .execute(&pool)
    .await
    .unwrap();

    let deleted = delete_audit_older_than(&pool, 365, 1000).await.unwrap();
    assert_eq!(deleted, 1);

    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_log")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 1, "recent audit row preserved");
}

#[sqlx::test(migrations = "../../migrations")]
async fn retention_batch_limit_is_respected(pool: PgPool) {
    let tenant = insert_tenant(&pool, "ret-batch").await;
    let doc = insert_document(&pool, tenant).await;
    let old = Utc::now() - Duration::days(40);

    // Insert 5 old dispatched rows; batch limit 2 → only 2 deleted per pass.
    for _ in 0..5 {
        insert_outbox(&pool, tenant, doc, "dispatched", Some(old)).await;
    }

    let deleted = delete_dispatched_outbox_older_than(&pool, 30, 2)
        .await
        .unwrap();
    assert_eq!(deleted, 2, "batch limit must cap the delete");

    let remaining: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM ingest_outbox WHERE status='dispatched'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(remaining, 3, "remaining rows stay for a later pass");
}

#[sqlx::test(migrations = "../../migrations")]
async fn run_retention_once_deletes_across_all_tables(pool: PgPool) {
    let tenant = insert_tenant(&pool, "ret-all").await;
    let doc = insert_document(&pool, tenant).await;
    let old = Utc::now() - Duration::days(400);

    insert_outbox(&pool, tenant, doc, "dispatched", Some(old)).await;
    sqlx::query(
        "INSERT INTO usage_events (tenant_id, metric, delta, created_at)
         VALUES ($1, 'llm_tokens', 1, $2)",
    )
    .bind(tenant)
    .bind(old)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO audit_log (tenant_id, action, created_at)
         VALUES ($1, 'x', $2)",
    )
    .bind(tenant)
    .bind(old)
    .execute(&pool)
    .await
    .unwrap();

    let total = run_retention_once(&pool, 30, 90, 365, 1000).await.unwrap();
    assert_eq!(total, 3, "one row deleted per table");
}
