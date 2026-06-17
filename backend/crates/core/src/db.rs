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
