//! Integration test for the two-pool design (T19 Blocker 2 fix).
//!
//! Verifies that:
//! - `init_pool` connects as the superuser role (`gmrag`) — bypasses RLS.
//! - `init_app_pool` downgrades every connection to `gmrag_app` via
//!   `after_connect(SET ROLE gmrag_app)` — RLS is enforced.
//!
//! Requires a running PostgreSQL (Docker) with the `gmrag_app` role created
//! by `infra/postgres/init.sql` / the identity migration.

use gmrag_core::{init_app_pool, init_pool};

/// The role that `init_pool` connects as (the `DATABASE_URL` superuser).
/// In dev this is `gmrag`.
const ADMIN_ROLE: &str = "gmrag";
/// The downgraded role that `init_app_pool` enforces via `SET ROLE`.
const APP_ROLE: &str = "gmrag_app";

#[tokio::test]
async fn init_pool_connects_as_superuser_role() {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = init_pool(&url).await.expect("admin pool should connect");

    let role: String = sqlx::query_scalar("SELECT current_role")
        .fetch_one(&pool)
        .await
        .expect("current_role query");

    assert_eq!(
        role, ADMIN_ROLE,
        "init_pool must connect as the superuser '{ADMIN_ROLE}', got '{role}'"
    );
}

#[tokio::test]
async fn init_app_pool_downgrades_to_gmrag_app() {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = init_app_pool(&url).await.expect("app pool should connect");

    // Every connection in the pool must report gmrag_app (after_connect hook).
    // Acquire two distinct connections to be sure the hook applies to all.
    for _ in 0..2 {
        let mut conn = pool.acquire().await.expect("acquire connection");
        let role: String = sqlx::query_scalar("SELECT current_role")
            .fetch_one(&mut *conn)
            .await
            .expect("current_role query");
        assert_eq!(
            role, APP_ROLE,
            "init_app_pool connections must run as '{APP_ROLE}' (RLS-enforced), got '{role}'"
        );
    }
}

#[tokio::test]
async fn init_app_pool_enforces_rls_on_tenants() {
    // Smoke-test that RLS is actually in effect on the app pool: without
    // SET LOCAL app.tenant_id, gmrag_current_tenant() returns NULL and the
    // tenants RLS policy hides all rows (per T15 FORCE RLS + policy).
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = init_app_pool(&url).await.expect("app pool should connect");

    let mut conn = pool.acquire().await.expect("acquire connection");
    // Without a tenant context, the tenants policy (id = NULL) matches nothing.
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tenants")
        .fetch_one(&mut *conn)
        .await
        .expect("count query");
    assert_eq!(
        count, 0,
        "app pool with no tenant context must see zero tenants (RLS enforced), saw {count}"
    );

    // And setting a bogus tenant id must still hide real rows.
    sqlx::Executor::execute(
        &mut *conn,
        "SET LOCAL app.tenant_id = '00000000-0000-0000-0000-000000000000'",
    )
    .await
    .expect("set local");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tenants")
        .fetch_one(&mut *conn)
        .await
        .expect("count query");
    assert_eq!(
        count, 0,
        "app pool with a non-existent tenant id must see zero tenants, saw {count}"
    );
}
