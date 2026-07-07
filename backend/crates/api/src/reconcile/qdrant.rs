//! Phase 3 — Qdrant drift reconciler.
//!
//! Postgres is the source of truth. This compares Qdrant points against
//! Postgres `document_chunks` / `graph_nodes` provenance, reports drift, and
//! — only when `auto_fix` is explicitly enabled — deletes orphaned points.
//!
//! Categories:
//! - **Orphaned chunk points**: Qdrant chunk points whose `document_id`
//!   payload has no live `documents` row → candidates for deletion.
//! - **Orphaned graph points**: Qdrant graph points whose `node_id` payload
//!   has no live `graph_nodes` row → candidates for deletion.
//! - **Missing chunk points**: `documents` with `status='indexed'` that have
//!   fewer Qdrant points than Postgres `document_chunks` rows → candidates
//!   for re-ingestion. **Report only** — never auto-re-embedded in this phase
//!   (a heavier, riskier write path; out of scope per the phase rules).
//! - **Missing graph points**: live `graph_nodes` with no Qdrant point →
//!   report only.
//!
//! Malformed points (missing/non-UUID `document_id` / `node_id` payload) are
//! counted and reported but never auto-deleted — their target cannot be
//! verified, so a blind delete would be unsafe.
//!
//! Auto-fix default is OFF. When `auto_fix = false`, this function makes NO
//! `delete_*` call to Qdrant — verified by test.

use std::collections::{HashMap, HashSet};

use gmrag_core::{status::document as doc_status, ChunkPointRef, GraphNodePointRef, QdrantStore};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

/// Bounded sample size per drift category.
pub const SAMPLE_LIMIT: usize = 50;

#[derive(Debug, Clone, Serialize)]
pub struct CategoryReport {
    pub count: usize,
    pub sample: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct QdrantReport {
    pub orphaned_chunk_points: CategoryReport,
    pub orphaned_graph_points: CategoryReport,
    pub missing_chunk_points: CategoryReport,
    pub missing_graph_points: CategoryReport,
    pub malformed_chunk_points: usize,
    pub malformed_graph_points: usize,
    pub auto_fix_ran: bool,
    pub deleted_chunk_docs: usize,
    pub deleted_graph_nodes: usize,
}

fn empty_category() -> CategoryReport {
    CategoryReport {
        count: 0,
        sample: Vec::new(),
    }
}

fn push_sample(cat: &mut CategoryReport, key: String) {
    cat.count += 1;
    if cat.sample.len() < SAMPLE_LIMIT {
        cat.sample.push(key);
    }
}

/// Extract the tenant UUID from a `chunks_{uuid}` / `graph_{uuid}` name.
fn parse_tenant_from_collection(name: &str, prefix: &str) -> Option<Uuid> {
    let rest = name.strip_prefix(prefix)?;
    Uuid::parse_str(rest).ok()
}

/// Run one Qdrant reconciliation pass.
///
/// `auto_fix = false` (the default) → report-only: NO deletes to Qdrant, and
/// missing points are NEVER re-embedded in this phase regardless. `auto_fix =
/// true` → delete orphaned chunk/graph points; missing points still report
/// only.
pub async fn run_qdrant_reconcile(
    pool: &PgPool,
    qdrant: &QdrantStore,
    auto_fix: bool,
) -> anyhow::Result<QdrantReport> {
    // ── Live Postgres provenance (source of truth) ──────────────────────
    let live_docs: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM documents")
        .fetch_all(pool)
        .await?;
    let live_doc_set: HashSet<Uuid> = live_docs.iter().copied().collect();
    let live_tenants: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM tenants")
        .fetch_all(pool)
        .await?;
    let live_doc_set_tenant: HashSet<Uuid> = live_tenants.iter().copied().collect();
    let live_nodes: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM graph_nodes")
        .fetch_all(pool)
        .await?;
    let live_node_set: HashSet<Uuid> = live_nodes.iter().copied().collect();
    let chunk_counts: Vec<(Uuid, i64)> =
        sqlx::query_as("SELECT document_id, count(*) FROM document_chunks GROUP BY document_id")
            .fetch_all(pool)
            .await?;
    let pg_chunk_counts: HashMap<Uuid, i64> = chunk_counts.into_iter().collect();
    let indexed_docs: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM documents WHERE status = $1")
        .bind(doc_status::INDEXED)
        .fetch_all(pool)
        .await?;

    // ── Enumerate Qdrant collections + scroll points ────────────────────
    // Only scan collections whose tenant still exists in Postgres. A
    // collection whose tenant was deleted is a tenant-level orphan (tenant
    // deletion already best-effort tears down collections in tenants.rs);
    // scanning it here would surface stale points from other test DBs / old
    // tenants as per-point orphans, which is noise, not the per-document
    // drift this reconciler targets. Such collections are logged and skipped.
    let names = qdrant.list_collection_names().await?;
    let mut chunk_refs_by_tenant: HashMap<Uuid, Vec<ChunkPointRef>> = HashMap::new();
    let mut graph_refs_by_tenant: HashMap<Uuid, Vec<GraphNodePointRef>> = HashMap::new();
    for name in &names {
        let Some(tid) = parse_tenant_from_collection(name, "chunks_")
            .or_else(|| parse_tenant_from_collection(name, "graph_"))
        else {
            continue;
        };
        if !live_doc_set_tenant.contains(&tid) {
            tracing::warn!(collection = %name, "qdrant reconcile: skipping collection whose tenant is absent from Postgres (tenant-level orphan; not per-point drift)");
            continue;
        }
        if name.starts_with("chunks_") {
            chunk_refs_by_tenant.insert(tid, qdrant.scroll_chunk_refs(tid).await?);
        } else if name.starts_with("graph_") {
            graph_refs_by_tenant.insert(tid, qdrant.scroll_graph_node_refs(tid).await?);
        }
    }

    // ── Chunks: orphaned + missing ──────────────────────────────────────
    let mut qdrant_chunk_count: HashMap<Uuid, i64> = HashMap::new();
    let mut malformed_chunks = 0usize;
    let mut orphaned_chunk = empty_category();
    let mut orphaned_chunk_docs: Vec<(Uuid, Uuid)> = Vec::new();
    for (tid, refs) in &chunk_refs_by_tenant {
        for r in refs {
            match r.document_id {
                Some(doc_id) => {
                    *qdrant_chunk_count.entry(doc_id).or_insert(0) += 1;
                    if !live_doc_set.contains(&doc_id) {
                        push_sample(
                            &mut orphaned_chunk,
                            format!("chunks_{tid}: document_id={doc_id}"),
                        );
                        orphaned_chunk_docs.push((*tid, doc_id));
                    }
                }
                None => malformed_chunks += 1,
            }
        }
    }
    let mut missing_chunk = empty_category();
    for doc_id in &indexed_docs {
        let q = qdrant_chunk_count.get(doc_id).copied().unwrap_or(0);
        let p = pg_chunk_counts.get(doc_id).copied().unwrap_or(0);
        if q < p {
            push_sample(
                &mut missing_chunk,
                format!("document_id={doc_id} qdrant={q} postgres={p}"),
            );
        }
    }

    // ── Graph: orphaned + missing ───────────────────────────────────────
    let mut qdrant_node_count: HashMap<Uuid, i64> = HashMap::new();
    let mut malformed_graph = 0usize;
    let mut orphaned_graph = empty_category();
    let mut orphaned_graph_nodes: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for (tid, refs) in &graph_refs_by_tenant {
        for r in refs {
            match r.node_id {
                Some(node_id) => {
                    *qdrant_node_count.entry(node_id).or_insert(0) += 1;
                    if !live_node_set.contains(&node_id) {
                        push_sample(
                            &mut orphaned_graph,
                            format!("graph_{tid}: node_id={node_id}"),
                        );
                        orphaned_graph_nodes.entry(*tid).or_default().push(node_id);
                    }
                }
                None => malformed_graph += 1,
            }
        }
    }
    let mut missing_graph = empty_category();
    for node_id in &live_nodes {
        if qdrant_node_count.get(node_id).copied().unwrap_or(0) == 0 {
            push_sample(&mut missing_graph, format!("node_id={node_id}"));
        }
    }

    let mut report = QdrantReport {
        orphaned_chunk_points: orphaned_chunk,
        orphaned_graph_points: orphaned_graph,
        missing_chunk_points: missing_chunk,
        missing_graph_points: missing_graph,
        malformed_chunk_points: malformed_chunks,
        malformed_graph_points: malformed_graph,
        auto_fix_ran: false,
        deleted_chunk_docs: 0,
        deleted_graph_nodes: 0,
    };

    tracing::info!(
        orphaned_chunks = report.orphaned_chunk_points.count,
        orphaned_graph = report.orphaned_graph_points.count,
        missing_chunks = report.missing_chunk_points.count,
        missing_graph = report.missing_graph_points.count,
        malformed_chunks = report.malformed_chunk_points,
        malformed_graph = report.malformed_graph_points,
        auto_fix,
        "qdrant reconcile: drift report"
    );

    // ── Repair — ONLY when auto_fix is true ─────────────────────────────
    // Missing points are NEVER auto-fixed in this phase (report only).
    if !auto_fix {
        return Ok(report);
    }
    report.auto_fix_ran = true;

    for (tid, doc_id) in &orphaned_chunk_docs {
        tracing::info!(
            tenant_id = %tid, document_id = %doc_id, before = "present", after = "absent",
            "qdrant reconcile: DELETE orphaned chunk points"
        );
        qdrant.delete_chunks_by_document(*tid, *doc_id).await?;
    }
    report.deleted_chunk_docs = orphaned_chunk_docs.len();

    for (tid, node_ids) in &orphaned_graph_nodes {
        tracing::info!(
            tenant_id = %tid, count = node_ids.len(), before = "present", after = "absent",
            "qdrant reconcile: DELETE orphaned graph points"
        );
        qdrant.delete_graph_nodes(*tid, node_ids).await?;
    }
    report.deleted_graph_nodes = orphaned_graph_nodes.values().map(|v| v.len()).sum();

    tracing::info!(
        deleted_chunk_docs = report.deleted_chunk_docs,
        deleted_graph_nodes = report.deleted_graph_nodes,
        "qdrant reconcile: auto-fix complete (missing points NOT re-embedded — report only)"
    );
    Ok(report)
}
