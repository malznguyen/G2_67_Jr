//! Integration tests for graph routes (T63, T84D pagination + ACL).

use axum::extract::{Extension, Path, Query};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use gmrag_api::auth::extractor::AuthUser;
use gmrag_api::auth::jwt::JwtClaims;
use gmrag_api::auth::tenant::TenantContext;
use gmrag_api::error::ApiError;
use gmrag_api::middleware::rls::SharedConnection;
use gmrag_api::routes::graph::{get_workspace_graph, GraphQueryParams};

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

fn default_query() -> Query<GraphQueryParams> {
    Query(GraphQueryParams {
        cursor: None,
        limit: None,
    })
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

async fn add_workspace_member(pool: &PgPool, workspace_id: Uuid, tenant_id: Uuid, user_id: Uuid) {
    sqlx::query(
        "INSERT INTO workspace_members (workspace_id, tenant_id, user_id, role)
         VALUES ($1, $2, $3, 'member')",
    )
    .bind(workspace_id)
    .bind(tenant_id)
    .bind(user_id)
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_document(
    pool: &PgPool,
    id: Uuid,
    tenant_id: Uuid,
    workspace_id: Uuid,
    owner_id: Uuid,
    title: &str,
    visibility: &str,
) {
    sqlx::query(
        "INSERT INTO documents (id, tenant_id, workspace_id, owner_id, title, status, visibility, s3_key)
         VALUES ($1, $2, $3, $4, $5, 'ready', $6, 'k')",
    )
    .bind(id)
    .bind(tenant_id)
    .bind(workspace_id)
    .bind(owner_id)
    .bind(title)
    .bind(visibility)
    .execute(pool)
    .await
    .unwrap();
}

async fn link_node_document(
    pool: &PgPool,
    node_id: Uuid,
    document_id: Uuid,
    tenant_id: Uuid,
) {
    sqlx::query(
        "INSERT INTO graph_node_documents (node_id, document_id, tenant_id)
         VALUES ($1, $2, $3)",
    )
    .bind(node_id)
    .bind(document_id)
    .bind(tenant_id)
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_doc_grant(
    pool: &PgPool,
    tenant_id: Uuid,
    doc_id: Uuid,
    principal_type: &str,
    principal_id: Uuid,
    permission: &str,
) {
    sqlx::query(
        "INSERT INTO resource_acl (tenant_id, resource_type, resource_id, principal_type, principal_id, permission)
         VALUES ($1, 'document', $2, $3, $4, $5)",
    )
    .bind(tenant_id)
    .bind(doc_id)
    .bind(principal_type)
    .bind(principal_id)
    .bind(permission)
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

async fn call_graph(
    tenant: Uuid,
    ws: Uuid,
    user: Uuid,
    conn: SharedConnection,
    params: GraphQueryParams,
) -> Value {
    let resp = get_workspace_graph(
        Path((tenant, ws)),
        Query(params),
        Extension(TenantContext(tenant)),
        Extension(auth_user(user)),
        Extension(conn),
    )
    .await
    .unwrap()
    .into_response();

    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn get_graph_returns_nodes_and_edges(pool: PgPool) {
    let tenant = insert_tenant(&pool, "graph-tenant").await;
    let user = create_user(&pool, "graph-user@test.com").await;
    let ws = insert_workspace(&pool, tenant, user, "graph-ws").await;
    add_workspace_member(&pool, ws, tenant, user).await;

    let doc = Uuid::new_v4();
    insert_document(&pool, doc, tenant, ws, user, "graph-doc", "shared").await;

    let node_a: Uuid = sqlx::query_scalar(
        "INSERT INTO graph_nodes (tenant_id, workspace_id, kind, label)
         VALUES ($1, $2, 'concept', 'Alpha') RETURNING id",
    )
    .bind(tenant)
    .bind(ws)
    .fetch_one(&pool)
    .await
    .unwrap();
    link_node_document(&pool, node_a, doc, tenant).await;

    let node_b: Uuid = sqlx::query_scalar(
        "INSERT INTO graph_nodes (tenant_id, workspace_id, kind, label)
         VALUES ($1, $2, 'concept', 'Beta') RETURNING id",
    )
    .bind(tenant)
    .bind(ws)
    .fetch_one(&pool)
    .await
    .unwrap();
    link_node_document(&pool, node_b, doc, tenant).await;

    sqlx::query(
        "INSERT INTO graph_edges (tenant_id, src_node_id, dst_node_id, kind)
         VALUES ($1, $2, $3, 'related_to')",
    )
    .bind(tenant)
    .bind(node_a)
    .bind(node_b)
    .execute(&pool)
    .await
    .unwrap();

    let conn = rls_conn(&pool, tenant).await;
    let body = call_graph(tenant, ws, user, conn, GraphQueryParams {
        cursor: None,
        limit: None,
    })
    .await;

    assert_eq!(body["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(body["edges"].as_array().unwrap().len(), 1);
    assert!(body["next_cursor"].is_null());
}

#[sqlx::test(migrations = "../../migrations")]
async fn get_graph_paginates_with_cursor(pool: PgPool) {
    let tenant = insert_tenant(&pool, "graph-page-tenant").await;
    let user = create_user(&pool, "graph-page@test.com").await;
    let ws = insert_workspace(&pool, tenant, user, "graph-page-ws").await;
    add_workspace_member(&pool, ws, tenant, user).await;

    let doc = Uuid::new_v4();
    insert_document(&pool, doc, tenant, ws, user, "page-doc", "shared").await;

    let base = chrono::Utc::now() - chrono::Duration::hours(1);
    for i in 0..3 {
        let ts = base + chrono::Duration::minutes(i);
        let node_id: Uuid = sqlx::query_scalar(
            "INSERT INTO graph_nodes (tenant_id, workspace_id, kind, label, created_at)
             VALUES ($1, $2, 'concept', $3, $4) RETURNING id",
        )
        .bind(tenant)
        .bind(ws)
        .bind(format!("Node{i}"))
        .bind(ts)
        .fetch_one(&pool)
        .await
        .unwrap();
        link_node_document(&pool, node_id, doc, tenant).await;
    }

    let conn = rls_conn(&pool, tenant).await;
    let page1 = call_graph(
        tenant,
        ws,
        user,
        conn,
        GraphQueryParams {
            cursor: None,
            limit: Some(2),
        },
    )
    .await;

    assert_eq!(page1["nodes"].as_array().unwrap().len(), 2);
    let cursor = page1["next_cursor"]
        .as_str()
        .expect("second page expected")
        .to_string();

    let conn = rls_conn(&pool, tenant).await;
    let page2 = call_graph(
        tenant,
        ws,
        user,
        conn,
        GraphQueryParams {
            cursor: Some(cursor),
            limit: Some(2),
        },
    )
    .await;

    assert_eq!(page2["nodes"].as_array().unwrap().len(), 1);
    assert!(page2["next_cursor"].is_null());
}

#[sqlx::test(migrations = "../../migrations")]
async fn get_graph_acl_hides_private_node(pool: PgPool) {
    let tenant = insert_tenant(&pool, "graph-acl-tenant").await;
    let owner = create_user(&pool, "graph-acl-owner@test.com").await;
    let stranger = create_user(&pool, "graph-acl-stranger@test.com").await;
    let viewer = create_user(&pool, "graph-acl-viewer@test.com").await;
    let ws = insert_workspace(&pool, tenant, owner, "graph-acl-ws").await;
    add_workspace_member(&pool, ws, tenant, owner).await;

    let private_doc = Uuid::new_v4();
    insert_document(
        &pool,
        private_doc,
        tenant,
        ws,
        owner,
        "secret.pdf",
        "private",
    )
    .await;

    let node_id: Uuid = sqlx::query_scalar(
        "INSERT INTO graph_nodes (tenant_id, workspace_id, kind, label)
         VALUES ($1, $2, 'concept', 'Secret') RETURNING id",
    )
    .bind(tenant)
    .bind(ws)
    .fetch_one(&pool)
    .await
    .unwrap();
    link_node_document(&pool, node_id, private_doc, tenant).await;

    let conn = rls_conn(&pool, tenant).await;
    let body = call_graph(tenant, ws, stranger, conn, GraphQueryParams {
        cursor: None,
        limit: None,
    })
    .await;
    assert_eq!(body["nodes"].as_array().unwrap().len(), 0);

    insert_doc_grant(&pool, tenant, private_doc, "user", viewer, "viewer").await;
    let conn = rls_conn(&pool, tenant).await;
    let body = call_graph(tenant, ws, viewer, conn, GraphQueryParams {
        cursor: None,
        limit: None,
    })
    .await;
    assert_eq!(body["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(body["nodes"][0]["id"].as_str().unwrap(), node_id.to_string());
}

#[sqlx::test(migrations = "../../migrations")]
async fn get_graph_denied_for_non_member(pool: PgPool) {
    let tenant = insert_tenant(&pool, "graph-deny-tenant").await;
    let owner = create_user(&pool, "graph-owner@test.com").await;
    let stranger = create_user(&pool, "graph-stranger@test.com").await;
    let ws = insert_workspace(&pool, tenant, owner, "graph-deny-ws").await;
    add_workspace_member(&pool, ws, tenant, owner).await;

    let conn = rls_conn(&pool, tenant).await;
    let err = get_workspace_graph(
        Path((tenant, ws)),
        default_query(),
        Extension(TenantContext(tenant)),
        Extension(auth_user(stranger)),
        Extension(conn),
    )
    .await;
    assert!(matches!(err, Err(ApiError::NotFound)));
}

#[sqlx::test(migrations = "../../migrations")]
async fn get_graph_cross_tenant(pool: PgPool) {
    let tenant_a = insert_tenant(&pool, "graph-tenant-a").await;
    let tenant_b = insert_tenant(&pool, "graph-tenant-b").await;
    let user = create_user(&pool, "graph-xtenant@test.com").await;
    let ws = insert_workspace(&pool, tenant_b, user, "graph-b-ws").await;
    add_workspace_member(&pool, ws, tenant_b, user).await;

    let conn = rls_conn(&pool, tenant_a).await;
    let err = get_workspace_graph(
        Path((tenant_a, ws)),
        default_query(),
        Extension(TenantContext(tenant_a)),
        Extension(auth_user(user)),
        Extension(conn),
    )
    .await;
    assert!(matches!(err, Err(ApiError::NotFound)));
}
