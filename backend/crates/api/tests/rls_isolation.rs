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

// ─── T25: cross-tenant isolation on domain tables (T19-T24) ──────────────
// These tests verify that RLS policies block cross-tenant data leaks on
// every new tenant-scoped table. Before the rls_apply_all migration, the
// gmrag_app role sees ALL rows (no policy) → these tests FAIL (red).

/// Helper: insert a workspace and return its id.
async fn create_workspace(pool: &PgPool, tenant_id: Uuid, name: &str, user_id: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workspaces (id, tenant_id, name, slug, created_by)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(tenant_id)
    .bind(name)
    .bind(format!("slug-{}", name))
    .bind(user_id)
    .execute(pool)
    .await
    .unwrap();
    id
}

/// Helper: insert a document and return its id.
async fn create_document(pool: &PgPool, tenant_id: Uuid, owner_id: Uuid, title: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO documents (id, tenant_id, owner_id, title)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(id)
    .bind(tenant_id)
    .bind(owner_id)
    .bind(title)
    .execute(pool)
    .await
    .unwrap();
    id
}

#[sqlx::test(migrations = "../../migrations")]
async fn rls_workspaces_blocks_cross_tenant_access(pool: PgPool) {
    // Diagnostic: check if RLS policy exists in the test DB.
    let policy_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pg_policy WHERE polrelid = 'workspaces'::regclass",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(policy_count, 1, "workspaces_isolation policy must exist in test DB");

    let rls_enabled: bool = sqlx::query_scalar(
        "SELECT relrowsecurity FROM pg_class WHERE relname = 'workspaces'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(rls_enabled, "RLS must be enabled on workspaces in test DB");

    let tenant_a = create_tenant(&pool, "Tenant A").await;
    let tenant_b = create_tenant(&pool, "Tenant B").await;
    let user_a = create_user(&pool, "a@ws.com").await;

    let ws_a = create_workspace(&pool, tenant_a, "WS A", user_a).await;
    let _ws_b = create_workspace(&pool, tenant_b, "WS B", user_a).await;

    // Diagnostic: check current_role after SET LOCAL ROLE
    let mut tx = begin_rls_tx(&pool).await;
    let role: String = sqlx::query_scalar("SELECT current_role")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(role, "gmrag_app", "SET LOCAL ROLE must switch to gmrag_app");

    set_tenant(&mut tx, tenant_a).await;

    let rows: Vec<(Uuid,)> = sqlx::query_as("SELECT id FROM workspaces")
        .fetch_all(&mut *tx)
        .await
        .unwrap();

    assert_eq!(rows.len(), 1, "should only see tenant_a's workspace");
    assert_eq!(rows[0].0, ws_a);

    tx.rollback().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn rls_documents_blocks_cross_tenant_access(pool: PgPool) {
    let tenant_a = create_tenant(&pool, "Tenant A").await;
    let tenant_b = create_tenant(&pool, "Tenant B").await;
    let user_a = create_user(&pool, "a@doc.com").await;

    let doc_a = create_document(&pool, tenant_a, user_a, "Doc A").await;
    let _doc_b = create_document(&pool, tenant_b, user_a, "Doc B").await;

    let mut tx = begin_rls_tx(&pool).await;
    set_tenant(&mut tx, tenant_a).await;

    let rows: Vec<(Uuid,)> = sqlx::query_as("SELECT id FROM documents")
        .fetch_all(&mut *tx)
        .await
        .unwrap();

    assert_eq!(rows.len(), 1, "should only see tenant_a's document");
    assert_eq!(rows[0].0, doc_a);

    tx.rollback().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn rls_chat_sessions_blocks_cross_tenant_access(pool: PgPool) {
    let tenant_a = create_tenant(&pool, "Tenant A").await;
    let tenant_b = create_tenant(&pool, "Tenant B").await;
    let user_a = create_user(&pool, "a@chat.com").await;

    sqlx::query("INSERT INTO chat_sessions (tenant_id, user_id, title) VALUES ($1, $2, 'Sess A')")
        .bind(tenant_a)
        .bind(user_a)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO chat_sessions (tenant_id, user_id, title) VALUES ($1, $2, 'Sess B')")
        .bind(tenant_b)
        .bind(user_a)
        .execute(&pool)
        .await
        .unwrap();

    let mut tx = begin_rls_tx(&pool).await;
    set_tenant(&mut tx, tenant_a).await;

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chat_sessions")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(count, 1, "should only see tenant_a's chat session");

    tx.rollback().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn rls_audit_log_blocks_cross_tenant_access(pool: PgPool) {
    let tenant_a = create_tenant(&pool, "Tenant A").await;
    let tenant_b = create_tenant(&pool, "Tenant B").await;

    sqlx::query("INSERT INTO audit_log (tenant_id, action) VALUES ($1, 'login')")
        .bind(tenant_a)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO audit_log (tenant_id, action) VALUES ($1, 'login')")
        .bind(tenant_b)
        .execute(&pool)
        .await
        .unwrap();

    let mut tx = begin_rls_tx(&pool).await;
    set_tenant(&mut tx, tenant_a).await;

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_log")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(count, 1, "should only see tenant_a's audit log entry");

    tx.rollback().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn rls_ingest_jobs_blocks_cross_tenant_access(pool: PgPool) {
    let tenant_a = create_tenant(&pool, "Tenant A").await;
    let tenant_b = create_tenant(&pool, "Tenant B").await;
    let user_a = create_user(&pool, "a@ingest.com").await;

    let doc_a = create_document(&pool, tenant_a, user_a, "Doc A").await;
    let doc_b = create_document(&pool, tenant_b, user_a, "Doc B").await;

    sqlx::query("INSERT INTO ingest_jobs (tenant_id, document_id) VALUES ($1, $2)")
        .bind(tenant_a)
        .bind(doc_a)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO ingest_jobs (tenant_id, document_id) VALUES ($1, $2)")
        .bind(tenant_b)
        .bind(doc_b)
        .execute(&pool)
        .await
        .unwrap();

    let mut tx = begin_rls_tx(&pool).await;
    set_tenant(&mut tx, tenant_a).await;

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ingest_jobs")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(count, 1, "should only see tenant_a's ingest job");

    tx.rollback().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn rls_graph_nodes_blocks_cross_tenant_access(pool: PgPool) {
    let tenant_a = create_tenant(&pool, "Tenant A").await;
    let tenant_b = create_tenant(&pool, "Tenant B").await;

    sqlx::query("INSERT INTO graph_nodes (tenant_id, kind, label) VALUES ($1, 'concept', 'Node A')")
        .bind(tenant_a)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO graph_nodes (tenant_id, kind, label) VALUES ($1, 'concept', 'Node B')")
        .bind(tenant_b)
        .execute(&pool)
        .await
        .unwrap();

    let mut tx = begin_rls_tx(&pool).await;
    set_tenant(&mut tx, tenant_a).await;

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM graph_nodes")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(count, 1, "should only see tenant_a's graph node");

    tx.rollback().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn rls_invitations_blocks_cross_tenant_access(pool: PgPool) {
    let tenant_a = create_tenant(&pool, "Tenant A").await;
    let tenant_b = create_tenant(&pool, "Tenant B").await;
    let user_a = create_user(&pool, "a@inv.com").await;

    sqlx::query("INSERT INTO invitations (tenant_id, email, invited_by) VALUES ($1, 'x@a.com', $2)")
        .bind(tenant_a)
        .bind(user_a)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO invitations (tenant_id, email, invited_by) VALUES ($1, 'y@b.com', $2)")
        .bind(tenant_b)
        .bind(user_a)
        .execute(&pool)
        .await
        .unwrap();

    let mut tx = begin_rls_tx(&pool).await;
    set_tenant(&mut tx, tenant_a).await;

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM invitations")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(count, 1, "should only see tenant_a's invitation");

    tx.rollback().await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn rls_no_context_hides_all_domain_tables(pool: PgPool) {
    // Without SET LOCAL app.tenant_id, every tenant-scoped table must
    // return zero rows (gmrag_current_tenant() = NULL → policy matches nothing).
    let tenant_a = create_tenant(&pool, "Tenant A").await;
    let user_a = create_user(&pool, "a@none.com").await;
    create_workspace(&pool, tenant_a, "WS", user_a).await;
    create_document(&pool, tenant_a, user_a, "Doc").await;

    let mut tx = begin_rls_tx(&pool).await;
    // No set_tenant call — app.tenant_id is unset.

    for table in [
        "workspaces",
        "workspace_members",
        "documents",
        "document_chunks",
        "graph_nodes",
        "graph_edges",
        "chat_sessions",
        "chat_messages",
        "resource_acl",
        "invitations",
        "tenant_quotas",
        "usage_events",
        "audit_log",
        "ingest_jobs",
    ] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(
            count, 0,
            "{table} must return 0 rows without tenant context (RLS), got {count}"
        );
    }

    tx.rollback().await.unwrap();
}
