//! Canonical status / role string constants (Phase 0, TASK-P0-02).
//!
//! Centralizes the lowercase status strings stored in the database so the
//! worker and API share one source of truth instead of repeating raw string
//! literals at every transition site. These mirror the CHECK constraints
//! added in migration `20260624000000_canonical_roles_and_statuses.sql`:
//!
//! - `documents.status` ∈ {`uploaded`, `processing`, `indexed`, `failed`}
//! - `ingest_jobs.status` ∈ {`pending`, `processing`, `completed`, `failed`}
//! - `ingest_outbox.status` ∈ {`pending`, `dispatched`}
//!
//! State machines preserved by Phase 0:
//!
//! ```text
//! documents:    uploaded -> processing -> indexed
//!               uploaded/processing -> failed
//! ingest_jobs:  pending -> processing -> completed
//!               pending/processing -> failed
//! ingest_outbox: pending -> dispatched
//! ```
//!
//! This is intentionally a flat `pub const` module — not a domain framework —
//! to keep the change surface small.

/// `documents.status` vocabulary.
pub mod document {
    pub const UPLOADED: &str = "uploaded";
    pub const PROCESSING: &str = "processing";
    pub const INDEXED: &str = "indexed";
    pub const FAILED: &str = "failed";
}

/// `ingest_jobs.status` vocabulary.
pub mod ingest_job {
    pub const PENDING: &str = "pending";
    pub const PROCESSING: &str = "processing";
    pub const COMPLETED: &str = "completed";
    pub const FAILED: &str = "failed";
}

/// `ingest_outbox.status` vocabulary.
pub mod ingest_outbox {
    pub const PENDING: &str = "pending";
    pub const DISPATCHED: &str = "dispatched";
}
