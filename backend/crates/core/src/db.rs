//! Postgres connection pool helper.
//!
//! `init_pool` creates a `sqlx::PgPool` sized for the runtime (api vs worker).
//! Pool size is kept conservative for T5-T7 — tuning belongs to later perf
//! tasks. The same helper is used by both `gmrag-api` and `gmrag-worker`, so
//! any future change to pool semantics happens in one place.

use std::time::Duration;

use sqlx::postgres::{PgPool, PgPoolOptions};

use crate::error::{Error, Result};

/// Build a Postgres connection pool from a `DATABASE_URL`.
///
/// This pool connects as the role encoded in `DATABASE_URL` (the `gmrag`
/// superuser in dev) and does **not** downgrade the role. It is therefore
/// suitable for operations that must bypass RLS: running migrations,
/// platform-level provisioning, cross-tenant membership lookups, and the
/// worker's platform-level queries.
///
/// Tenant-scoped request handling MUST use [`init_app_pool`] instead, which
/// downgrades every connection to the `gmrag_app` role so PostgreSQL RLS
/// policies are enforced.
///
/// `max_connections` defaults to 10 — sufficient for skeleton phase. Worker
/// processes that want a different ceiling should override via
/// `PgPoolOptions::max_connections` directly (added in later tasks).
pub async fn init_pool(database_url: &str) -> Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .min_connections(1)
        .acquire_timeout(Duration::from_secs(5))
        .idle_timeout(Some(Duration::from_secs(300)))
        .connect(database_url)
        .await
        .map_err(Error::from)?;

    // Cheap liveness check — surfaces a clean error early if creds / DNS / TLS are wrong.
    sqlx::query("SELECT 1").execute(&pool).await?;
    Ok(pool)
}

/// Build a Postgres connection pool that enforces RLS.
///
/// Identical sizing to [`init_pool`], but installs an `after_connect` hook
/// that executes `SET ROLE gmrag_app` on every fresh connection. The
/// `gmrag_app` role is a non-superuser (created in `infra/postgres/init.sql`
/// and re-created idempotently in the identity migration) so PostgreSQL
/// Row Level Security policies are enforced for all queries on this pool.
///
/// `app.tenant_id` must still be set per-transaction (e.g. via
/// `SET LOCAL app.tenant_id = '<uuid>'`) by the RLS middleware for
/// `gmrag_current_tenant()` to return the active tenant.
pub async fn init_app_pool(database_url: &str) -> Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .min_connections(1)
        .acquire_timeout(Duration::from_secs(5))
        .idle_timeout(Some(Duration::from_secs(300)))
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                use sqlx::Executor as _;
                conn.execute("SET ROLE gmrag_app").await.map(|_| ())
            })
        })
        .connect(database_url)
        .await
        .map_err(Error::from)?;

    // Liveness check on the downgraded role.
    sqlx::query("SELECT 1").execute(&pool).await?;
    Ok(pool)
}

/// Type alias re-exported so consumers don't have to depend on `sqlx` directly.
pub type DbPool = PgPool;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_pool_alias_is_pgpool() {
        // Compile-time check: DbPool must be the same type as PgPool.
        fn _assert_same_type(_: DbPool) -> PgPool {
            unreachable!()
        }
    }
}
