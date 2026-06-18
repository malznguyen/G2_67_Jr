//! Idempotent dual-write — persist ingestion artifacts to PostgreSQL
//! (metadata + relationships) and Qdrant (vectors) inside one Postgres
//! transaction.
//!
//! T42: `dual_write_ingestion` writes `document_chunks`, `graph_nodes`,
//! `graph_edges` with `ON CONFLICT DO UPDATE` (idempotent on retry) and
//! upserts the matching vectors into `chunks_{tenant_id}` and
//! `graph_{tenant_id}`. If the Qdrant upsert fails, the Postgres
//! transaction is rolled back so a retry starts clean.

use std::collections::HashMap;

use gmrag_core::QdrantStore;
use qdrant_client::qdrant::PointStruct;
use qdrant_client::Payload;
use sqlx::PgPool;
use thiserror::Error;
use uuid::Uuid;

use crate::graph::GraphExtraction;

/// Inputs needed to persist one ingestion pass.
pub struct DualWriteInput<'a> {
    pub tenant_id: Uuid,
    pub workspace_id: Uuid,
    pub document_id: Uuid,
    pub owner_id: Uuid,
    pub visibility: &'a str,
    pub filename: &'a str,
    pub chunks: &'a [String],
    pub chunk_vectors: Vec<Vec<f32>>,
    pub extraction: &'a GraphExtraction,
    pub node_vectors: Vec<Vec<f32>>,
}

/// Result of a successful dual-write — the IDs written to Postgres (and
/// mirrored as Qdrant point IDs), useful for status tracking / tests.
#[derive(Debug, Clone)]
pub struct DualWriteResult {
    pub chunk_ids: Vec<Uuid>,
    pub node_ids: Vec<Uuid>,
    pub edges_written: usize,
}

/// Errors emitted by dual-write.
#[derive(Debug, Error)]
pub enum IngestError {
    #[error("database error: {0}")]
    Db(String),
    #[error("qdrant error: {0}")]
    Qdrant(String),
    #[error("input mismatch: {0}")]
    Input(String),
}

impl From<sqlx::Error> for IngestError {
    fn from(e: sqlx::Error) -> Self {
        IngestError::Db(e.to_string())
    }
}

impl From<gmrag_core::Error> for IngestError {
    fn from(e: gmrag_core::Error) -> Self {
        IngestError::Qdrant(e.to_string())
    }
}

/// Persist chunks + graph (nodes/edges) to Postgres and Qdrant inside one
/// Postgres transaction.
///
/// Steps:
/// 1. `BEGIN` + `SET LOCAL app.tenant_id = {tenant_id}` (RLS).
/// 2. Upsert each `document_chunks` row (`ON CONFLICT (document_id,
///    chunk_index) DO UPDATE`), returning its `id` — that UUID is reused as
///    the Qdrant point id so retries overwrite the same point.
/// 3. Upsert each `graph_nodes` row (`ON CONFLICT (tenant_id, workspace_id,
///    label, kind) DO UPDATE`), returning its `id`.
/// 4. Build Qdrant `PointStruct`s for chunks + nodes and call
///    [`QdrantStore::upsert_chunks`] / [`QdrantStore::upsert_graph_nodes`].
///    On Qdrant failure the Postgres tx is rolled back.
/// 5. Insert `graph_edges` (`ON CONFLICT (src_node_id, dst_node_id, kind)
///    DO UPDATE`), mapping `source`/`target` labels → node ids. Edges whose
///    endpoints are missing from the extraction are skipped.
/// 6. `COMMIT`.
pub async fn dual_write_ingestion(
    pool: &PgPool,
    qdrant: &QdrantStore,
    input: DualWriteInput<'_>,
) -> Result<DualWriteResult, IngestError> {
    if input.chunks.len() != input.chunk_vectors.len() {
        return Err(IngestError::Input(format!(
            "chunks ({}) vs chunk_vectors ({}) length mismatch",
            input.chunks.len(),
            input.chunk_vectors.len()
        )));
    }
    if input.extraction.nodes.len() != input.node_vectors.len() {
        return Err(IngestError::Input(format!(
            "nodes ({}) vs node_vectors ({}) length mismatch",
            input.extraction.nodes.len(),
            input.node_vectors.len()
        )));
    }

    let mut tx = pool.begin().await?;
    // RLS: downgrade to app role + scope this tx to the tenant.
    sqlx::Executor::execute(&mut *tx, "SET LOCAL ROLE gmrag_app")
        .await
        .map_err(|e| IngestError::Db(e.to_string()))?;
    sqlx::query(&format!("SET LOCAL app.tenant_id = '{}'", input.tenant_id))
        .execute(&mut *tx)
        .await
        .map_err(|e| IngestError::Db(e.to_string()))?;

    // 1. Upsert document_chunks (idempotent on (document_id, chunk_index)).
    let mut chunk_ids = Vec::with_capacity(input.chunks.len());
    let mut chunk_points = Vec::with_capacity(input.chunks.len());
    for (idx, (text, vector)) in input
        .chunks
        .iter()
        .zip(input.chunk_vectors.iter())
        .enumerate()
    {
        let row_id: (Uuid,) = sqlx::query_as(
            r#"
            INSERT INTO document_chunks (tenant_id, document_id, chunk_index, content, qdrant_point_id)
            VALUES ($1, $2, $3, $4, gen_random_uuid())
            ON CONFLICT (document_id, chunk_index) DO UPDATE
                SET content = EXCLUDED.content
            RETURNING qdrant_point_id
            "#,
        )
        .bind(input.tenant_id)
        .bind(input.document_id)
        .bind(idx as i32)
        .bind(text)
        .fetch_one(&mut *tx)
        .await?;
        let point_id = row_id.0;
        chunk_ids.push(point_id);

        let payload = Payload::try_from(serde_json::json!({
            "workspace_id": input.workspace_id.to_string(),
            "document_id": input.document_id.to_string(),
            "chunk_index": idx as i64,
            "filename": input.filename,
            "owner_id": input.owner_id.to_string(),
            "visibility": input.visibility,
        }))
        .map_err(|e| IngestError::Qdrant(e.to_string()))?;
        chunk_points.push(PointStruct::new(point_id.to_string(), vector.clone(), payload));
    }

    // 2. Upsert graph_nodes (idempotent on (tenant_id, workspace_id, label, kind)).
    let mut node_ids = Vec::with_capacity(input.extraction.nodes.len());
    let mut label_to_id: HashMap<String, Uuid> = HashMap::new();
    let mut node_points = Vec::with_capacity(input.extraction.nodes.len());
    for (node, vector) in input
        .extraction
        .nodes
        .iter()
        .zip(input.node_vectors.iter())
    {
        let row_id: (Uuid,) = sqlx::query_as(
            r#"
            INSERT INTO graph_nodes (tenant_id, workspace_id, kind, label, properties)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (tenant_id, workspace_id, label, kind) DO UPDATE
                SET properties = EXCLUDED.properties
            RETURNING id
            "#,
        )
        .bind(input.tenant_id)
        .bind(input.workspace_id)
        .bind(&node.kind)
        .bind(&node.label)
        .bind(serde_json::json!({"description": node.description}))
        .fetch_one(&mut *tx)
        .await?;
        let node_id = row_id.0;
        node_ids.push(node_id);
        label_to_id
            .entry(node.label.clone())
            .or_insert(node_id);

        let payload = Payload::try_from(serde_json::json!({
            "node_id": node_id.to_string(),
            "workspace_id": input.workspace_id.to_string(),
            "entity_name": node.label,
        }))
        .map_err(|e| IngestError::Qdrant(e.to_string()))?;
        node_points.push(PointStruct::new(node_id.to_string(), vector.clone(), payload));
    }

    // 3. Sync Qdrant BEFORE committing Postgres — on failure, rollback.
    //    Ensure collections exist (idempotent) then upsert.
    qdrant.setup_tenant_collections(input.tenant_id).await?;
    if !chunk_points.is_empty() {
        qdrant.upsert_chunks(input.tenant_id, chunk_points).await?;
    }
    if !node_points.is_empty() {
        qdrant
            .upsert_graph_nodes(input.tenant_id, node_points)
            .await?;
    }

    // 4. Insert graph_edges (idempotent on (src_node_id, dst_node_id, kind)).
    let mut edges_written = 0usize;
    for edge in &input.extraction.edges {
        let Some(src_id) = label_to_id.get(&edge.source) else {
            continue;
        };
        let Some(dst_id) = label_to_id.get(&edge.target) else {
            continue;
        };
        sqlx::query(
            r#"
            INSERT INTO graph_edges (tenant_id, src_node_id, dst_node_id, kind, weight, properties)
            VALUES ($1, $2, $3, $4, 1.0, '{}'::jsonb)
            ON CONFLICT (src_node_id, dst_node_id, kind) DO UPDATE
                SET weight = EXCLUDED.weight
            "#,
        )
        .bind(input.tenant_id)
        .bind(src_id)
        .bind(dst_id)
        .bind(&edge.kind)
        .execute(&mut *tx)
        .await?;
        edges_written += 1;
    }

    tx.commit().await?;

    Ok(DualWriteResult {
        chunk_ids,
        node_ids,
        edges_written,
    })
}
