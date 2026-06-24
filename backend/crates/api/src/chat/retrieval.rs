//! Query-time retrieval: chunk kNN (ACL-filtered), graph nodes + ILIKE fallback, edges.

use std::collections::HashSet;

use gmrag_core::config::{DeepSeekConfig, OllamaConfig};
use gmrag_core::QdrantStore;
use qdrant_client::qdrant::{Condition, Filter, MinShould, ScoredPoint};
use sqlx::PgConnection;
use thiserror::Error;
use uuid::Uuid;

use crate::llm::byok::{resolve_llm_config, ByokError};
use crate::llm::provider::{DeepSeekProvider, LlmError, LlmProvider};

pub const DEFAULT_TOP_K: u64 = 5;
const GRAPH_SCORE_THRESHOLD: f32 = 0.25;

#[derive(Debug, Clone, PartialEq)]
pub struct ChunkHit {
    pub citation_index: u32,
    pub point_id: Uuid,
    pub document_id: Uuid,
    pub chunk_index: i32,
    pub content: String,
    pub filename: Option<String>,
    pub score: f32,
    /// T84D Phase 3.1: page range from `document_chunks.page_start` /
    /// `page_end` (1-based). `None` when the chunker had no page info
    /// (legacy rows or non-PDF ingest).
    pub page_start: Option<i32>,
    pub page_end: Option<i32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GraphNodeHit {
    pub node_id: Uuid,
    pub label: String,
    pub kind: String,
    pub description: String,
    pub score: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphEdgeHit {
    pub src_node_id: Uuid,
    pub dst_node_id: Uuid,
    pub src_label: String,
    pub dst_label: String,
    pub kind: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GraphContext {
    pub nodes: Vec<GraphNodeHit>,
    pub edges: Vec<GraphEdgeHit>,
}

#[derive(Debug, Clone)]
pub struct RetrievalParams {
    pub tenant_id: Uuid,
    pub workspace_id: Uuid,
    pub user_id: Uuid,
    pub query: String,
    pub top_k: u64,
}

impl RetrievalParams {
    pub fn new(
        tenant_id: Uuid,
        workspace_id: Uuid,
        user_id: Uuid,
        query: impl Into<String>,
    ) -> Self {
        Self {
            tenant_id,
            workspace_id,
            user_id,
            query: query.into(),
            top_k: DEFAULT_TOP_K,
        }
    }
}

#[derive(Debug, Error)]
pub enum RetrievalError {
    #[error("user is not a member of this workspace")]
    NotWorkspaceMember,
    #[error("llm error: {0}")]
    Llm(#[from] LlmError),
    #[error("byok error: {0}")]
    Byok(#[from] ByokError),
    #[error("database error: {0}")]
    Db(String),
    #[error("qdrant error: {0}")]
    Qdrant(String),
    #[error("invalid qdrant point id")]
    InvalidPointId,
    #[error("metering error: {0}")]
    Metering(String),
}

impl From<sqlx::Error> for RetrievalError {
    fn from(e: sqlx::Error) -> Self {
        RetrievalError::Db(e.to_string())
    }
}

impl From<gmrag_core::Error> for RetrievalError {
    fn from(e: gmrag_core::Error) -> Self {
        RetrievalError::Qdrant(e.to_string())
    }
}

/// Document ids the user may read within a workspace (ReBAC viewer relation).
///
/// This is the set-compiled form of [`crate::rbac::check::check_relation`]`
/// `(document, viewer, user)` so the filter stays a single indexed query
/// instead of an N+1 per-document Check. A document is accessible iff the
/// caller holds the `viewer` relation on it (T83/C1/C5):
/// - `visibility = 'shared'` (publicly readable in the tenant), or
/// - the caller is the owner (`owner_id`), or
/// - the caller is a member of the document's workspace (inheritance), or
/// - the caller is the recipient of a `resource_acl` grant — directly or via
///   a workspace-group share — with `permission IN ('owner','editor','viewer')`.
pub async fn accessible_document_ids(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<Vec<Uuid>, RetrievalError> {
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT DISTINCT d.id
        FROM documents d
        WHERE d.workspace_id = $1
          AND (
            d.visibility = 'shared'
            OR d.owner_id = $2
            OR EXISTS (
              SELECT 1 FROM workspace_members wm
              WHERE wm.workspace_id = d.workspace_id
                AND wm.user_id = $2
            )
            OR EXISTS (
              SELECT 1 FROM resource_acl ra
              WHERE ra.resource_type = 'document'
                AND ra.resource_id = d.id
                AND ra.permission IN ('owner', 'editor', 'viewer')
                AND (
                  (ra.principal_type = 'user' AND ra.principal_id = $2)
                  OR (ra.principal_type = 'workspace' AND EXISTS (
                        SELECT 1 FROM workspace_members wmg
                        WHERE wmg.workspace_id = ra.principal_id
                          AND wmg.user_id = $2))
                )
            )
          )
        "#,
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_all(conn)
    .await?;

    Ok(rows.into_iter().map(|r| r.0).collect())
}

async fn ensure_workspace_member(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<(), RetrievalError> {
    let member: Option<(bool,)> = sqlx::query_as(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM workspace_members
            WHERE workspace_id = $1 AND user_id = $2
        )
        "#,
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_optional(conn)
    .await?;

    match member.map(|r| r.0) {
        Some(true) => Ok(()),
        _ => Err(RetrievalError::NotWorkspaceMember),
    }
}

fn build_chunk_filter(workspace_id: Uuid, document_ids: &[Uuid]) -> Filter {
    let doc_conditions: Vec<Condition> = document_ids
        .iter()
        .map(|id| Condition::matches("document_id", id.to_string()))
        .collect();

    Filter {
        must: vec![Condition::matches("workspace_id", workspace_id.to_string())],
        should: doc_conditions,
        min_should: Some(MinShould {
            min_count: 1,
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn scored_point_uuid(point: &ScoredPoint) -> Result<Uuid, RetrievalError> {
    use qdrant_client::qdrant::point_id::PointIdOptions;

    let Some(id) = &point.id else {
        return Err(RetrievalError::InvalidPointId);
    };
    match &id.point_id_options {
        Some(PointIdOptions::Uuid(s)) => {
            Uuid::parse_str(s).map_err(|_| RetrievalError::InvalidPointId)
        }
        _ => Err(RetrievalError::InvalidPointId),
    }
}

fn payload_str(point: &ScoredPoint, key: &str) -> Option<String> {
    point
        .payload
        .get(key)
        .and_then(|v| v.as_str().map(|s| s.to_string()))
}

/// kNN chunk search using a pre-computed query vector (T46 core).
pub async fn retrieve_chunks_with_vector(
    conn: &mut PgConnection,
    qdrant: &QdrantStore,
    query_vector: &[f32],
    params: &RetrievalParams,
) -> Result<Vec<ChunkHit>, RetrievalError> {
    ensure_workspace_member(conn, params.workspace_id, params.user_id).await?;

    let accessible = accessible_document_ids(conn, params.workspace_id, params.user_id).await?;
    if accessible.is_empty() {
        return Ok(Vec::new());
    }
    let allowed: HashSet<Uuid> = accessible.iter().copied().collect();
    let filter = build_chunk_filter(params.workspace_id, &accessible);
    let scored = qdrant
        .search_chunks(
            params.tenant_id,
            query_vector.to_vec(),
            Some(filter),
            params.top_k,
        )
        .await?;

    let mut hits = Vec::new();
    for point in scored {
        let point_id = scored_point_uuid(&point)?;
        let document_id = payload_str(&point, "document_id")
            .and_then(|s| Uuid::parse_str(&s).ok())
            .ok_or(RetrievalError::InvalidPointId)?;

        if !allowed.contains(&document_id) {
            continue;
        }

        let row: Option<(String, i32, Option<String>, Option<i32>, Option<i32>)> = sqlx::query_as(
            r#"
            SELECT dc.content, dc.chunk_index, d.title, dc.page_start, dc.page_end
            FROM document_chunks dc
            JOIN documents d ON d.id = dc.document_id
            WHERE dc.qdrant_point_id = $1
            "#,
        )
        .bind(point_id)
        .fetch_optional(&mut *conn)
        .await?;

        let Some((content, chunk_index, title, page_start, page_end)) = row else {
            continue;
        };

        hits.push(ChunkHit {
            citation_index: 0,
            point_id,
            document_id,
            chunk_index,
            content,
            filename: title,
            score: point.score,
            page_start,
            page_end,
        });
    }

    for (idx, hit) in hits.iter_mut().enumerate() {
        hit.citation_index = (idx + 1) as u32;
    }

    Ok(hits)
}

/// Embed the query then search accessible chunks (T46).
pub async fn retrieve_chunks(
    conn: &mut PgConnection,
    qdrant: &QdrantStore,
    llm: &dyn LlmProvider,
    params: &RetrievalParams,
) -> Result<Vec<ChunkHit>, RetrievalError> {
    let query_vector = llm.embed_query(&params.query).await?;
    retrieve_chunks_with_vector(conn, qdrant, &query_vector, params).await
}

async fn hydrate_graph_node(
    conn: &mut PgConnection,
    node_id: Uuid,
    score: Option<f32>,
) -> Result<Option<GraphNodeHit>, RetrievalError> {
    let row: Option<(String, String, serde_json::Value)> = sqlx::query_as(
        r#"
        SELECT kind, label, properties
        FROM graph_nodes
        WHERE id = $1
        "#,
    )
    .bind(node_id)
    .fetch_optional(conn)
    .await?;

    Ok(row.map(|(kind, label, properties)| GraphNodeHit {
        node_id,
        kind,
        label,
        description: properties
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        score,
    }))
}

fn ilike_pattern(query: &str) -> String {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return "%".into();
    }
    format!("%{trimmed}%")
}

async fn ilike_graph_fallback(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    query_text: &str,
    limit: i64,
    existing: &HashSet<Uuid>,
) -> Result<Vec<GraphNodeHit>, RetrievalError> {
    let pattern = ilike_pattern(query_text);
    let rows: Vec<(Uuid, String, String, serde_json::Value)> = sqlx::query_as(
        r#"
        SELECT id, kind, label, properties
        FROM graph_nodes
        WHERE workspace_id = $1
          AND (
            label ILIKE $2
            OR properties->>'description' ILIKE $2
          )
        LIMIT $3
        "#,
    )
    .bind(workspace_id)
    .bind(&pattern)
    .bind(limit)
    .fetch_all(conn)
    .await?;

    Ok(rows
        .into_iter()
        .filter(|(id, _, _, _)| !existing.contains(id))
        .map(|(node_id, kind, label, properties)| GraphNodeHit {
            node_id,
            kind,
            label,
            description: properties
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            score: None,
        })
        .collect())
}

async fn load_graph_edges(
    conn: &mut PgConnection,
    tenant_id: Uuid,
    node_ids: &[Uuid],
) -> Result<Vec<GraphEdgeHit>, RetrievalError> {
    if node_ids.is_empty() {
        return Ok(Vec::new());
    }

    let rows: Vec<(Uuid, Uuid, String, String, String)> = sqlx::query_as(
        r#"
        SELECT e.src_node_id, e.dst_node_id, e.kind, sn.label, dn.label
        FROM graph_edges e
        JOIN graph_nodes sn ON sn.id = e.src_node_id
        JOIN graph_nodes dn ON dn.id = e.dst_node_id
        WHERE e.tenant_id = $1
          AND (e.src_node_id = ANY($2) OR e.dst_node_id = ANY($2))
        "#,
    )
    .bind(tenant_id)
    .bind(node_ids)
    .fetch_all(conn)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(src_node_id, dst_node_id, kind, src_label, dst_label)| GraphEdgeHit {
            src_node_id,
            dst_node_id,
            src_label,
            dst_label,
            kind,
        })
        .collect())
}

/// Graph kNN + ILIKE fallback + edge expansion (T47, T84D ACL provenance).
///
/// T84D Phase 2.1 (SEC-1): graph nodes are deduped and shared across
/// documents, so the kNN results may include a node extracted only from
/// a private document the caller cannot read. Post-filter:
/// a node is kept iff `EXISTS (graph_node_documents gnd WHERE
/// gnd.node_id = node AND gnd.document_id = ANY($accessible))`.
/// The ILIKE fallback path applies the same filter. Edges whose src or
/// dst was filtered out are dropped.
pub async fn retrieve_graph_context(
    conn: &mut PgConnection,
    qdrant: &QdrantStore,
    query_vector: &[f32],
    tenant_id: Uuid,
    workspace_id: Uuid,
    query_text: &str,
    top_k: u64,
    accessible_document_ids: &[Uuid],
) -> Result<GraphContext, RetrievalError> {
    let scored = qdrant
        .search_graph_nodes(tenant_id, workspace_id, query_vector.to_vec(), top_k)
        .await?;

    let mut nodes = Vec::new();
    let mut seen = HashSet::new();
    let mut needs_fallback = scored.is_empty();

    for point in scored {
        if point.score < GRAPH_SCORE_THRESHOLD {
            needs_fallback = true;
            continue;
        }
        let node_id = payload_str(&point, "node_id")
            .and_then(|s| Uuid::parse_str(&s).ok())
            .unwrap_or_else(|| scored_point_uuid(&point).unwrap_or(Uuid::nil()));

        if node_id.is_nil() || !seen.insert(node_id) {
            continue;
        }

        if let Some(hit) = hydrate_graph_node(conn, node_id, Some(point.score)).await? {
            if node_visible_via_provenance(conn, node_id, accessible_document_ids).await? {
                nodes.push(hit);
            }
        }
    }

    if needs_fallback && nodes.len() < top_k as usize {
        let fallback = ilike_graph_fallback(
            conn,
            workspace_id,
            query_text,
            top_k as i64,
            &seen,
        )
        .await?;
        for hit in fallback {
            if node_visible_via_provenance(conn, hit.node_id, accessible_document_ids).await? {
                seen.insert(hit.node_id);
                nodes.push(hit);
                if nodes.len() >= top_k as usize {
                    break;
                }
            } else {
                // Record the skip so the next ILIKE iteration doesn't
                // re-hydrate the same filtered node.
                seen.insert(hit.node_id);
            }
        }
    }

    let node_ids: Vec<Uuid> = nodes.iter().map(|n| n.node_id).collect();
    let mut edges = load_graph_edges(conn, tenant_id, &node_ids).await?;

    // Drop edges whose endpoints were filtered out by the ACL check above.
    let allowed: HashSet<Uuid> = node_ids.iter().copied().collect();
    edges.retain(|e| allowed.contains(&e.src_node_id) && allowed.contains(&e.dst_node_id));

    Ok(GraphContext { nodes, edges })
}

/// T84D Phase 2.1: ACL provenance check — a node is visible to the caller
/// iff ANY of its source documents is in the caller's
/// `accessible_document_ids` set.
pub(crate) async fn node_visible_via_provenance(
    conn: &mut PgConnection,
    node_id: Uuid,
    accessible: &[Uuid],
) -> Result<bool, RetrievalError> {
    if accessible.is_empty() {
        return Ok(false);
    }
    let visible: Option<(bool,)> = sqlx::query_as(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM graph_node_documents gnd
            WHERE gnd.node_id = $1
              AND gnd.document_id = ANY($2)
        )
        "#,
    )
    .bind(node_id)
    .bind(accessible)
    .fetch_optional(conn)
    .await?;
    Ok(visible.map(|r| r.0).unwrap_or(false))
}

/// Single embed + chunk/graph retrieval (orchestrator for T49).
pub async fn retrieve_all_with_provider(
    conn: &mut PgConnection,
    qdrant: &QdrantStore,
    llm: &dyn LlmProvider,
    params: &RetrievalParams,
) -> Result<(Vec<ChunkHit>, GraphContext), RetrievalError> {
    let query_vector = llm.embed_query(&params.query).await?;
    let chunks =
        retrieve_chunks_with_vector(conn, qdrant, &query_vector, params).await?;
    // T84D Phase 2.1: compute the accessible set once before the graph
    // call (the chunk path already produces it inside
    // `retrieve_chunks_with_vector`, but that one is private to its
    // scope — compute it again here so the graph ACL matches the chunk
    // ACL exactly).
    let accessible = accessible_document_ids(conn, params.workspace_id, params.user_id).await?;
    let graph = retrieve_graph_context(
        conn,
        qdrant,
        &query_vector,
        params.tenant_id,
        params.workspace_id,
        &params.query,
        params.top_k,
        &accessible,
    )
    .await?;
    Ok((chunks, graph))
}

/// Like [`retrieve_all_with_provider`] but records `embedding_tokens` after embed.
pub async fn retrieve_all_with_metering(
    conn: &mut PgConnection,
    qdrant: &QdrantStore,
    llm: &dyn LlmProvider,
    params: &RetrievalParams,
) -> Result<(Vec<ChunkHit>, GraphContext), RetrievalError> {
    let query_vector = llm.embed_query(&params.query).await?;
    crate::metering::record_embedding_usage(
        conn,
        params.tenant_id,
        &params.query,
        llm.provider(),
    )
    .await
    .map_err(|e| RetrievalError::Metering(e.to_string()))?;

    let chunks =
        retrieve_chunks_with_vector(conn, qdrant, &query_vector, params).await?;
    let accessible = accessible_document_ids(conn, params.workspace_id, params.user_id).await?;
    let graph = retrieve_graph_context(
        conn,
        qdrant,
        &query_vector,
        params.tenant_id,
        params.workspace_id,
        &params.query,
        params.top_k,
        &accessible,
    )
    .await?;
    Ok((chunks, graph))
}

/// Resolve tenant LLM config, embed once, then retrieve chunks + graph.
pub async fn retrieve_all(
    conn: &mut PgConnection,
    qdrant: &QdrantStore,
    deepseek: &DeepSeekConfig,
    ollama: &OllamaConfig,
    tenant_key: Option<&[u8; 32]>,
    params: &RetrievalParams,
) -> Result<(Vec<ChunkHit>, GraphContext), RetrievalError> {
    let resolved = resolve_llm_config(
        conn,
        params.tenant_id,
        deepseek,
        ollama,
        tenant_key,
    )
    .await?;
    let provider = DeepSeekProvider::new(resolved.provider);
    retrieve_all_with_provider(conn, qdrant, &provider, params).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use gmrag_core::config::QdrantConfig;
    use qdrant_client::qdrant::PointStruct;
    use qdrant_client::Payload;
    use sqlx::PgPool;

    use crate::llm::provider::{
        ChatMessage, ChatStream, ChatStreamFuture, GraphExtraction, LlmProvider,
        ProviderFuture,
    };

    struct MockEmbedProvider {
        calls: AtomicUsize,
        vector: Vec<f32>,
    }

    impl MockEmbedProvider {
        fn new(vector: Vec<f32>) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                vector,
            }
        }
    }

    impl LlmProvider for MockEmbedProvider {
        fn embed_query<'a>(&'a self, _query: &'a str) -> ProviderFuture<'a, Vec<f32>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let vector = self.vector.clone();
            Box::pin(async move { Ok(vector) })
        }

        fn chat_stream<'a>(&'a self, _messages: &'a [ChatMessage]) -> ChatStreamFuture<'a> {
            Box::pin(async {
                let stream: ChatStream = Box::pin(futures::stream::empty());
                Ok(stream)
            })
        }

        fn graph_extract<'a>(&'a self, _text: &'a str) -> ProviderFuture<'a, GraphExtraction> {
            Box::pin(async { Ok(GraphExtraction::default()) })
        }

        fn provider(&self) -> &str {
            "mock"
        }

        fn chat_model(&self) -> &str {
            "mock"
        }
    }

    fn unit_vec(pos: usize) -> Vec<f32> {
        let mut v = vec![0.0f32; 768];
        v[pos] = 1.0;
        v
    }

    fn local_qdrant() -> QdrantConfig {
        QdrantConfig {
            url: "http://localhost:6334".into(),
            api_key: None,
            collection_default: "gmrag_chunks".into(),
        }
    }

    async fn rls_conn(pool: &PgPool, tenant_id: Uuid) -> sqlx::pool::PoolConnection<sqlx::Postgres> {
        let mut conn = pool.acquire().await.unwrap();
        sqlx::Executor::execute(&mut *conn, "BEGIN").await.unwrap();
        sqlx::Executor::execute(&mut *conn, "SET LOCAL ROLE gmrag_app")
            .await
            .unwrap();
        sqlx::query(&format!("SET LOCAL app.tenant_id = '{tenant_id}'"))
            .execute(&mut *conn)
            .await
            .unwrap();
        conn
    }

    async fn seed_user(pool: &PgPool) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, email, name) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(format!("u{id}@retrieval.test"))
            .bind("Retrieval User")
            .execute(pool)
            .await
            .unwrap();
        id
    }

    async fn seed_tenant(pool: &PgPool, name: &str) -> Uuid {
        sqlx::query_scalar("INSERT INTO tenants (name) VALUES ($1) RETURNING id")
            .bind(name)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn seed_workspace(pool: &PgPool, tenant: Uuid, owner: Uuid) -> Uuid {
        let ws = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO workspaces (id, tenant_id, name, slug, created_by) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(ws)
        .bind(tenant)
        .bind("Retrieval WS")
        .bind(format!("ws-{ws}"))
        .bind(owner)
        .execute(pool)
        .await
        .unwrap();
        ws
    }

    async fn add_workspace_member(pool: &PgPool, ws: Uuid, tenant: Uuid, user: Uuid) {
        sqlx::query(
            "INSERT INTO workspace_members (workspace_id, tenant_id, user_id) VALUES ($1, $2, $3)",
        )
        .bind(ws)
        .bind(tenant)
        .bind(user)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn insert_doc(
        conn: &mut sqlx::pool::PoolConnection<sqlx::Postgres>,
        id: Uuid,
        tenant: Uuid,
        ws: Uuid,
        owner: Uuid,
        title: &str,
        visibility: &str,
    ) {
        sqlx::query(
            r#"INSERT INTO documents (id, tenant_id, workspace_id, owner_id, title, status, visibility, s3_key)
               VALUES ($1, $2, $3, $4, $5, 'ready', $6, 'k')"#,
        )
        .bind(id)
        .bind(tenant)
        .bind(ws)
        .bind(owner)
        .bind(title)
        .bind(visibility)
        .execute(&mut **conn)
        .await
        .unwrap();
    }

    async fn insert_doc_grant(
        pool: &PgPool,
        tenant: Uuid,
        doc_id: Uuid,
        principal_type: &str,
        principal_id: Uuid,
        permission: &str,
    ) {
        sqlx::query(
            "INSERT INTO resource_acl (tenant_id, resource_type, resource_id, principal_type, principal_id, permission)
             VALUES ($1, 'document', $2, $3, $4, $5)",
        )
        .bind(tenant)
        .bind(doc_id)
        .bind(principal_type)
        .bind(principal_id)
        .bind(permission)
        .execute(pool)
        .await
        .unwrap();
    }

    /// C1 regression: a non-member (no workspace membership, no grant) can see
    /// only their own documents and `visibility='shared'` documents. Foreign
    /// private documents are excluded. This replaces the old false-positive
    /// test that used `visibility='public'` (a value T58 can never produce).
    #[sqlx::test(migrations = "../../migrations")]
    async fn accessible_docs_non_member_sees_owned_and_shared_only(pool: PgPool) {
        let tenant = seed_tenant(&pool, "acl-non-member").await;
        let caller = seed_user(&pool).await;
        let other = seed_user(&pool).await;
        let ws = seed_workspace(&pool, tenant, other).await;
        add_workspace_member(&pool, ws, tenant, other).await;

        let mut conn = rls_conn(&pool, tenant).await;

        let owned_doc = Uuid::new_v4();
        insert_doc(&mut conn, owned_doc, tenant, ws, caller, "mine", "private").await;

        let shared_doc = Uuid::new_v4();
        insert_doc(&mut conn, shared_doc, tenant, ws, other, "theirs-shared", "shared").await;

        let foreign_private = Uuid::new_v4();
        insert_doc(&mut conn, foreign_private, tenant, ws, other, "theirs-private", "private")
            .await;

        let ids = accessible_document_ids(&mut conn, ws, caller).await.unwrap();
        assert!(ids.contains(&owned_doc), "caller sees own private doc");
        assert!(
            ids.contains(&shared_doc),
            "caller sees visibility='shared' doc"
        );
        assert!(
            !ids.contains(&foreign_private),
            "non-member without grant cannot see foreign private doc"
        );
    }

    /// C1/C5 regression: a workspace member sees ALL documents in the workspace
    /// (ReBAC `tuple_to_userset(workspace → member)` inheritance), including
    /// foreign private documents.
    #[sqlx::test(migrations = "../../migrations")]
    async fn accessible_docs_workspace_member_sees_all(pool: PgPool) {
        let tenant = seed_tenant(&pool, "acl-ws-member").await;
        let caller = seed_user(&pool).await;
        let other = seed_user(&pool).await;
        let ws = seed_workspace(&pool, tenant, other).await;
        add_workspace_member(&pool, ws, tenant, other).await;
        add_workspace_member(&pool, ws, tenant, caller).await;

        let mut conn = rls_conn(&pool, tenant).await;

        let owned_doc = Uuid::new_v4();
        insert_doc(&mut conn, owned_doc, tenant, ws, caller, "mine", "private").await;

        let foreign_private = Uuid::new_v4();
        insert_doc(&mut conn, foreign_private, tenant, ws, other, "theirs-private", "private")
            .await;

        let ids = accessible_document_ids(&mut conn, ws, caller).await.unwrap();
        assert!(ids.contains(&owned_doc), "member sees own doc");
        assert!(
            ids.contains(&foreign_private),
            "workspace member sees foreign private doc via inheritance"
        );
    }

    /// C1/C5 regression: a non-member with a `resource_acl` grant
    /// `permission='viewer'` can see a private document (production-realistic
    /// ReBAC data per T64 CHECK + T67 grant flow).
    #[sqlx::test(migrations = "../../migrations")]
    async fn accessible_docs_grant_sees_private_doc(pool: PgPool) {
        let tenant = seed_tenant(&pool, "acl-grant").await;
        let caller = seed_user(&pool).await;
        let other = seed_user(&pool).await;
        let ws = seed_workspace(&pool, tenant, other).await;
        add_workspace_member(&pool, ws, tenant, other).await;

        let mut conn = rls_conn(&pool, tenant).await;

        let granted_doc = Uuid::new_v4();
        insert_doc(&mut conn, granted_doc, tenant, ws, other, "granted", "private").await;

        let ungranted_doc = Uuid::new_v4();
        insert_doc(&mut conn, ungranted_doc, tenant, ws, other, "ungranted", "private").await;

        insert_doc_grant(&pool, tenant, granted_doc, "user", caller, "viewer").await;

        let ids = accessible_document_ids(&mut conn, ws, caller).await.unwrap();
        assert!(
            ids.contains(&granted_doc),
            "non-member with viewer grant sees private doc"
        );
        assert!(
            !ids.contains(&ungranted_doc),
            "non-member without grant cannot see private doc"
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn retrieve_chunks_empty_when_no_chunks_indexed(pool: PgPool) {
        let tenant = seed_tenant(&pool, "empty-acl").await;
        let owner = seed_user(&pool).await;
        let ws = seed_workspace(&pool, tenant, owner).await;
        add_workspace_member(&pool, ws, tenant, owner).await;

        let mut conn = rls_conn(&pool, tenant).await;

        let doc = Uuid::new_v4();
        insert_doc(&mut conn, doc, tenant, ws, owner, "no-chunks", "private").await;

        let qdrant = match QdrantStore::new(&local_qdrant()).await {
            Ok(s) => s,
            Err(_) => return,
        };
        qdrant.setup_tenant_collections(tenant).await.unwrap();

        let params = RetrievalParams::new(tenant, ws, owner, "test query");
        let hits = retrieve_chunks_with_vector(&mut conn, &qdrant, &unit_vec(0), &params)
            .await
            .unwrap();
        assert!(hits.is_empty(), "no Qdrant points → no hits");

        let _ = qdrant.teardown_tenant_collections(tenant).await;
    }

    /// C1 regression: retrieval is scoped to the caller's workspace. Documents
    /// in another workspace are excluded by both the SQL `workspace_id` filter
    /// and the Qdrant `must` payload filter, even when the caller owns them.
    #[sqlx::test(migrations = "../../migrations")]
    async fn retrieve_chunks_scoped_to_workspace(pool: PgPool) {
        let qdrant = QdrantStore::new(&local_qdrant())
            .await
            .expect("Qdrant at localhost:6334 required");

        let tenant = seed_tenant(&pool, "chunk-scoping").await;
        let owner = seed_user(&pool).await;
        let other = seed_user(&pool).await;
        let ws = seed_workspace(&pool, tenant, owner).await;
        add_workspace_member(&pool, ws, tenant, owner).await;
        let other_ws = seed_workspace(&pool, tenant, other).await;
        add_workspace_member(&pool, other_ws, tenant, other).await;

        let mut conn = rls_conn(&pool, tenant).await;

        let allowed_doc = Uuid::new_v4();
        insert_doc(&mut conn, allowed_doc, tenant, ws, owner, "allowed.pdf", "private").await;

        let blocked_doc = Uuid::new_v4();
        insert_doc(&mut conn, blocked_doc, tenant, other_ws, other, "blocked.pdf", "private")
            .await;

        let allowed_point = Uuid::new_v4();
        sqlx::query(
            r#"INSERT INTO document_chunks (tenant_id, document_id, chunk_index, content, qdrant_point_id)
               VALUES ($1, $2, 0, 'allowed chunk text', $3)"#,
        )
        .bind(tenant)
        .bind(allowed_doc)
        .bind(allowed_point)
        .execute(&mut *conn)
        .await
        .unwrap();

        let blocked_point = Uuid::new_v4();
        sqlx::query(
            r#"INSERT INTO document_chunks (tenant_id, document_id, chunk_index, content, qdrant_point_id)
               VALUES ($1, $2, 0, 'blocked chunk text', $3)"#,
        )
        .bind(tenant)
        .bind(blocked_doc)
        .bind(blocked_point)
        .execute(&mut *conn)
        .await
        .unwrap();

        qdrant.setup_tenant_collections(tenant).await.unwrap();

        let make_point = |point_id: Uuid, ws_id: Uuid, doc_id: Uuid, text: &str, vec: Vec<f32>| -> PointStruct {
            PointStruct::new(
                point_id.to_string(),
                vec,
                Payload::try_from(serde_json::json!({
                    "workspace_id": ws_id.to_string(),
                    "document_id": doc_id.to_string(),
                    "chunk_index": 0_i64,
                    "filename": text,
                    "owner_id": owner.to_string(),
                    "visibility": "private",
                }))
                .unwrap(),
            )
        };

        qdrant
            .upsert_chunks(
                tenant,
                vec![
                    make_point(allowed_point, ws, allowed_doc, "allowed.pdf", unit_vec(0)),
                    make_point(blocked_point, other_ws, blocked_doc, "blocked.pdf", unit_vec(0)),
                ],
            )
            .await
            .unwrap();

        let params = RetrievalParams::new(tenant, ws, owner, "query");
        let hits = retrieve_chunks_with_vector(&mut conn, &qdrant, &unit_vec(0), &params)
            .await
            .unwrap();

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].content, "allowed chunk text");
        assert_eq!(hits[0].citation_index, 1);
        assert_eq!(hits[0].point_id, allowed_point);

        let _ = qdrant.teardown_tenant_collections(tenant).await;
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn graph_ilike_fallback_finds_label_match(pool: PgPool) {
        let tenant = seed_tenant(&pool, "graph-ilike").await;
        let owner = seed_user(&pool).await;
        let ws = seed_workspace(&pool, tenant, owner).await;

        let mut conn = rls_conn(&pool, tenant).await;

        // T84D Phase 2.1: the node is extracted only from a document the
        // caller owns, so it must be returned. Without the provenance
        // link the new ACL post-filter would (correctly) drop the node.
        let doc = Uuid::new_v4();
        insert_doc(&mut conn, doc, tenant, ws, owner, "keycloak-pdf", "private").await;

        let node_id = Uuid::new_v4();
        sqlx::query(
            r#"INSERT INTO graph_nodes (id, tenant_id, workspace_id, kind, label, properties)
               VALUES ($1, $2, $3, 'Service', 'Keycloak', '{"description":"OIDC provider"}'::jsonb)"#,
        )
        .bind(node_id)
        .bind(tenant)
        .bind(ws)
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            r#"INSERT INTO graph_node_documents (node_id, document_id, tenant_id)
               VALUES ($1, $2, $3)"#,
        )
        .bind(node_id)
        .bind(doc)
        .bind(tenant)
        .execute(&mut *conn)
        .await
        .unwrap();

        let qdrant = QdrantStore::new(&local_qdrant())
            .await
            .expect("Qdrant required");
        qdrant.setup_tenant_collections(tenant).await.unwrap();

        let zero = vec![0.0f32; 768];
        let accessible = accessible_document_ids(&mut conn, ws, owner).await.unwrap();
        let ctx = retrieve_graph_context(
            &mut conn,
            &qdrant,
            &zero,
            tenant,
            ws,
            "keycloak",
            5,
            &accessible,
        )
        .await
        .unwrap();

        assert_eq!(ctx.nodes.len(), 1);
        assert_eq!(ctx.nodes[0].label, "Keycloak");
        assert!(ctx.nodes[0].score.is_none());

        let _ = qdrant.teardown_tenant_collections(tenant).await;
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn graph_edges_connect_retrieved_nodes(pool: PgPool) {
        let tenant = seed_tenant(&pool, "graph-edges").await;
        let owner = seed_user(&pool).await;
        let ws = seed_workspace(&pool, tenant, owner).await;

        let mut conn = rls_conn(&pool, tenant).await;

        let n1 = Uuid::new_v4();
        let n2 = Uuid::new_v4();
        sqlx::query(
            r#"INSERT INTO graph_nodes (id, tenant_id, workspace_id, kind, label, properties)
               VALUES ($1, $2, $3, 'Person', 'Alice', '{}'::jsonb),
                  ($4, $2, $3, 'Org', 'Acme', '{}'::jsonb)"#,
        )
        .bind(n1)
        .bind(tenant)
        .bind(ws)
        .bind(n2)
        .execute(&mut *conn)
        .await
        .unwrap();

        sqlx::query(
            r#"INSERT INTO graph_edges (tenant_id, src_node_id, dst_node_id, kind)
               VALUES ($1, $2, $3, 'works_at')"#,
        )
        .bind(tenant)
        .bind(n1)
        .bind(n2)
        .execute(&mut *conn)
        .await
        .unwrap();

        let edges = load_graph_edges(&mut conn, tenant, &[n1, n2]).await.unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].kind, "works_at");
        assert_eq!(edges[0].src_label, "Alice");
        assert_eq!(edges[0].dst_label, "Acme");
    }

    #[tokio::test]
    async fn retrieve_all_embeds_once() {
        let provider = MockEmbedProvider::new(unit_vec(0));
        let vector = provider.embed_query("hello").await.unwrap();
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        assert_eq!(vector.len(), 768);
    }

    /// T84D Phase 2.1 regression (SEC-1): a graph node extracted only from
    /// a private document the caller cannot read MUST NOT leak through the
    /// ILIKE fallback. The plan calls out two cases:
    ///   - a non-member: filtered out (no accessible document).
    ///   - a non-member holding a `viewer` grant on the private document:
    ///     returned (the document is now accessible).
    ///
    /// Pure SQL + ILIKE fallback — no Qdrant needed (the kNN result set is
    /// empty, so the fallback path runs).
    #[sqlx::test(migrations = "../../migrations")]
    async fn graph_acl_provenance_filters_private_node_for_non_member(pool: PgPool) {
        let tenant = seed_tenant(&pool, "graph-acl").await;
        let owner = seed_user(&pool).await;
        let stranger = seed_user(&pool).await;
        let viewer = seed_user(&pool).await;
        let ws = seed_workspace(&pool, tenant, owner).await;
        add_workspace_member(&pool, ws, tenant, owner).await;

        let mut conn = rls_conn(&pool, tenant).await;

        let private_doc = Uuid::new_v4();
        insert_doc(&mut conn, private_doc, tenant, ws, owner, "secret.pdf", "private").await;

        let node_id = Uuid::new_v4();
        sqlx::query(
            r#"INSERT INTO graph_nodes (id, tenant_id, workspace_id, kind, label, properties)
               VALUES ($1, $2, $3, 'Project', 'Secret', '{"description":"top secret"}'::jsonb)"#,
        )
        .bind(node_id)
        .bind(tenant)
        .bind(ws)
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            r#"INSERT INTO graph_node_documents (node_id, document_id, tenant_id)
               VALUES ($1, $2, $3)"#,
        )
        .bind(node_id)
        .bind(private_doc)
        .bind(tenant)
        .execute(&mut *conn)
        .await
        .unwrap();

        // Stranger: no workspace membership + no grant → empty accessible set
        // → node filtered out via the ILIKE fallback.
        let zero = vec![0.0f32; 768];
        let qdrant = match QdrantStore::new(&local_qdrant()).await {
            Ok(s) => s,
            Err(_) => return,
        };
        qdrant.setup_tenant_collections(tenant).await.unwrap();

        let accessible_stranger = accessible_document_ids(&mut conn, ws, stranger).await.unwrap();
        assert!(
            accessible_stranger.is_empty(),
            "stranger sees no documents in this workspace"
        );
        let ctx = retrieve_graph_context(
            &mut conn,
            &qdrant,
            &zero,
            tenant,
            ws,
            "secret",
            5,
            &accessible_stranger,
        )
        .await
        .unwrap();
        assert!(
            ctx.nodes.iter().all(|n| n.node_id != node_id),
            "non-member must NOT see the private-only graph node"
        );

        // Viewer grant on the private document → node returned.
        insert_doc_grant(&pool, tenant, private_doc, "user", viewer, "viewer").await;
        let accessible_viewer = accessible_document_ids(&mut conn, ws, viewer).await.unwrap();
        assert!(
            accessible_viewer.contains(&private_doc),
            "viewer grant makes the private doc accessible"
        );
        let ctx = retrieve_graph_context(
            &mut conn,
            &qdrant,
            &zero,
            tenant,
            ws,
            "secret",
            5,
            &accessible_viewer,
        )
        .await
        .unwrap();
        assert!(
            ctx.nodes.iter().any(|n| n.node_id == node_id),
            "viewer grant holder MUST see the private-only graph node"
        );

        let _ = qdrant.teardown_tenant_collections(tenant).await;
    }
}
