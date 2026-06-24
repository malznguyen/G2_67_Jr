//! T42 — integration tests for idempotent dual-write.
//!
//! Requires a running PostgreSQL (migrations via `#[sqlx::test]`) AND a
//! running Qdrant gRPC at localhost:6334. Verifies:
//! - chunks + graph nodes/edges land in Postgres and Qdrant;
//! - calling `dual_write_ingestion` twice with the same input does NOT
//!   duplicate Postgres rows (idempotency via `ON CONFLICT DO UPDATE`);
//! - a Qdrant failure rolls back the Postgres transaction.

use gmrag_core::config::QdrantConfig;
use gmrag_core::QdrantStore;
use gmrag_worker::{Chunk, DualWriteInput, dual_write_ingestion};
use gmrag_worker::{ExtractedEdge, ExtractedNode, GraphExtraction};
use sqlx::PgPool;
use uuid::Uuid;

fn local_qdrant_config() -> QdrantConfig {
    QdrantConfig {
        url: "http://localhost:6334".into(),
        api_key: None,
        collection_default: "gmrag_chunks".into(),
    }
}

async fn qdrant() -> QdrantStore {
    QdrantStore::new(&local_qdrant_config())
        .await
        .expect("Qdrant gRPC at localhost:6334 must be reachable")
}

async fn create_tenant_workspace(pool: &PgPool) -> (Uuid, Uuid, Uuid) {
    let tenant = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, $2)")
        .bind(tenant)
        .bind("T42 Tenant")
        .execute(pool)
        .await
        .unwrap();
    let owner = create_user(pool).await;
    let ws = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workspaces (id, tenant_id, name, slug, created_by) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(ws)
    .bind(tenant)
    .bind("T42 WS")
    .bind(format!("t42-ws-{ws}"))
    .bind(owner)
    .execute(pool)
    .await
    .unwrap();
    (tenant, ws, owner)
}

async fn create_document(pool: &PgPool, tenant: Uuid, ws: Uuid, owner: Uuid) -> Uuid {
    let doc = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO documents (id, tenant_id, workspace_id, owner_id, title, status, visibility, s3_key)
           VALUES ($1, $2, $3, $4, 'T42 doc', 'uploaded', 'private', 'k')"#,
    )
    .bind(doc)
    .bind(tenant)
    .bind(ws)
    .bind(owner)
    .execute(pool)
    .await
    .unwrap();
    doc
}

async fn create_user(pool: &PgPool) -> Uuid {
    let user = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, email, name) VALUES ($1, $2, $3)")
        .bind(user)
        .bind(format!("u{user}@t42.test"))
        .bind("T42 Owner")
        .execute(pool)
        .await
        .unwrap();
    user
}

fn sample_extraction() -> GraphExtraction {
    GraphExtraction {
        nodes: vec![
            ExtractedNode {
                kind: "Person".into(),
                label: "Alice".into(),
                description: "engineer".into(),
            },
            ExtractedNode {
                kind: "Company".into(),
                label: "Acme".into(),
                description: "tech co".into(),
            },
        ],
        edges: vec![ExtractedEdge {
            source: "Alice".into(),
            target: "Acme".into(),
            kind: "works_at".into(),
        }],
    }
}

fn vec768(seed: f32) -> Vec<f32> {
    vec![seed; 768]
}

#[sqlx::test(migrations = "../../migrations")]
async fn dual_write_inserts_chunks_nodes_edges_and_qdrant_points(pool: PgPool) {
    let (tenant, ws, owner) = create_tenant_workspace(&pool).await;
    let doc = create_document(&pool, tenant, ws, owner).await;
    let store = qdrant().await;
    store.setup_tenant_collections(tenant).await.unwrap();

    let extraction = sample_extraction();
    let input = DualWriteInput {
        tenant_id: tenant,
        workspace_id: ws,
        document_id: doc,
        owner_id: owner,
        visibility: "private",
        filename: "t42.pdf",
        chunks: &[Chunk { text: "chunk zero".to_string(), page_start: 1, page_end: 1 }, Chunk { text: "chunk one".to_string(), page_start: 1, page_end: 1 }],
        chunk_vectors: vec![vec768(0.1), vec768(0.2)],
        extraction: &extraction,
        node_vectors: vec![vec768(0.3), vec768(0.4)],
    };

    let res = dual_write_ingestion(&pool, &store, input)
        .await
        .expect("dual-write must succeed");

    assert_eq!(res.chunk_ids.len(), 2);
    assert_eq!(res.node_ids.len(), 2);
    assert_eq!(res.edges_written, 1);

    // Postgres: 2 chunk rows, 2 node rows, 1 edge row.
    let chunk_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM document_chunks WHERE document_id = $1")
            .bind(doc)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(chunk_count, 2);

    let node_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM graph_nodes WHERE tenant_id = $1")
            .bind(tenant)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(node_count, 2);

    let edge_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM graph_edges WHERE tenant_id = $1")
            .bind(tenant)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(edge_count, 1);

    // Cleanup Qdrant collections.
    let _ = store.teardown_tenant_collections(tenant).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn dual_write_preserves_idempotency_on_retry(pool: PgPool) {
    let (tenant, ws, owner) = create_tenant_workspace(&pool).await;
    let doc = create_document(&pool, tenant, ws, owner).await;
    let store = qdrant().await;
    store.setup_tenant_collections(tenant).await.unwrap();

    let extraction = sample_extraction();
    let input = DualWriteInput {
        tenant_id: tenant,
        workspace_id: ws,
        document_id: doc,
        owner_id: owner,
        visibility: "private",
        filename: "t42.pdf",
        chunks: &[Chunk { text: "chunk zero".to_string(), page_start: 1, page_end: 1 }, Chunk { text: "chunk one".to_string(), page_start: 1, page_end: 1 }],
        chunk_vectors: vec![vec768(0.1), vec768(0.2)],
        extraction: &extraction,
        node_vectors: vec![vec768(0.3), vec768(0.4)],
    };

    // First write.
    let res1 = dual_write_ingestion(&pool, &store, input)
        .await
        .expect("first dual-write");
    // Second write with identical input (simulate retry).
    let input2 = DualWriteInput {
        tenant_id: tenant,
        workspace_id: ws,
        document_id: doc,
        owner_id: owner,
        visibility: "private",
        filename: "t42.pdf",
        chunks: &[Chunk { text: "chunk zero".to_string(), page_start: 1, page_end: 1 }, Chunk { text: "chunk one".to_string(), page_start: 1, page_end: 1 }],
        chunk_vectors: vec![vec768(0.1), vec768(0.2)],
        extraction: &extraction,
        node_vectors: vec![vec768(0.3), vec768(0.4)],
    };
    let res2 = dual_write_ingestion(&pool, &store, input2)
        .await
        .expect("second dual-write (idempotent)");

    // IDs must be stable across retries.
    assert_eq!(res1.chunk_ids, res2.chunk_ids, "chunk ids must be stable");
    assert_eq!(res1.node_ids, res2.node_ids, "node ids must be stable");

    // No duplicate rows.
    let chunk_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM document_chunks WHERE document_id = $1")
            .bind(doc)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(chunk_count, 2, "retry must not duplicate chunks");

    let node_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM graph_nodes WHERE tenant_id = $1")
            .bind(tenant)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(node_count, 2, "retry must not duplicate graph nodes");

    let edge_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM graph_edges WHERE tenant_id = $1")
            .bind(tenant)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(edge_count, 1, "retry must not duplicate edges");

    let _ = store.teardown_tenant_collections(tenant).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn dual_write_rolls_back_postgres_when_qdrant_fails(pool: PgPool) {
    let (tenant, ws, owner) = create_tenant_workspace(&pool).await;
    let doc = create_document(&pool, tenant, ws, owner).await;
    let store = qdrant().await;
    store.setup_tenant_collections(tenant).await.unwrap();

    let extraction = sample_extraction();
    // Wrong-dimension vectors (4 instead of 768) → Qdrant upsert fails.
    let bad_vec = vec![0.1f32; 4];
    let input = DualWriteInput {
        tenant_id: tenant,
        workspace_id: ws,
        document_id: doc,
        owner_id: owner,
        visibility: "private",
        filename: "t42.pdf",
        chunks: &[Chunk { text: "chunk zero".to_string(), page_start: 1, page_end: 1 }],
        chunk_vectors: vec![bad_vec.clone()],
        extraction: &extraction,
        node_vectors: vec![bad_vec],
    };

    let res = dual_write_ingestion(&pool, &store, input).await;
    assert!(res.is_err(), "wrong-dim vectors must cause Qdrant failure");

    // Postgres must have been rolled back — no chunk rows.
    let chunk_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM document_chunks WHERE document_id = $1")
            .bind(doc)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(chunk_count, 0, "rollback must leave no chunks");

    let node_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM graph_nodes WHERE tenant_id = $1")
            .bind(tenant)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(node_count, 0, "rollback must leave no graph nodes");

    let _ = store.teardown_tenant_collections(tenant).await;
}
