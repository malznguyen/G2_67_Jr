//! Configuration loaded from environment variables.
//!
//! `Config::from_env()` reads the variables declared in `.env.example` and
//! fails fast on missing required values. Defaults are kept minimal — only
//! fields with a true safe default (logging level, bind address) get one.
//!
//! Multi-tenant invariant (per project README): the tenant identity used in
//! business logic is NEVER pulled from a URL; it always comes from the
//! resolved request context (TenantContext middleware, added in later tasks).
//! Therefore `Config` does not expose a `tenant_id` field. The single
//! `GMRAG_DEFAULT_TENANT` placeholder is reserved for dev seeding only and is
//! not wired into request handling here.

use std::env;
use std::net::SocketAddr;

use crate::error::Error;

/// Default HTTP bind address when `GMRAG_HTTP_BIND` is unset.
const DEFAULT_HTTP_BIND: &str = "0.0.0.0:8080";

/// Default `RUST_LOG`-style filter when `GMRAG_RUST_LOG` is unset.
const DEFAULT_LOG_FILTER: &str = "info,gmrag_core=debug,gmrag_api=debug";

/// Application configuration resolved at startup.
#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub http_bind: SocketAddr,
    pub log_filter: String,
    pub tenant_header: String,
    pub service_name: String,
}

impl Config {
    /// Load configuration from process environment.
    ///
    /// Order of precedence:
    /// 1. Real environment variables (already set when running under
    ///    docker-compose / Kubernetes / systemd).
    /// 2. `.env` file in the current working directory (loaded by `dotenvy`
    ///    if present — never required).
    pub fn from_env() -> Result<Self, Error> {
        // Best-effort .env load. Ignore the error if no .env exists.
        let _ = dotenvy::dotenv();

        let database_url = require_env("DATABASE_URL")?;
        let http_bind_raw = env::var("GMRAG_HTTP_BIND")
            .unwrap_or_else(|_| DEFAULT_HTTP_BIND.to_string());
        let http_bind: SocketAddr = http_bind_raw
            .parse()
            .map_err(|e| Error::Config(format!("invalid GMRAG_HTTP_BIND '{http_bind_raw}': {e}")))?;
        let log_filter = env::var("GMRAG_RUST_LOG")
            .unwrap_or_else(|_| DEFAULT_LOG_FILTER.to_string());
        let tenant_header = env::var("GMRAG_TENANT_HEADER")
            .unwrap_or_else(|_| "X-Tenant-ID".to_string());
        let service_name =
            env::var("GMRAG_SERVICE_NAME").unwrap_or_else(|_| "gmrag-api".to_string());

        Ok(Self {
            database_url,
            http_bind,
            log_filter,
            tenant_header,
            service_name,
        })
    }

    /// Returns the effective HTTP listen address as a string.
    pub fn bind_address(&self) -> String {
        self.http_bind.to_string()
    }
}

fn require_env(key: &'static str) -> Result<String, Error> {
    match env::var(key) {
        Ok(v) if !v.trim().is_empty() => Ok(v),
        Ok(_) => Err(Error::Config(format!("environment variable {key} is empty"))),
        Err(_) => Err(Error::Config(format!("environment variable {key} is required"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Mutex guarding process env mutation in tests. `cargo test` runs tests
    /// in parallel by default; env mutation is process-wide, so we serialize.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// All cases touch the process env, so they share one serialized test.
    #[test]
    fn config_env_matrix() {
        let _guard = ENV_LOCK.lock().unwrap();

        // Save originals.
        let orig_db = env::var("DATABASE_URL").ok();
        let orig_bind = env::var("GMRAG_HTTP_BIND").ok();
        let orig_tenant = env::var("GMRAG_TENANT_HEADER").ok();

        // -- Case 1: missing DATABASE_URL must fail loudly.
        env::remove_var("DATABASE_URL");
        let res = Config::from_env();
        assert!(
            matches!(res, Err(Error::Config(ref msg)) if msg.contains("DATABASE_URL")),
            "missing DATABASE_URL must surface as Error::Config, got {res:?}"
        );

        // -- Case 2: minimal env (DATABASE_URL only) parses with defaults.
        env::set_var("DATABASE_URL", "postgres://u:p@h:5432/d");
        env::remove_var("GMRAG_HTTP_BIND");
        env::remove_var("GMRAG_TENANT_HEADER");
        let cfg = Config::from_env().expect("config should parse");
        assert_eq!(cfg.database_url, "postgres://u:p@h:5432/d");
        assert_eq!(cfg.http_bind.to_string(), DEFAULT_HTTP_BIND);
        assert_eq!(cfg.tenant_header, "X-Tenant-ID");

        // -- Case 3: empty DATABASE_URL also fails.
        env::set_var("DATABASE_URL", "   ");
        let res = Config::from_env();
        assert!(
            matches!(res, Err(Error::Config(_))),
            "empty DATABASE_URL must surface as Error::Config, got {res:?}"
        );

        // Restore originals (best-effort).
        match orig_db {
            Some(v) => env::set_var("DATABASE_URL", v),
            None => env::remove_var("DATABASE_URL"),
        }
        match orig_bind {
            Some(v) => env::set_var("GMRAG_HTTP_BIND", v),
            None => env::remove_var("GMRAG_HTTP_BIND"),
        }
        match orig_tenant {
            Some(v) => env::set_var("GMRAG_TENANT_HEADER", v),
            None => env::remove_var("GMRAG_TENANT_HEADER"),
        }
    }
}
