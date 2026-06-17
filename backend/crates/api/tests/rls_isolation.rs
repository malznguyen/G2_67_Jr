//! Integration tests for RLS policies on the tenants table.
//!
//! These tests require a running PostgreSQL instance (Docker).
//! They use `#[sqlx::test]` which creates isolated test databases
//! and runs migrations automatically.
//!
//! The DATABASE_URL user is a superuser, which bypasses RLS.
//! Each test uses `SET LOCAL ROLE gmrag_app` to switch to the
//! non-superuser application role, making RLS policies effective.

use sqlx::PgPool;
use uuid::Uuid;

/// Helper: insert a tenant and return its id.
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

/// Helper: insert a user and return its id.
async fn create_user(pool: &PgPool, email: &str) -> Uuid {
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

/// Helper: add user to tenant.
async fn add_member(pool: &PgPool, tenant_id: Uuid, user_id: Uuid, role: &str) {
    sqlx::query("INSERT INTO tenant_members (tenant_id, user_id, role) VALUES ($1, $2, $3)")
        .bind(tenant_id)
        .bind(user_id)
        .bind(role)
        .execute(pool)
        .await
        .unwrap();
}

/// Helper: start a transaction with RLS active (as gmrag_app role).
async fn begin_rls_tx(pool: &PgPool) -> sqlx::Transaction<'static, sqlx::Postgres> {
    let mut tx = pool.begin().await.unwrap();
    sqlx::Executor::execute(&mut *tx, "SET LOCAL ROLE gmrag_app")
        .await
        .unwrap();
    tx
}

/// Helper: set tenant context within a transaction.
async fn set_tenant(tx: &mut sqlx::Transaction<'static, sqlx::Postgres>, tenant_id: Uuid) {
    sqlx::query(&format!("SET LOCAL app.tenant_id = '{}'", tenant_id))
        .execute(&mut **tx)
        .await
        .unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn rls_tenants_only_shows_current_tenant(pool: PgPool) {
    let tenant_a = create_tenant(&pool, "Tenant A").await;
    let tenant_b = create_tenant(&pool, "Tenant B").await;

    // Query with tenant_a context.
    let mut tx = begin_rls_tx(&pool).await;
    set_tenant(&mut tx, tenant_a).await;

    let rows: Vec<(Uuid, String)> = sqlx::query_as("SELECT id, name FROM tenants")
        .fetch_all(&mut *tx)
        .await
        .unwrap();

    assert_eq!(rows.len(), 1, "should only see tenant_a");
    assert_eq!(rows[0].0, tenant_a);
    assert_eq!(rows[0].1, "Tenant A");

    tx.rollback().await.unwrap();

    // Query with tenant_b context.
    let mut tx = begin_rls_tx(&pool).await;
    set_tenant(&mut tx, tenant_b).await;

    let rows: Vec<(Uuid, String)> = sqlx::query_as("SELECT id, name FROM tenants")
        .fetch_all(&mut *tx)
        .await
        .unwrap();

    assert_eq!(rows.len(), 1, "should only see tenant_b");
    assert_eq!(rows[0].0, tenant_b);
    assert_eq!(rows[0].1, "Tenant B");

    tx.rollback().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn rls_tenants_blocks_cross_tenant_access(pool: PgPool) {
    let tenant_a = create_tenant(&pool, "Tenant A").await;
    let tenant_b = create_tenant(&pool, "Tenant B").await;

    // Query with tenant_a context — should NOT see tenant_b.
    let mut tx = begin_rls_tx(&pool).await;
    set_tenant(&mut tx, tenant_a).await;

    let rows: Vec<(Uuid,)> = sqlx::query_as("SELECT id FROM tenants WHERE id = $1")
        .bind(tenant_b)
        .fetch_all(&mut *tx)
        .await
        .unwrap();

    assert!(
        rows.is_empty(),
        "tenant_b should NOT be visible when app.tenant_id = tenant_a"
    );

    tx.rollback().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn rls_tenants_no_context_returns_nothing(pool: PgPool) {
    let _tenant_a = create_tenant(&pool, "Tenant A").await;

    // Without SET LOCAL app.tenant_id, gmrag_current_tenant() returns NULL.
    // RLS policy: id = NULL → no rows match.
    let mut tx = begin_rls_tx(&pool).await;

    let rows: Vec<(Uuid,)> = sqlx::query_as("SELECT id FROM tenants")
        .fetch_all(&mut *tx)
        .await
        .unwrap();

    assert!(
        rows.is_empty(),
        "without tenant context, no tenants should be visible, got {} rows",
        rows.len()
    );

    tx.rollback().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn rls_tenants_multiple_tenants_visible_in_sequence(pool: PgPool) {
    let tenant_a = create_tenant(&pool, "Tenant A").await;
    let tenant_b = create_tenant(&pool, "Tenant B").await;

    // Switch context to A, verify A visible.
    let mut tx = begin_rls_tx(&pool).await;
    set_tenant(&mut tx, tenant_a).await;

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tenants")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(count, 1);

    tx.commit().await.unwrap();

    // Switch context to B, verify B visible.
    let mut tx = begin_rls_tx(&pool).await;
    set_tenant(&mut tx, tenant_b).await;

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tenants")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(count, 1);

    tx.rollback().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn rls_tenant_members_isolation(pool: PgPool) {
    let tenant_a = create_tenant(&pool, "Tenant A").await;
    let tenant_b = create_tenant(&pool, "Tenant B").await;
    let user_a = create_user(&pool, "a@test.com").await;
    let user_b = create_user(&pool, "b@test.com").await;

    add_member(&pool, tenant_a, user_a, "member").await;
    add_member(&pool, tenant_b, user_b, "member").await;

    // With tenant_a context, should only see user_a's membership.
    let mut tx = begin_rls_tx(&pool).await;
    set_tenant(&mut tx, tenant_a).await;

    let members: Vec<(Uuid,)> = sqlx::query_as("SELECT user_id FROM tenant_members")
        .fetch_all(&mut *tx)
        .await
        .unwrap();

    assert_eq!(members.len(), 1);
    assert_eq!(members[0].0, user_a);

    tx.rollback().await.unwrap();
}
