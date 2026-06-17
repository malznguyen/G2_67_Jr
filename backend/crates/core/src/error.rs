//! Application-wide error type for gmrag-core.
//!
//! This is intentionally a thin envelope: it exists so crates that depend on
//! `gmrag-core` (api, worker, future jobs) can share a single error vocabulary
//! for the cross-cutting categories below. Domain-specific errors will be
//! added in later tasks (auth, tenancy, ingest, ...).

use std::io;

/// Top-level error type used across gmrag-core.
///
/// Variants are deliberately coarse for T5-T7. As the codebase grows, each
/// subsystem is expected to add its own error type that can convert `From`
/// into this envelope at API boundaries.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("configuration error: {0}")]
    Config(String),

    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("database migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    #[error("io error: {0}")]
    Io(#[from] io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// Qdrant client error.
    ///
    /// `qdrant_client::QdrantError` is ~176 bytes (wraps a `tonic::Status`
    /// plus other variants), which trips `clippy::result_large_err` when
    /// inlined into `Error`. Boxing keeps the `Error` enum small on the
    /// happy path. A manual `From` is provided so callers can still use
    /// `.map_err(Error::from)` on `Result<_, QdrantError>`.
    #[error("qdrant error: {0}")]
    Qdrant(Box<qdrant_client::QdrantError>),
}

impl From<qdrant_client::QdrantError> for Error {
    fn from(e: qdrant_client::QdrantError) -> Self {
        Error::Qdrant(Box::new(e))
    }
}

impl Error {
    /// Returns a stable, machine-readable error code (kebab-case).
    pub fn code(&self) -> &'static str {
        match self {
            Error::Config(_) => "config-error",
            Error::Database(_) => "database-error",
            Error::Migrate(_) => "database-migrate-error",
            Error::Io(_) => "io-error",
            Error::Json(_) => "json-error",
            Error::Qdrant(_) => "qdrant-error",
        }
    }
}

/// Convenience alias for `Result<T, Error>` in gmrag-core consumers.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_is_stable_per_variant() {
        let cfg = Error::Config("x".into());
        let db = Error::Database(sqlx::Error::PoolClosed);
        assert_eq!(cfg.code(), "config-error");
        assert_eq!(db.code(), "database-error");
    }

    #[test]
    fn display_includes_context() {
        let e = Error::Config("missing DATABASE_URL".into());
        let s = e.to_string();
        assert!(s.contains("configuration error"));
        assert!(s.contains("missing DATABASE_URL"));
    }
}
