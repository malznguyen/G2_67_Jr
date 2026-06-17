//! Qdrant vector DB wrapper module.
//!
//! T27 introduces the connection wrapper. Collection schema helpers live
//! alongside it in `store.rs` (added in T28 / T29).

mod store;

pub use store::QdrantStore;
