//! ReBAC (relationship-based access control) — Zanzibar-style authorization.
//!
//! Implements the MVP subset of Google Zanzibar (paper `docs/5068.pdf`)
//! natively over the `resource_acl` relation-tuple table, evaluated inside the
//! request's RLS transaction so authorization shares the tenant-isolation
//! context with every other query (no external policy engine, no data sync).
//!
//! - [`model`] — relation vocabulary + namespace config + userset-rewrite
//!   rules (the pure policy layer).
//! - [`check`] — the `Check` API: recursive, bounded evaluation of a
//!   `(object, relation, principal)` query against PostgreSQL (T66).

pub mod check;
pub mod model;

pub use check::{check_relation, CheckError};

pub use model::{ObjectRef, ParentEdge, Principal, Relation, RewriteOp};
pub use model::{NS_CHAT_SESSION, NS_DOCUMENT, NS_WORKSPACE};
