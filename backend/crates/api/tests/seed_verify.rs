//! T26: Verify the dev seed script runs without FK/RLS errors.
//!
//! Reads `infra/postgres/seed.sql`, executes it against a fresh test
//! database (migrations already applied by `#[sqlx::test]`), and asserts
//! the expected row counts for each table.

use sqlx::PgPool;

#[sqlx::test(migrations = "../../migrations")]
async fn seed_script_runs_clean_and_populates_expected_counts(pool: PgPool) {
    let seed_sql = include_str!("../../../../infra/postgres/seed.sql");

    // Execute the seed script. It contains BEGIN/COMMIT + INSERTs + a
    // summary SELECT. sqlx::raw_sql runs all statements in order.
    sqlx::raw_sql(seed_sql).execute(&pool).await.unwrap();

    // Verify expected row counts.
    let tenant_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tenants")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(tenant_count, 2, "seed should create 2 tenants");

    let user_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(user_count, 3, "seed should create 3 users");

    let workspace_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workspaces")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(workspace_count, 2, "seed should create 2 workspaces");

    let doc_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM documents")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(doc_count, 3, "seed should create 3 documents");

    let chunk_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM document_chunks")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(chunk_count, 5, "seed should create 5 document chunks");

    let chat_msg_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chat_messages")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(chat_msg_count, 2, "seed should create 2 chat messages");

    let quota_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tenant_quotas")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(quota_count, 2, "seed should create 2 tenant quotas");

    let ingest_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ingest_jobs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(ingest_count, 1, "seed should create 1 ingest job");
}

#[sqlx::test(migrations = "../../migrations")]
async fn seed_script_is_idempotent(pool: PgPool) {
    let seed_sql = include_str!("../../../../infra/postgres/seed.sql");

    // Run the seed twice — ON CONFLICT DO NOTHING must prevent duplicates.
    sqlx::raw_sql(seed_sql).execute(&pool).await.unwrap();
    sqlx::raw_sql(seed_sql).execute(&pool).await.unwrap();

    let tenant_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tenants")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        tenant_count, 2,
        "re-running seed must not duplicate tenants"
    );

    let doc_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM documents")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(doc_count, 3, "re-running seed must not duplicate documents");
}
