//! Schema verification tests for T64 (ReBAC relation-tuple semantics over
//! `resource_acl`, Zanzibar-style — paper `docs/5068.pdf`).
//!
//! The migration `rebac_relation_tuples` gives `resource_acl.permission`
//! relation semantics (`owner|editor|viewer`), constrains `principal_type` to
//! the supported subject namespaces (`user|workspace`), sets the default
//! relation to `viewer`, and adds a covering index for the Check hot path.

use sqlx::PgPool;
use uuid::Uuid;
// (migration: 20260622000000_rebac_relation_tuples)

async fn insert_tenant(pool: &PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, $2)")
        .bind(id)
        .bind("Acme")
        .execute(pool)
        .await
        .unwrap();
    id
}

#[sqlx::test(migrations = "../../migrations")]
async fn resource_acl_has_check_covering_index(pool: PgPool) {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pg_indexes
         WHERE tablename = 'resource_acl'
           AND indexname = 'idx_resource_acl_check'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        count, 1,
        "resource_acl must have idx_resource_acl_check covering index for Check evaluation"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn resource_acl_permission_defaults_to_viewer(pool: PgPool) {
    let row: (Option<String>,) = sqlx::query_as(
        "SELECT column_default FROM information_schema.columns
         WHERE table_name = 'resource_acl' AND column_name = 'permission'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let default = row.0.unwrap_or_default();
    assert!(
        default.contains("viewer"),
        "resource_acl.permission must default to 'viewer', got '{default}'"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn resource_acl_accepts_valid_relations(pool: PgPool) {
    let tenant = insert_tenant(&pool).await;
    for relation in ["owner", "editor", "viewer"] {
        let res = sqlx::query(
            "INSERT INTO resource_acl
               (tenant_id, resource_type, resource_id, principal_type, principal_id, permission)
             VALUES ($1, 'document', $2, 'user', $3, $4)",
        )
        .bind(tenant)
        .bind(Uuid::new_v4())
        .bind(Uuid::new_v4())
        .bind(relation)
        .execute(&pool)
        .await;
        assert!(res.is_ok(), "relation '{relation}' must be accepted");
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn resource_acl_rejects_unknown_relation(pool: PgPool) {
    let tenant = insert_tenant(&pool).await;
    let res = sqlx::query(
        "INSERT INTO resource_acl
           (tenant_id, resource_type, resource_id, principal_type, principal_id, permission)
         VALUES ($1, 'document', $2, 'user', $3, 'superuser')",
    )
    .bind(tenant)
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await;
    assert!(
        res.is_err(),
        "an unknown relation must be rejected by the CHECK constraint"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn resource_acl_accepts_workspace_principal(pool: PgPool) {
    let tenant = insert_tenant(&pool).await;
    let res = sqlx::query(
        "INSERT INTO resource_acl
           (tenant_id, resource_type, resource_id, principal_type, principal_id, permission)
         VALUES ($1, 'document', $2, 'workspace', $3, 'viewer')",
    )
    .bind(tenant)
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await;
    assert!(
        res.is_ok(),
        "a 'workspace' (group) principal must be accepted for shared grants"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn resource_acl_rejects_unknown_principal_type(pool: PgPool) {
    let tenant = insert_tenant(&pool).await;
    let res = sqlx::query(
        "INSERT INTO resource_acl
           (tenant_id, resource_type, resource_id, principal_type, principal_id, permission)
         VALUES ($1, 'document', $2, 'robot', $3, 'viewer')",
    )
    .bind(tenant)
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await;
    assert!(
        res.is_err(),
        "an unknown principal_type must be rejected by the CHECK constraint"
    );
}
