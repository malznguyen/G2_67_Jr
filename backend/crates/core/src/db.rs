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
/// `max_connections` is read from the `DATABASE_MAX_CONNECTIONS` process env
/// (default 10). Reading the env internally keeps the call signature stable
/// so existing call sites (`init_pool(&cfg.database_url)`) need no churn.
pub async fn init_pool(database_url: &str) -> Result<PgPool> {
    let max_connections = env_max_connections();
    let pool = PgPoolOptions::new()
        .max_connections(max_connections)
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
    let max_connections = env_max_connections();
    let pool = PgPoolOptions::new()
        .max_connections(max_connections)
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

/// Read `DATABASE_MAX_CONNECTIONS` from the process env (default 10).
/// Used internally by `init_pool` / `init_app_pool`.
fn env_max_connections() -> u32 {
    std::env::var("DATABASE_MAX_CONNECTIONS")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(10)
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
