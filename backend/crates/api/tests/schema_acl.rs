//! Schema verification for ACL cutover.

use sqlx::PgPool;

#[sqlx::test(migrations = "../../migrations")]
async fn resource_acl_table_is_removed_after_openfga_cutover(pool: PgPool) {
    let exists: bool = sqlx::query_scalar("SELECT to_regclass('public.resource_acl') IS NOT NULL")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        !exists,
        "resource_acl must be removed after OpenFGA cutover"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn invitations_table_remains(pool: PgPool) {
    let cols: Vec<String> = sqlx::query_scalar(
        "SELECT column_name FROM information_schema.columns
         WHERE table_name = 'invitations' ORDER BY column_name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    for required in ["id", "tenant_id", "email", "role", "token", "status"] {
        assert!(
            cols.iter().any(|c| c == required),
            "invitations missing column '{required}'"
        );
    }
}
