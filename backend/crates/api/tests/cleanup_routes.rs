//! Phase 0 (TASK-P0-04) — canonical cleanup behavior integration tests.
//!
//! Workspace cleanup:
//! - generates a workspace-scoped Qdrant cleanup call (correct tenant+workspace);
//! - uses the exact S3 prefix `{tid}/{wid}/`;
//! - is idempotent (repeated deletes are harmless);
//! - external cleanup failure does NOT block the Postgres deletion path.
//!
//! Document graph provenance cleanup:
//! - a node linked only to the deleted document is removed (SQL + Qdrant);
//! - a node shared by two documents remains after deleting one, and is only
//!   removed after the final provenance document is deleted;
//! - related `graph_edges` cascade correctly;
//! - repeated cleanup is harmless.
//!
//! `#[sqlx::test]` cases need a running PostgreSQL instance; when it is
//! unavailable the binary is skipped by the sqlx harness (environmental
//! blocker, not a code failure).

use std::sync::Arc;
use std::sync::Mutex;

use axum::extract::{Extension, Path};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use sqlx::PgPool;
use uuid::Uuid;

use gmrag_api::auth::extractor::AuthUser;
use gmrag_api::auth::jwt::JwtClaims;
use gmrag_api::auth::tenant::TenantContext;
use gmrag_api::authz::AuthzService;
use gmrag_api::error::ApiError;
use gmrag_api::middleware::rls::SharedConnection;
use gmrag_api::routes::documents::delete_document;
use gmrag_api::routes::workspaces::delete_workspace;
use gmrag_api::storage::ObjectStore;
use gmrag_api::vector::{GraphCleaner, VectorCleaner};

#[path = "support/authz.rs"]
mod authz_support;
use authz_support::test_authz;

// ─── mocks ───────────────────────────────────────────────────────────────────

#[derive(Default)]
struct RecordingVectorCleaner {
    doc_cleaned: Mutex<Vec<(Uuid, Uuid)>>,
    ws_cleaned: Mutex<Vec<(Uuid, Uuid)>>,
}

#[async_trait::async_trait]
impl VectorCleaner for RecordingVectorCleaner {
    async fn delete_document_chunks(
        &self,
        tenant_id: Uuid,
        document_id: Uuid,
    ) -> Result<(), String> {
        self.doc_cleaned
            .lock()
            .unwrap()
            .push((tenant_id, document_id));
        Ok(())
    }
    async fn delete_workspace_chunks(
        &self,
        tenant_id: Uuid,
        workspace_id: Uuid,
    ) -> Result<(), String> {
        self.ws_cleaned
            .lock()
            .unwrap()
            .push((tenant_id, workspace_id));
        Ok(())
    }
}

#[derive(Default)]
struct RecordingGraphCleaner {
    deletes: Mutex<Vec<(Uuid, Vec<Uuid>)>>,
}

#[async_trait::async_trait]
impl GraphCleaner for RecordingGraphCleaner {
    async fn delete_graph_nodes(&self, tenant_id: Uuid, node_ids: &[Uuid]) -> Result<(), String> {
        self.deletes
            .lock()
            .unwrap()
            .push((tenant_id, node_ids.to_vec()));
        Ok(())
    }
}

/// Object store that records every prefix delete (workspace cleanup) and
/// single-key delete (document cleanup). Configurable to force a failure
/// for the "external failure does not block DB delete" test.
#[derive(Default)]
struct RecordingObjectStore {
    prefix_deletes: Mutex<Vec<String>>,
    key_deletes: Mutex<Vec<String>>,
    fail_prefix: Mutex<bool>,
}

#[async_trait::async_trait]
impl ObjectStore for RecordingObjectStore {
    async fn put(&self, _key: &str, _data: Vec<u8>, _content_type: &str) -> Result<(), String> {
        Ok(())
    }
    async fn delete(&self, key: &str) -> Result<(), String> {
        self.key_deletes.lock().unwrap().push(key.to_string());
        Ok(())
    }
    async fn delete_prefix(&self, prefix: &str) -> Result<(), String> {
        if *self.fail_prefix.lock().unwrap() {
            return Err("forced s3 failure".into());
        }
        self.prefix_deletes.lock().unwrap().push(prefix.to_string());
        Ok(())
    }
}

// ─── helpers ─────────────────────────────────────────────────────────────────

fn claims_for(user_id: Uuid) -> JwtClaims {
    JwtClaims {
        sub: user_id.to_string(),
        exp: (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp() as u64,
        iat: chrono::Utc::now().timestamp() as u64,
        iss: "http://localhost:8080/realms/gmrag".to_string(),
        aud: None,
        azp: None,
        scope: None,
        preferred_username: None,
        email: None,
        realm_access: None,
    }
}

fn auth_user(user_id: Uuid) -> AuthUser {
    AuthUser::new(user_id, claims_for(user_id))
}

async fn create_user(pool: &PgPool, email: &str) -> Uuid {
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

async fn add_tenant_member(pool: &PgPool, tenant_id: Uuid, user_id: Uuid, role: &str) {
    sqlx::query("INSERT INTO tenant_members (tenant_id, user_id, role) VALUES ($1, $2, $3)")
        .bind(tenant_id)
        .bind(user_id)
        .bind(role)
        .execute(pool)
        .await
        .unwrap();
}

async fn insert_workspace(pool: &PgPool, tenant_id: Uuid, created_by: Uuid, slug: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workspaces (id, tenant_id, name, slug, created_by)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(tenant_id)
    .bind(slug)
    .bind(slug)
    .bind(created_by)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn insert_document(
    pool: &PgPool,
    tenant_id: Uuid,
    workspace_id: Uuid,
    owner_id: Uuid,
    title: &str,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO documents (id, tenant_id, workspace_id, owner_id, title, status, visibility, s3_key)
         VALUES ($1, $2, $3, $4, $5, 'indexed', 'shared', $6)",
    )
    .bind(id)
    .bind(tenant_id)
    .bind(workspace_id)
    .bind(owner_id)
    .bind(title)
    .bind(format!("{tenant_id}/{workspace_id}/{id}.pdf"))
    .execute(pool)
    .await
    .unwrap();
    id
}

/// Insert a graph node (shared, deduped per (tenant, workspace, label, kind))
/// and link it to `document_id` via `graph_node_documents`.
async fn insert_graph_node(
    pool: &PgPool,
    tenant_id: Uuid,
    workspace_id: Uuid,
    label: &str,
    document_id: Uuid,
) -> Uuid {
    let node_id: Uuid = sqlx::query_scalar(
        "INSERT INTO graph_nodes (tenant_id, workspace_id, kind, label, properties)
         VALUES ($1, $2, 'Entity', $3, '{}'::jsonb)
         ON CONFLICT (tenant_id, workspace_id, label, kind) DO UPDATE SET properties = EXCLUDED.properties
         RETURNING id",
    )
    .bind(tenant_id)
    .bind(workspace_id)
    .bind(label)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO graph_node_documents (node_id, document_id, tenant_id)
         VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(node_id)
    .bind(document_id)
    .bind(tenant_id)
    .execute(pool)
    .await
    .unwrap();
    node_id
}

async fn insert_graph_edge(pool: &PgPool, tenant_id: Uuid, src: Uuid, dst: Uuid, kind: &str) {
    sqlx::query(
        "INSERT INTO graph_edges (tenant_id, src_node_id, dst_node_id, kind, weight, properties)
         VALUES ($1, $2, $3, $4, 1.0, '{}'::jsonb)
         ON CONFLICT (src_node_id, dst_node_id, kind) DO NOTHING",
    )
    .bind(tenant_id)
    .bind(src)
    .bind(dst)
    .bind(kind)
    .execute(pool)
    .await
    .unwrap();
}

async fn rls_conn(pool: &PgPool, tenant_id: Uuid) -> SharedConnection {
    let mut conn = pool.acquire().await.unwrap().detach();
    sqlx::Executor::execute(&mut conn, "BEGIN").await.unwrap();
    sqlx::Executor::execute(&mut conn, "SET LOCAL ROLE gmrag_app")
        .await
        .unwrap();
    sqlx::query(&format!("SET LOCAL app.tenant_id = '{tenant_id}'"))
        .execute(&mut conn)
        .await
        .unwrap();
    SharedConnection::new(conn)
}

async fn parts(result: Result<impl IntoResponse, ApiError>) -> (StatusCode, serde_json::Value) {
    let resp = result.into_response();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, body)
}

// ─── workspace cleanup ───────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn workspace_delete_invokes_workspace_qdrant_cleanup_and_s3_prefix(pool: PgPool) {
    let owner = create_user(&pool, "owner@p0wc.com").await;
    let tenant = insert_tenant(&pool, "P0Wc").await;
    add_tenant_member(&pool, tenant, owner, "owner").await;
    let ws = insert_workspace(&pool, tenant, owner, "eng").await;

    let cleaner = Arc::new(RecordingVectorCleaner::default());
    let store = Arc::new(RecordingObjectStore::default());
    let conn = rls_conn(&pool, tenant).await;
    let (status, _) = parts(
        delete_workspace(
            Path((tenant, ws)),
            Extension(TenantContext(tenant)),
            Extension(auth_user(owner)),
            Extension(conn.clone()),
            Extension(test_authz(&pool)),
            Extension(cleaner.clone() as Arc<dyn VectorCleaner>),
            Extension(store.clone() as Arc<dyn ObjectStore>),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Workspace-scoped Qdrant cleanup called with the correct ids.
    assert_eq!(
        cleaner.ws_cleaned.lock().unwrap().as_slice(),
        &[(tenant, ws)],
        "workspace cleanup must use the correct tenant+workspace filter"
    );
    // Exact S3 prefix.
    assert_eq!(
        store.prefix_deletes.lock().unwrap().as_slice(),
        &[format!("{tenant}/{ws}/")],
        "workspace cleanup must use the exact S3 prefix {{tid}}/{{wid}}/"
    );

    // Postgres row gone.
    let mut guard = conn.lock().await;
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workspaces WHERE id = $1")
        .bind(ws)
        .fetch_one(&mut *guard)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn workspace_delete_external_failure_does_not_block_db_delete(pool: PgPool) {
    let owner = create_user(&pool, "owner@p0wcf.com").await;
    let tenant = insert_tenant(&pool, "P0WcF").await;
    add_tenant_member(&pool, tenant, owner, "owner").await;
    let ws = insert_workspace(&pool, tenant, owner, "doomed").await;

    let cleaner = Arc::new(RecordingVectorCleaner::default());
    let store = Arc::new(RecordingObjectStore {
        fail_prefix: Mutex::new(true),
        ..Default::default()
    });
    let conn = rls_conn(&pool, tenant).await;
    let (status, _) = parts(
        delete_workspace(
            Path((tenant, ws)),
            Extension(TenantContext(tenant)),
            Extension(auth_user(owner)),
            Extension(conn.clone()),
            Extension(test_authz(&pool)),
            Extension(cleaner.clone() as Arc<dyn VectorCleaner>),
            Extension(store.clone() as Arc<dyn ObjectStore>),
        )
        .await,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "external cleanup failure must not block the Postgres delete"
    );

    let mut guard = conn.lock().await;
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workspaces WHERE id = $1")
        .bind(ws)
        .fetch_one(&mut *guard)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn workspace_delete_is_idempotent(pool: PgPool) {
    let owner = create_user(&pool, "owner@p0wci.com").await;
    let tenant = insert_tenant(&pool, "P0WcI").await;
    add_tenant_member(&pool, tenant, owner, "owner").await;
    let ws = insert_workspace(&pool, tenant, owner, "idem").await;

    let cleaner = Arc::new(RecordingVectorCleaner::default());
    let store = Arc::new(RecordingObjectStore::default());

    // Reuse ONE SharedConnection for both deletes so the second sees the
    // first's uncommitted changes within the same transaction (the test
    // harness never commits the conn's BEGIN; a second conn would block on
    // the row lock held by the first delete's open transaction).
    let conn = rls_conn(&pool, tenant).await;

    // First delete succeeds.
    let (status, _) = parts(
        delete_workspace(
            Path((tenant, ws)),
            Extension(TenantContext(tenant)),
            Extension(auth_user(owner)),
            Extension(conn.clone()),
            Extension(test_authz(&pool)),
            Extension(cleaner.clone() as Arc<dyn VectorCleaner>),
            Extension(store.clone() as Arc<dyn ObjectStore>),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Second delete returns 404 (already gone within this tx) — no panic,
    // no row-lock wait. Repeated cleanup is harmless.
    let (status2, _) = parts(
        delete_workspace(
            Path((tenant, ws)),
            Extension(TenantContext(tenant)),
            Extension(auth_user(owner)),
            Extension(conn.clone()),
            Extension(test_authz(&pool)),
            Extension(cleaner.clone() as Arc<dyn VectorCleaner>),
            Extension(store.clone() as Arc<dyn ObjectStore>),
        )
        .await,
    )
    .await;
    assert_eq!(status2, StatusCode::NOT_FOUND);

    let n = count_in_tx(&conn, "SELECT COUNT(*) FROM workspaces WHERE id = $1", ws).await;
    assert_eq!(n, 0);
}

// ─── document graph provenance cleanup ───────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn delete_doc_with_cleaners(
    pool: &PgPool,
    tenant: Uuid,
    doc: Uuid,
    owner: Uuid,
    authz: AuthzService,
    cleaner: Arc<dyn VectorCleaner>,
    graph_cleaner: Arc<dyn GraphCleaner>,
    store: Arc<dyn ObjectStore>,
) -> (StatusCode, SharedConnection) {
    let conn = rls_conn(pool, tenant).await;
    let status = delete_doc_on_conn(
        conn.clone(),
        tenant,
        doc,
        owner,
        authz,
        cleaner,
        graph_cleaner,
        store,
    )
    .await;
    (status, conn)
}

/// Run `delete_document` on an EXISTING SharedConnection (the test harness
/// never commits the conn's BEGIN, so multi-step tests must reuse one conn
/// to see prior uncommitted deletes within the same transaction).
#[allow(clippy::too_many_arguments)]
async fn delete_doc_on_conn(
    conn: SharedConnection,
    tenant: Uuid,
    doc: Uuid,
    owner: Uuid,
    authz: AuthzService,
    cleaner: Arc<dyn VectorCleaner>,
    graph_cleaner: Arc<dyn GraphCleaner>,
    store: Arc<dyn ObjectStore>,
) -> StatusCode {
    let (status, _) = parts(
        delete_document(
            Path((tenant, doc)),
            Extension(TenantContext(tenant)),
            Extension(auth_user(owner)),
            Extension(conn),
            Extension(authz),
            Extension(store.clone() as Arc<dyn ObjectStore>),
            Extension(cleaner.clone() as Arc<dyn VectorCleaner>),
            Extension(graph_cleaner.clone() as Arc<dyn GraphCleaner>),
        )
        .await,
    )
    .await;
    status
}

/// Count rows in `table` matching `expr` from within the handler's
/// SharedConnection transaction (the test harness never commits the
/// SharedConnection's BEGIN, so reads must happen on the same tx to see
/// uncommitted deletes — mirroring the existing T59 document-route tests).
async fn count_in_tx(conn: &SharedConnection, sql: &str, bind: Uuid) -> i64 {
    let mut guard = conn.lock().await;
    sqlx::query_scalar(sql)
        .bind(bind)
        .fetch_one(&mut *guard)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn document_delete_removes_sole_provenance_graph_node_and_qdrant_point(pool: PgPool) {
    let owner = create_user(&pool, "owner@p0g1.com").await;
    let tenant = insert_tenant(&pool, "P0G1").await;
    add_tenant_member(&pool, tenant, owner, "owner").await;
    let ws = insert_workspace(&pool, tenant, owner, "g1").await;
    let doc = insert_document(&pool, tenant, ws, owner, "only-prov").await;
    let node = insert_graph_node(&pool, tenant, ws, "Alice", doc).await;

    let cleaner = Arc::new(RecordingVectorCleaner::default());
    let graph_cleaner = Arc::new(RecordingGraphCleaner::default());
    let store = Arc::new(RecordingObjectStore::default());

    let (status, conn) = delete_doc_with_cleaners(
        &pool,
        tenant,
        doc,
        owner,
        test_authz(&pool),
        cleaner,
        graph_cleaner.clone(),
        store,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // The sole-provenance node is gone from Postgres (read within the
    // handler's transaction — the test harness does not commit it).
    let n = count_in_tx(
        &conn,
        "SELECT COUNT(*) FROM graph_nodes WHERE id = $1",
        node,
    )
    .await;
    assert_eq!(n, 0, "sole-provenance node must be removed");

    // The Qdrant graph point was deleted in one bulk call with the node id.
    let deletes = graph_cleaner.deletes.lock().unwrap().clone();
    assert_eq!(deletes.len(), 1, "exactly one bulk graph delete call");
    assert_eq!(deletes[0].0, tenant);
    assert_eq!(deletes[0].1, vec![node]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn document_delete_keeps_shared_graph_node_until_last_provenance_deleted(pool: PgPool) {
    let owner = create_user(&pool, "owner@p0g2.com").await;
    let tenant = insert_tenant(&pool, "P0G2").await;
    add_tenant_member(&pool, tenant, owner, "owner").await;
    let ws = insert_workspace(&pool, tenant, owner, "g2").await;
    let doc_a = insert_document(&pool, tenant, ws, owner, "a").await;
    let doc_b = insert_document(&pool, tenant, ws, owner, "b").await;
    // Shared node linked to both documents.
    let node = insert_graph_node(&pool, tenant, ws, "SharedEntity", doc_a).await;
    insert_graph_node(&pool, tenant, ws, "SharedEntity", doc_b).await;
    // Edge from the shared node to a per-doc node so cascading is exercised.
    let only_b = insert_graph_node(&pool, tenant, ws, "OnlyB", doc_b).await;
    insert_graph_edge(&pool, tenant, node, only_b, "related_to").await;

    let cleaner = Arc::new(RecordingVectorCleaner::default());
    let store = Arc::new(RecordingObjectStore::default());
    let conn = rls_conn(&pool, tenant).await;
    let authz = test_authz(&pool);

    // Delete doc_a first: the shared node must survive (still referenced by
    // doc_b). Reuse the same SharedConnection so the second delete sees the
    // first delete's uncommitted changes within the same transaction.
    let graph_cleaner = Arc::new(RecordingGraphCleaner::default());
    let status = delete_doc_on_conn(
        conn.clone(),
        tenant,
        doc_a,
        owner,
        authz.clone(),
        cleaner.clone(),
        graph_cleaner,
        store.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let shared_n = count_in_tx(
        &conn,
        "SELECT COUNT(*) FROM graph_nodes WHERE id = $1",
        node,
    )
    .await;
    assert_eq!(
        shared_n, 1,
        "shared node must remain while another document still proves it"
    );

    // Now delete doc_b (the final provenance). The shared node + only_b node
    // both become orphans and are removed; the edge cascades away.
    let graph_cleaner2 = Arc::new(RecordingGraphCleaner::default());
    let status = delete_doc_on_conn(
        conn.clone(),
        tenant,
        doc_b,
        owner,
        authz,
        cleaner,
        graph_cleaner2,
        store,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let shared_after = count_in_tx(
        &conn,
        "SELECT COUNT(*) FROM graph_nodes WHERE id = $1",
        node,
    )
    .await;
    assert_eq!(
        shared_after, 0,
        "shared node removed after final provenance deleted"
    );

    let only_b_after = count_in_tx(
        &conn,
        "SELECT COUNT(*) FROM graph_nodes WHERE id = $1",
        only_b,
    )
    .await;
    assert_eq!(only_b_after, 0, "sole-provenance node removed");

    // The edge is gone (cascade via graph_nodes FK).
    let edges = count_in_tx(
        &conn,
        "SELECT COUNT(*) FROM graph_edges WHERE src_node_id = $1 OR dst_node_id = $1",
        node,
    )
    .await;
    assert_eq!(edges, 0, "related graph_edges cascade correctly");
}

#[sqlx::test(migrations = "../../migrations")]
async fn document_delete_cleanup_is_idempotent(pool: PgPool) {
    let owner = create_user(&pool, "owner@p0g3.com").await;
    let tenant = insert_tenant(&pool, "P0G3").await;
    add_tenant_member(&pool, tenant, owner, "owner").await;
    let ws = insert_workspace(&pool, tenant, owner, "g3").await;
    let doc = insert_document(&pool, tenant, ws, owner, "idem").await;
    insert_graph_node(&pool, tenant, ws, "Solo", doc).await;

    let conn = rls_conn(&pool, tenant).await;
    let cleaner = Arc::new(RecordingVectorCleaner::default());
    let graph_cleaner = Arc::new(RecordingGraphCleaner::default());
    let store = Arc::new(RecordingObjectStore::default());
    let authz = test_authz(&pool);

    // First delete removes the document + sole-provenance node.
    let status = delete_doc_on_conn(
        conn.clone(),
        tenant,
        doc,
        owner,
        authz.clone(),
        cleaner.clone(),
        graph_cleaner.clone(),
        store.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Second delete of the same doc on the same transaction → 404 (already
    // gone), no panic, no row touched. Repeated cleanup is harmless.
    let status2 = delete_doc_on_conn(
        conn.clone(),
        tenant,
        doc,
        owner,
        authz,
        cleaner,
        graph_cleaner,
        store,
    )
    .await;
    assert_eq!(status2, StatusCode::NOT_FOUND);
}
