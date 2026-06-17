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

const DEFAULT_HTTP_BIND: &str = "0.0.0.0:8080";
const DEFAULT_LOG_FILTER: &str = "info,gmrag_core=debug,gmrag_api=debug";
const DEFAULT_TENANT_HEADER: &str = "X-Tenant-ID";
const DEFAULT_QDRANT_URL: &str = "http://localhost:6333";
const DEFAULT_S3_REGION: &str = "us-east-1";
const DEFAULT_S3_FORCE_PATH_STYLE: bool = true;
const DEFAULT_REDIS_URL: &str = "redis://localhost:6379/0";
const DEFAULT_OLLAMA_HOST: &str = "http://localhost:11434";
const DEFAULT_DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com/v1";
const DEFAULT_DEEPSEEK_MODEL: &str = "deepseek-v4-flash";
const DEFAULT_DEEPSEEK_TIMEOUT_S: u64 = 60;

/// OIDC / Keycloak configuration.
#[derive(Debug, Clone)]
pub struct OidcConfig {
    pub issuer: String,
    pub client_id: String,
    pub client_secret: String,
    pub frontend_client_id: String,
}

/// Qdrant vector DB configuration.
#[derive(Debug, Clone)]
pub struct QdrantConfig {
    pub url: String,
    pub api_key: Option<String>,
    pub collection_default: String,
}

/// S3 / MinIO object storage configuration.
#[derive(Debug, Clone)]
pub struct S3Config {
    pub endpoint: String,
    pub public_endpoint: String,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
    pub bucket: String,
    pub force_path_style: bool,
}

/// Redis configuration.
#[derive(Debug, Clone)]
pub struct RedisConfig {
    pub url: String,
}

/// Ollama LLM / embedding configuration.
#[derive(Debug, Clone)]
pub struct OllamaConfig {
    pub host: String,
    pub embed_model: String,
    pub llm_model: String,
    pub keep_alive: String,
}

/// DeepSeek remote LLM configuration (optional).
#[derive(Debug, Clone)]
pub struct DeepSeekConfig {
    pub api_key: Option<String>,
    pub base_url: String,
    pub model: String,
    pub timeout_s: u64,
}

/// Application configuration resolved at startup.
#[derive(Debug, Clone)]
pub struct Config {
    // Core
    pub database_url: String,
    pub http_bind: SocketAddr,
    pub log_filter: String,
    pub tenant_header: String,
    pub service_name: String,

    // Subsystems
    pub oidc: OidcConfig,
    pub qdrant: QdrantConfig,
    pub s3: S3Config,
    pub redis: RedisConfig,
    pub ollama: OllamaConfig,
    pub deepseek: DeepSeekConfig,
}

impl Config {
    /// Load configuration from process environment with best-effort `.env` file.
    pub fn from_env() -> Result<Self, Error> {
        let _ = dotenvy::dotenv();
        Self::from_process_env()
    }

    /// Build config from process environment variables only (no `.env` file).
    /// Used internally and by tests to avoid `.env` interference.
    pub(crate) fn from_process_env() -> Result<Self, Error> {
        // Core
        let database_url = require_env("DATABASE_URL")?;
        let http_bind_raw = optional_env("GMRAG_HTTP_BIND", DEFAULT_HTTP_BIND);
        let http_bind: SocketAddr = http_bind_raw
            .parse()
            .map_err(|e| Error::Config(format!("invalid GMRAG_HTTP_BIND '{http_bind_raw}': {e}")))?;
        let log_filter = optional_env("GMRAG_RUST_LOG", DEFAULT_LOG_FILTER);
        let tenant_header = optional_env("GMRAG_TENANT_HEADER", DEFAULT_TENANT_HEADER);
        let service_name = optional_env("GMRAG_SERVICE_NAME", "gmrag-api");

        // OIDC / Keycloak
        let oidc = OidcConfig {
            issuer: require_env("KEYCLOAK_ISSUER")?,
            client_id: require_env("KEYCLOAK_CLIENT_ID")?,
            client_secret: require_env("KEYCLOAK_CLIENT_SECRET")?,
            frontend_client_id: optional_env("KEYCLOAK_FRONTEND_CLIENT_ID", "gmrag-frontend"),
        };

        // Qdrant
        let qdrant = QdrantConfig {
            url: optional_env("QDRANT_URL", DEFAULT_QDRANT_URL),
            api_key: env::var("QDRANT_API_KEY").ok().filter(|v| !v.trim().is_empty()),
            collection_default: optional_env("QDRANT_COLLECTION_DEFAULT", "gmrag_chunks"),
        };

        // S3 / MinIO
        let s3 = S3Config {
            endpoint: require_env("S3_ENDPOINT")?,
            public_endpoint: optional_env("S3_PUBLIC_ENDPOINT", "http://localhost:9000"),
            region: optional_env("S3_REGION", DEFAULT_S3_REGION),
            access_key: require_env("S3_ACCESS_KEY")?,
            secret_key: require_env("S3_SECRET_KEY")?,
            bucket: require_env("S3_BUCKET")?,
            force_path_style: env::var("S3_FORCE_PATH_STYLE")
                .ok()
                .map(|v| v == "true" || v == "1")
                .unwrap_or(DEFAULT_S3_FORCE_PATH_STYLE),
        };

        // Redis
        let redis = RedisConfig {
            url: optional_env("REDIS_URL", DEFAULT_REDIS_URL),
        };

        // Ollama
        let ollama = OllamaConfig {
            host: optional_env("OLLAMA_HOST", DEFAULT_OLLAMA_HOST),
            embed_model: optional_env("OLLAMA_EMBED_MODEL", "nomic-embed-text"),
            llm_model: optional_env("OLLAMA_LLM_MODEL", "llama3.1:8b"),
            keep_alive: optional_env("OLLAMA_KEEP_ALIVE", "30m"),
        };

        // DeepSeek (optional remote LLM)
        let deepseek = DeepSeekConfig {
            api_key: env::var("DEEPSEEK_API_KEY").ok().filter(|v| !v.trim().is_empty()),
            base_url: optional_env("DEEPSEEK_BASE_URL", DEFAULT_DEEPSEEK_BASE_URL),
            model: optional_env("DEEPSEEK_MODEL", DEFAULT_DEEPSEEK_MODEL),
            timeout_s: env::var("DEEPSEEK_TIMEOUT_S")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(DEFAULT_DEEPSEEK_TIMEOUT_S),
        };

        Ok(Self {
            database_url,
            http_bind,
            log_filter,
            tenant_header,
            service_name,
            oidc,
            qdrant,
            s3,
            redis,
            ollama,
            deepseek,
        })
    }

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

fn optional_env(key: &str, default: &str) -> String {
    env::var(key).ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| default.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Helper: clear all env vars that Config reads, so tests start from a
    /// clean slate without `.env` file interference.
    fn clear_config_env() {
        let keys = [
            "DATABASE_URL",
            "GMRAG_HTTP_BIND",
            "GMRAG_RUST_LOG",
            "GMRAG_TENANT_HEADER",
            "GMRAG_SERVICE_NAME",
            "KEYCLOAK_ISSUER",
            "KEYCLOAK_CLIENT_ID",
            "KEYCLOAK_CLIENT_SECRET",
            "KEYCLOAK_FRONTEND_CLIENT_ID",
            "QDRANT_URL",
            "QDRANT_API_KEY",
            "QDRANT_COLLECTION_DEFAULT",
            "S3_ENDPOINT",
            "S3_PUBLIC_ENDPOINT",
            "S3_REGION",
            "S3_ACCESS_KEY",
            "S3_SECRET_KEY",
            "S3_BUCKET",
            "S3_FORCE_PATH_STYLE",
            "REDIS_URL",
            "OLLAMA_HOST",
            "OLLAMA_EMBED_MODEL",
            "OLLAMA_LLM_MODEL",
            "OLLAMA_KEEP_ALIVE",
            "DEEPSEEK_API_KEY",
            "DEEPSEEK_BASE_URL",
            "DEEPSEEK_MODEL",
            "DEEPSEEK_TIMEOUT_S",
        ];
        for k in keys {
            env::remove_var(k);
        }
    }

    /// Set the minimum required env vars for a successful Config parse.
    fn set_minimal_env() {
        env::set_var("DATABASE_URL", "postgres://u:p@h:5432/d");
        env::set_var("KEYCLOAK_ISSUER", "http://kc:8080/realms/test");
        env::set_var("KEYCLOAK_CLIENT_ID", "test-client");
        env::set_var("KEYCLOAK_CLIENT_SECRET", "test-secret");
        env::set_var("S3_ENDPOINT", "http://minio:9000");
        env::set_var("S3_ACCESS_KEY", "ak");
        env::set_var("S3_SECRET_KEY", "sk");
        env::set_var("S3_BUCKET", "bucket");
    }

    #[test]
    fn config_env_matrix() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_config_env();

        // Case 1: missing DATABASE_URL must fail.
        let res = Config::from_process_env();
        assert!(
            matches!(res, Err(Error::Config(ref msg)) if msg.contains("DATABASE_URL")),
            "missing DATABASE_URL must surface as Error::Config, got {res:?}"
        );

        // Case 2: all required env set → parse succeeds with defaults.
        set_minimal_env();
        let cfg = Config::from_process_env().expect("config should parse");
        assert_eq!(cfg.database_url, "postgres://u:p@h:5432/d");
        assert_eq!(cfg.http_bind.to_string(), DEFAULT_HTTP_BIND);
        assert_eq!(cfg.tenant_header, "X-Tenant-ID");
        assert_eq!(cfg.oidc.issuer, "http://kc:8080/realms/test");
        assert_eq!(cfg.qdrant.url, DEFAULT_QDRANT_URL);
        assert_eq!(cfg.s3.region, DEFAULT_S3_REGION);
        assert_eq!(cfg.redis.url, DEFAULT_REDIS_URL);
        assert_eq!(cfg.ollama.host, DEFAULT_OLLAMA_HOST);
        assert!(cfg.deepseek.api_key.is_none());

        // Case 3: empty DATABASE_URL also fails.
        env::set_var("DATABASE_URL", "   ");
        let res = Config::from_process_env();
        assert!(
            matches!(res, Err(Error::Config(_))),
            "empty DATABASE_URL must surface as Error::Config, got {res:?}"
        );

        clear_config_env();
    }

    #[test]
    fn config_oidc_fields_parsed() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_config_env();
        set_minimal_env();

        let cfg = Config::from_process_env().unwrap();
        assert_eq!(cfg.oidc.client_id, "test-client");
        assert_eq!(cfg.oidc.client_secret, "test-secret");
        assert_eq!(cfg.oidc.frontend_client_id, "gmrag-frontend");

        // Override frontend client id.
        env::set_var("KEYCLOAK_FRONTEND_CLIENT_ID", "custom-frontend");
        let cfg = Config::from_process_env().unwrap();
        assert_eq!(cfg.oidc.frontend_client_id, "custom-frontend");

        clear_config_env();
    }

    #[test]
    fn config_qdrant_optional_api_key() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_config_env();
        set_minimal_env();

        // No QDRANT_API_KEY → None
        let cfg = Config::from_process_env().unwrap();
        assert!(cfg.qdrant.api_key.is_none());

        // With QDRANT_API_KEY → Some
        env::set_var("QDRANT_API_KEY", "my-key");
        let cfg = Config::from_process_env().unwrap();
        assert_eq!(cfg.qdrant.api_key.as_deref(), Some("my-key"));

        // Empty QDRANT_API_KEY → None
        env::set_var("QDRANT_API_KEY", "  ");
        let cfg = Config::from_process_env().unwrap();
        assert!(cfg.qdrant.api_key.is_none());

        clear_config_env();
    }

    #[test]
    fn config_s3_force_path_style() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_config_env();
        set_minimal_env();

        // Default = true
        let cfg = Config::from_process_env().unwrap();
        assert!(cfg.s3.force_path_style);

        env::set_var("S3_FORCE_PATH_STYLE", "false");
        let cfg = Config::from_process_env().unwrap();
        assert!(!cfg.s3.force_path_style);

        clear_config_env();
    }

    #[test]
    fn config_deepseek_optional() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_config_env();
        set_minimal_env();

        // No DEEPSEEK_API_KEY → None
        let cfg = Config::from_process_env().unwrap();
        assert!(cfg.deepseek.api_key.is_none());
        assert_eq!(cfg.deepseek.model, DEFAULT_DEEPSEEK_MODEL);
        assert_eq!(cfg.deepseek.timeout_s, 60);

        env::set_var("DEEPSEEK_API_KEY", "sk-test");
        env::set_var("DEEPSEEK_MODEL", "deepseek-v3");
        env::set_var("DEEPSEEK_TIMEOUT_S", "120");
        let cfg = Config::from_process_env().unwrap();
        assert_eq!(cfg.deepseek.api_key.as_deref(), Some("sk-test"));
        assert_eq!(cfg.deepseek.model, "deepseek-v3");
        assert_eq!(cfg.deepseek.timeout_s, 120);

        clear_config_env();
    }

    #[test]
    fn config_missing_required_subsystem_field_fails() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_config_env();
        set_minimal_env();

        // Missing KEYCLOAK_ISSUER.
        env::remove_var("KEYCLOAK_ISSUER");
        let res = Config::from_process_env();
        assert!(
            matches!(res, Err(Error::Config(ref msg)) if msg.contains("KEYCLOAK_ISSUER")),
            "missing KEYCLOAK_ISSUER must fail, got {res:?}"
        );

        // Restore and test S3_ACCESS_KEY.
        env::set_var("KEYCLOAK_ISSUER", "http://kc:8080/realms/test");
        env::remove_var("S3_ACCESS_KEY");
        let res = Config::from_process_env();
        assert!(
            matches!(res, Err(Error::Config(ref msg)) if msg.contains("S3_ACCESS_KEY")),
            "missing S3_ACCESS_KEY must fail, got {res:?}"
        );

        clear_config_env();
    }
}
