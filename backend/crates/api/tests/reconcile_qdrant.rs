//! Phase 3 — Qdrant reconciler integration tests.
//!
//! Requires a running PostgreSQL (migrations via `#[sqlx::test]`) AND a
//! running Qdrant gRPC at localhost:6334. Verifies:
//! - orphaned chunk points (document_id with no Postgres row) are detected;
//! - orphaned graph points (node_id with no Postgres row) are detected;
//! - a document with fewer Qdrant points than Postgres chunks is reported
//!   as missing (and NOT re-embedded even with auto-fix);
//! - `auto_fix = true` deletes orphaned points;
//! - `auto_fix = false` makes NO delete call to Qdrant even with drift.

use gmrag_api::reconcile::run_qdrant_reconcile;
use gmrag_core::config::QdrantConfig;
use gmrag_core::QdrantStore;
use qdrant_client::qdrant::PointStruct;
use qdrant_client::Payload;
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

fn vec768(seed: f32) -> Vec<f32> {
    vec![seed; 768]
}

async fn insert_tenant(pool: &PgPool, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, $2)")
        .bind(id)
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn insert_user(pool: &PgPool, email: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, email, name) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(email)
        .bind(email)
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn insert_document(pool: &PgPool, tenant: Uuid, owner: Uuid, status: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO documents (id, tenant_id, owner_id, title, status, visibility, s3_key)
         VALUES ($1, $2, $3, 'd', $4, 'private', 'k')",
    )
    .bind(id)
    .bind(tenant)
    .bind(owner)
    .bind(status)
    .execute(pool)
    .await
    .unwrap();
    id
}

/// Insert a `document_chunks` row and return its id (the Qdrant point id).
async fn insert_chunk_row(pool: &PgPool, tenant: Uuid, doc: Uuid, idx: i32) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO document_chunks (id, tenant_id, document_id, chunk_index, content, qdrant_point_id)
         VALUES ($1, $2, $3, $4, 'c', $5)",
    )
    .bind(id)
    .bind(tenant)
    .bind(doc)
    .bind(idx)
    .bind(id)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn insert_graph_node(pool: &PgPool, tenant: Uuid, ws: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO graph_nodes (id, tenant_id, workspace_id, kind, label, properties)
         VALUES ($1, $2, $3, 'Concept', 'n', '{}'::jsonb)",
    )
    .bind(id)
    .bind(tenant)
    .bind(ws)
    .execute(pool)
    .await
    .unwrap();
    id
}

fn chunk_point(point_id: Uuid, doc_id: Uuid) -> PointStruct {
    let payload = Payload::try_from(serde_json::json!({
        "document_id": doc_id.to_string(),
        "workspace_id": Uuid::nil().to_string(),
    }))
    .expect("chunk payload");
    PointStruct::new(point_id.to_string(), vec768(0.1), payload)
}

fn graph_point(point_id: Uuid, node_id: Uuid) -> PointStruct {
    let payload = Payload::try_from(serde_json::json!({
        "node_id": node_id.to_string(),
        "workspace_id": Uuid::nil().to_string(),
        "entity_name": "n",
    }))
    .expect("graph payload");
    PointStruct::new(point_id.to_string(), vec768(0.2), payload)
}

async fn insert_workspace(pool: &PgPool, tenant: Uuid, owner: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workspaces (id, tenant_id, name, slug, created_by) VALUES ($1, $2, 'ws', $3, $4)",
    )
    .bind(id)
    .bind(tenant)
    .bind(format!("ws-{id}"))
    .bind(owner)
    .execute(pool)
    .await
    .unwrap();
    id
}

#[sqlx::test(migrations = "../../migrations")]
async fn qdrant_reconcile_detects_orphans_and_missing_and_preserves_in_dry_run(pool: PgPool) {
    let tenant = insert_tenant(&pool, "qrec").await;
    let owner = insert_user(&pool, "owner@qrec.test").await;
    let ws = insert_workspace(&pool, tenant, owner).await;
    let store = qdrant().await;
    store.setup_tenant_collections(tenant).await.unwrap();

    // Live document with 2 Postgres chunks but only 1 Qdrant point → missing.
    let live_doc = insert_document(&pool, tenant, owner, "indexed").await;
    let chunk_a = insert_chunk_row(&pool, tenant, live_doc, 0).await;
    let _chunk_b = insert_chunk_row(&pool, tenant, live_doc, 1).await; // no Qdrant point
    store
        .upsert_chunks(tenant, vec![chunk_point(chunk_a, live_doc)])
        .await
        .unwrap();

    // Orphaned chunk point: document_id has no Postgres row.
    let ghost_doc = Uuid::new_v4();
    let orphan_chunk_id = Uuid::new_v4();
    store
        .upsert_chunks(tenant, vec![chunk_point(orphan_chunk_id, ghost_doc)])
        .await
        .unwrap();

    // Live graph node + orphaned graph point (node_id with no Postgres row).
    let _live_node = insert_graph_node(&pool, tenant, ws).await;
    let ghost_node = Uuid::new_v4();
    let orphan_graph_id = Uuid::new_v4();
    store
        .upsert_graph_nodes(tenant, vec![graph_point(orphan_graph_id, ghost_node)])
        .await
        .unwrap();

    let report = run_qdrant_reconcile(&pool, &store, false)
        .await
        .expect("reconcile");

    assert_eq!(
        report.orphaned_chunk_points.count, 1,
        "one orphaned chunk point"
    );
    assert!(
        report
            .orphaned_chunk_points
            .sample
            .iter()
            .any(|s| s.contains(&format!("document_id={ghost_doc}"))),
        "orphaned chunk should reference ghost doc"
    );
    assert_eq!(
        report.missing_chunk_points.count, 1,
        "one doc with missing points"
    );
    assert!(
        report
            .missing_chunk_points
            .sample
            .iter()
            .any(|s| s.contains(&format!("document_id={live_doc}"))),
        "missing chunk should reference the live doc"
    );
    assert_eq!(
        report.orphaned_graph_points.count, 1,
        "one orphaned graph point"
    );
    assert!(
        report
            .orphaned_graph_points
            .sample
            .iter()
            .any(|s| s.contains(&format!("node_id={ghost_node}"))),
        "orphaned graph should reference ghost node"
    );
    assert!(
        report.missing_graph_points.count >= 1,
        "live node missing a Qdrant point"
    );

    assert!(!report.auto_fix_ran);
    assert_eq!(report.deleted_chunk_docs, 0);
    assert_eq!(report.deleted_graph_nodes, 0);

    let chunk_refs = store.scroll_chunk_refs(tenant).await.unwrap();
    assert!(
        chunk_refs.iter().any(|r| r.document_id == Some(ghost_doc)),
        "dry-run must leave orphaned chunk point in place"
    );

    let _ = store.teardown_tenant_collections(tenant).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn qdrant_reconcile_auto_fix_deletes_orphans_but_not_missing(pool: PgPool) {
    let tenant = insert_tenant(&pool, "qrec-fix").await;
    let owner = insert_user(&pool, "owner@qrec-fix.test").await;
    let ws = insert_workspace(&pool, tenant, owner).await;
    let store = qdrant().await;
    store.setup_tenant_collections(tenant).await.unwrap();

    let live_doc = insert_document(&pool, tenant, owner, "indexed").await;
    let chunk_a = insert_chunk_row(&pool, tenant, live_doc, 0).await;
    let _chunk_b = insert_chunk_row(&pool, tenant, live_doc, 1).await;
    store
        .upsert_chunks(tenant, vec![chunk_point(chunk_a, live_doc)])
        .await
        .unwrap();

    let ghost_doc = Uuid::new_v4();
    let orphan_chunk_id = Uuid::new_v4();
    store
        .upsert_chunks(tenant, vec![chunk_point(orphan_chunk_id, ghost_doc)])
        .await
        .unwrap();

    let _live_node = insert_graph_node(&pool, tenant, ws).await;
    let ghost_node = Uuid::new_v4();
    let orphan_graph_id = Uuid::new_v4();
    store
        .upsert_graph_nodes(tenant, vec![graph_point(orphan_graph_id, ghost_node)])
        .await
        .unwrap();

    let report = run_qdrant_reconcile(&pool, &store, true)
        .await
        .expect("reconcile auto-fix");

    assert!(report.auto_fix_ran);
    assert_eq!(report.deleted_chunk_docs, 1, "orphaned chunk doc deleted");
    assert_eq!(report.deleted_graph_nodes, 1, "orphaned graph node deleted");
    assert_eq!(
        report.missing_chunk_points.count, 1,
        "missing still reported, NOT re-embedded"
    );

    let chunk_refs = store.scroll_chunk_refs(tenant).await.unwrap();
    assert!(
        !chunk_refs.iter().any(|r| r.document_id == Some(ghost_doc)),
        "orphaned chunk point deleted by auto-fix"
    );
    assert!(
        chunk_refs.iter().any(|r| r.document_id == Some(live_doc)),
        "live doc chunk point preserved"
    );

    let _ = store.teardown_tenant_collections(tenant).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn qdrant_reconcile_dry_run_never_deletes(pool: PgPool) {
    let tenant = insert_tenant(&pool, "qrec-gate").await;
    let _owner = insert_user(&pool, "owner@qrec-gate.test").await;
    let store = qdrant().await;
    store.setup_tenant_collections(tenant).await.unwrap();

    let ghost_doc = Uuid::new_v4();
    let orphan_chunk_id = Uuid::new_v4();
    store
        .upsert_chunks(tenant, vec![chunk_point(orphan_chunk_id, ghost_doc)])
        .await
        .unwrap();

    let ghost_node = Uuid::new_v4();
    let orphan_graph_id = Uuid::new_v4();
    store
        .upsert_graph_nodes(tenant, vec![graph_point(orphan_graph_id, ghost_node)])
        .await
        .unwrap();

    let report = run_qdrant_reconcile(&pool, &store, false)
        .await
        .expect("reconcile");
    assert_eq!(report.orphaned_chunk_points.count, 1);
    assert_eq!(report.orphaned_graph_points.count, 1);
    assert!(!report.auto_fix_ran);
    assert_eq!(report.deleted_chunk_docs, 0);
    assert_eq!(report.deleted_graph_nodes, 0);

    let chunk_refs = store.scroll_chunk_refs(tenant).await.unwrap();
    assert!(chunk_refs.iter().any(|r| r.document_id == Some(ghost_doc)));
    let graph_refs = store.scroll_graph_node_refs(tenant).await.unwrap();
    assert!(graph_refs.iter().any(|r| r.node_id == Some(ghost_node)));

    let _ = store.teardown_tenant_collections(tenant).await;
}
