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

use base64::Engine as _;

use crate::error::Error;

const DEFAULT_HTTP_BIND: &str = "0.0.0.0:8080";
const DEFAULT_LOG_FILTER: &str = "info,gmrag_core=debug,gmrag_api=debug";
const DEFAULT_TENANT_HEADER: &str = "X-Tenant-ID";
/// Qdrant gRPC port (6334). The rust `qdrant-client` uses gRPC, not the
/// REST port 6333. Using 6333 causes `FRAME_SIZE_ERROR` at connection time.
const DEFAULT_QDRANT_URL: &str = "http://localhost:6334";
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
    /// Container-internal issuer used to fetch the OIDC discovery document /
    /// JWKS (e.g. `http://keycloak:8080/realms/gmrag`). The backend cannot
    /// resolve the host's `localhost`.
    pub issuer: String,
    /// Issuer used to verify the `iss` claim in tokens. Keycloak emits `iss`
    /// using the host-side origin (e.g. `http://localhost:8080/realms/gmrag`),
    /// so this must match what the IdP puts in the token — not the internal
    /// discovery URL. Defaults to `issuer` when `KEYCLOAK_ISSUER_VERIFY` is
    /// unset (single-host deployments).
    pub issuer_verify: String,
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
    pub tenant_key_encryption_key: Option<[u8; 32]>,

    /// T84D Phase 1.3: feature-flag for the OCR fallback path. Off by
    /// default so the Docker image never bakes native libpdfium in.
    pub ocr_enabled: bool,

    /// T84D Phase 1.1: how often the worker relay polls `ingest_outbox`
    /// (seconds).
    pub outbox_poll_interval_secs: u64,
    /// T84D Phase 1.2: how often the worker sweeps stuck ingestion jobs
    /// (seconds).
    pub sweep_interval_secs: u64,

    /// T84D Phase 4.1: explicit, testable cap read for `init_pool`.
    pub database_max_connections: u32,

    /// T84D Phase 3.3: number of past chat messages threaded into the LLM
    /// context per turn.
    pub chat_history_limit: usize,
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
        let oidc_issuer = require_env("KEYCLOAK_ISSUER")?;
        // `iss` claim verification value — defaults to the public issuer env
        // (host-side origin) when present, otherwise to the internal issuer.
        let issuer_verify = optional_env("KEYCLOAK_ISSUER_VERIFY", &oidc_issuer);
        let oidc = OidcConfig {
            issuer: oidc_issuer,
            issuer_verify,
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

        let tenant_key_encryption_key = optional_base64_32_env("GMRAG_TENANT_KEY_ENCRYPTION_KEY")?;

        // T84D Phase 1.3 / 1.1 / 1.2 / 3.3 / 4.1: numeric + flag env vars.
        let ocr_enabled = parse_bool_env("GMRAG_OCR_ENABLED", false);
        let outbox_poll_interval_secs =
            parse_usize_env("GMRAG_OUTBOX_POLL_INTERVAL_SECS", 3).max(1) as u64;
        let sweep_interval_secs =
            parse_usize_env("GMRAG_SWEEP_INTERVAL_SECS", 60).max(1) as u64;
        let database_max_connections = parse_usize_env("DATABASE_MAX_CONNECTIONS", 10) as u32;
        let chat_history_limit = parse_usize_env("GMRAG_CHAT_HISTORY_LIMIT", 10);

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
            tenant_key_encryption_key,
            ocr_enabled,
            outbox_poll_interval_secs,
            sweep_interval_secs,
            database_max_connections,
            chat_history_limit,
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

/// Parse a boolean env var: "true"/"1" (case-insensitive) → true, else default.
fn parse_bool_env(key: &str, default: bool) -> bool {
    match env::var(key).ok().filter(|v| !v.trim().is_empty()) {
        Some(v) => matches!(v.trim().to_ascii_lowercase().as_str(), "true" | "1"),
        None => default,
    }
}

/// Parse an unsigned env var with a fallback default. Empty/invalid → default.
fn parse_usize_env(key: &str, default: usize) -> usize {
    env::var(key)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

fn optional_base64_32_env(key: &'static str) -> Result<Option<[u8; 32]>, Error> {
    let Some(raw) = env::var(key).ok().filter(|v| !v.trim().is_empty()) else {
        return Ok(None);
    };

    let decoded = base64::engine::general_purpose::STANDARD
        .decode(raw.trim())
        .map_err(|e| Error::Config(format!("invalid {key}: expected base64: {e}")))?;
    let len = decoded.len();
    let bytes: [u8; 32] = decoded.try_into().map_err(|_| {
        Error::Config(format!(
            "invalid {key}: decoded key must be 32 bytes, got {len}"
        ))
    })?;
    Ok(Some(bytes))
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
            "GMRAG_TENANT_KEY_ENCRYPTION_KEY",
            "GMRAG_OCR_ENABLED",
            "GMRAG_OUTBOX_POLL_INTERVAL_SECS",
            "GMRAG_SWEEP_INTERVAL_SECS",
            "DATABASE_MAX_CONNECTIONS",
            "GMRAG_CHAT_HISTORY_LIMIT",
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
        assert!(
            DEFAULT_QDRANT_URL.ends_with(":6334"),
            "Qdrant default URL must use gRPC port 6334, got {DEFAULT_QDRANT_URL}"
        );
        assert_eq!(cfg.s3.region, DEFAULT_S3_REGION);
        assert_eq!(cfg.redis.url, DEFAULT_REDIS_URL);
        assert_eq!(cfg.ollama.host, DEFAULT_OLLAMA_HOST);
        assert!(cfg.deepseek.api_key.is_none());
        assert!(cfg.tenant_key_encryption_key.is_none());

        // T84D defaults: OCR off, sane relay/sweep intervals, pool cap 10,
        // chat history 10.
        assert!(!cfg.ocr_enabled);
        assert_eq!(cfg.outbox_poll_interval_secs, 3);
        assert_eq!(cfg.sweep_interval_secs, 60);
        assert_eq!(cfg.database_max_connections, 10);
        assert_eq!(cfg.chat_history_limit, 10);

        // T84D overrides are honoured.
        env::set_var("GMRAG_OCR_ENABLED", "true");
        env::set_var("GMRAG_OUTBOX_POLL_INTERVAL_SECS", "7");
        env::set_var("GMRAG_SWEEP_INTERVAL_SECS", "120");
        env::set_var("DATABASE_MAX_CONNECTIONS", "25");
        env::set_var("GMRAG_CHAT_HISTORY_LIMIT", "20");
        let cfg = Config::from_process_env().expect("config should parse");
        assert!(cfg.ocr_enabled);
        assert_eq!(cfg.outbox_poll_interval_secs, 7);
        assert_eq!(cfg.sweep_interval_secs, 120);
        assert_eq!(cfg.database_max_connections, 25);
        assert_eq!(cfg.chat_history_limit, 20);

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
    fn config_tenant_key_encryption_key_is_base64_32_bytes() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_config_env();
        set_minimal_env();

        env::set_var(
            "GMRAG_TENANT_KEY_ENCRYPTION_KEY",
            base64::engine::general_purpose::STANDARD.encode([7_u8; 32]),
        );
        let cfg = Config::from_process_env().unwrap();
        assert_eq!(cfg.tenant_key_encryption_key, Some([7_u8; 32]));

        env::set_var("GMRAG_TENANT_KEY_ENCRYPTION_KEY", "not base64");
        let res = Config::from_process_env();
        assert!(
            matches!(res, Err(Error::Config(ref msg)) if msg.contains("GMRAG_TENANT_KEY_ENCRYPTION_KEY")),
            "invalid base64 must surface as Error::Config, got {res:?}"
        );

        env::set_var(
            "GMRAG_TENANT_KEY_ENCRYPTION_KEY",
            base64::engine::general_purpose::STANDARD.encode([1_u8; 31]),
        );
        let res = Config::from_process_env();
        assert!(
            matches!(res, Err(Error::Config(ref msg)) if msg.contains("32 bytes")),
            "wrong key length must surface as Error::Config, got {res:?}"
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
