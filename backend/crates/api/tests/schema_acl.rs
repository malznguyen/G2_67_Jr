//! Schema verification tests for T23 (resource_acl + invitations).

use sqlx::PgPool;

#[sqlx::test(migrations = "../../migrations")]
async fn resource_acl_table_exists_with_expected_columns(pool: PgPool) {
    let cols: Vec<(String,)> = sqlx::query_as(
        "SELECT column_name FROM information_schema.columns
         WHERE table_name = 'resource_acl' ORDER BY column_name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let names: Vec<String> = cols.into_iter().map(|c| c.0).collect();
    for required in [
        "id", "tenant_id", "resource_type", "resource_id", "principal_type",
        "principal_id", "permission", "created_at",
    ] {
        assert!(
            names.contains(&required.to_string()),
            "resource_acl missing column '{required}'"
        );
    }

    // UNIQUE constraint on the polymorphic composite key.
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pg_indexes
         WHERE tablename = 'resource_acl'
           AND indexdef LIKE '%resource_type, resource_id, principal_type, principal_id, permission%'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        count, 1,
        "resource_acl must have UNIQUE(resource_type, resource_id, principal_type, principal_id, permission)"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn invitations_table_has_token_default_and_status(pool: PgPool) {
    // token must default to gen_random_uuid()
    let row: (String,) = sqlx::query_as(
        "SELECT column_default FROM information_schema.columns
         WHERE table_name = 'invitations' AND column_name = 'token'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        row.0.contains("gen_random_uuid"),
        "invitations.token must default to gen_random_uuid(), got '{}'",
        row.0
    );

    // status default must be 'pending'
    let row: (Option<String>,) = sqlx::query_as(
        "SELECT column_default FROM information_schema.columns
         WHERE table_name = 'invitations' AND column_name = 'status'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let default = row.0.unwrap_or_default();
    assert!(
        default.contains("pending"),
        "invitations.status must default to 'pending', got '{}'",
        default
    );

    // Core columns present.
    let cols: Vec<(String,)> = sqlx::query_as(
        "SELECT column_name FROM information_schema.columns
         WHERE table_name = 'invitations' ORDER BY column_name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let names: Vec<String> = cols.into_iter().map(|c| c.0).collect();
    for required in [
        "id", "tenant_id", "workspace_id", "email", "role", "token",
        "status", "invited_by", "expires_at", "created_at", "accepted_at",
    ] {
        assert!(
            names.contains(&required.to_string()),
            "invitations missing column '{required}'"
        );
    }
}
