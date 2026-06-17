//! gmrag-core — shared building blocks for the backend crates.
//!
//! Scope (T5-T7): configuration loader, error envelope, and Postgres pool helper.
//! Anything more (telemetry, tenancy middleware, etc.) belongs to later tasks.

pub mod config;
pub mod db;
pub mod error;
pub mod qdrant;

pub use config::Config;
pub use db::{DbPool, init_app_pool, init_pool};
pub use error::Error;
pub use qdrant::QdrantStore;
