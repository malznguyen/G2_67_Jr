//! Schema verification tests for T24 (tenant_quotas + usage_events +
//! audit_log + ingest_jobs).

use sqlx::PgPool;

#[sqlx::test(migrations = "../../migrations")]
async fn tenant_quotas_pk_is_tenant_id(pool: PgPool) {
    let row: (String,) = sqlx::query_as(
        "SELECT column_name FROM information_schema.columns
         WHERE table_name = 'tenant_quotas' AND column_name = 'tenant_id'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.0, "tenant_id");

    // tenant_id must be the primary key (1:1 with tenants).
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pg_constraint
         WHERE conrelid = 'tenant_quotas'::regclass AND contype = 'p'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "tenant_quotas must have a PRIMARY KEY constraint");

    // default storage = 10 GiB
    let row: (Option<String>,) = sqlx::query_as(
        "SELECT column_default FROM information_schema.columns
         WHERE table_name = 'tenant_quotas' AND column_name = 'max_storage_bytes'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let default = row.0.unwrap_or_default();
    assert!(
        default.contains("10737418240"),
        "max_storage_bytes default must be 10737418240 (10 GiB), got '{default}'"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn usage_events_and_audit_log_have_jsonb_metadata(pool: PgPool) {
    for table in ["usage_events", "audit_log"] {
        let row: (String,) = sqlx::query_as(&format!(
            "SELECT data_type FROM information_schema.columns
             WHERE table_name = '{table}' AND column_name = 'metadata'"
        ))
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            row.0, "jsonb",
            "{table}.metadata must be JSONB, got {}",
            row.0
        );
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn ingest_jobs_has_status_and_attempts_defaults(pool: PgPool) {
    let status_default: (Option<String>,) = sqlx::query_as(
        "SELECT column_default FROM information_schema.columns
         WHERE table_name = 'ingest_jobs' AND column_name = 'status'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        status_default.0.as_deref().unwrap_or("").contains("pending"),
        "ingest_jobs.status must default to 'pending'"
    );

    let attempts_default: (Option<String>,) = sqlx::query_as(
        "SELECT column_default FROM information_schema.columns
         WHERE table_name = 'ingest_jobs' AND column_name = 'attempts'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        attempts_default.0.as_deref().unwrap_or("").contains("0"),
        "ingest_jobs.attempts must default to 0"
    );

    // document_id FK must exist (ingest_jobs references documents).
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.table_constraints
         WHERE table_name = 'ingest_jobs' AND constraint_type = 'FOREIGN KEY'
           AND constraint_name = 'ingest_jobs_document_id_fkey'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "ingest_jobs.document_id FK must exist");
}
