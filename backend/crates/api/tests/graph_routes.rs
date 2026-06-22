//! Integration tests for graph routes (T63).

use axum::extract::{Extension, Path};
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
use gmrag_api::routes::graph::get_workspace_graph;

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

#[sqlx::test(migrations = "../../migrations")]
async fn get_graph_returns_nodes_and_edges(pool: PgPool) {
    let tenant = insert_tenant(&pool, "graph-tenant").await;
    let user = create_user(&pool, "graph-user@test.com").await;
    let ws = insert_workspace(&pool, tenant, user, "graph-ws").await;
    add_workspace_member(&pool, ws, tenant, user).await;

    let node_a: Uuid = sqlx::query_scalar(
        "INSERT INTO graph_nodes (tenant_id, workspace_id, kind, label)
         VALUES ($1, $2, 'concept', 'Alpha') RETURNING id",
    )
    .bind(tenant)
    .bind(ws)
    .fetch_one(&pool)
    .await
    .unwrap();

    let node_b: Uuid = sqlx::query_scalar(
        "INSERT INTO graph_nodes (tenant_id, workspace_id, kind, label)
         VALUES ($1, $2, 'concept', 'Beta') RETURNING id",
    )
    .bind(tenant)
    .bind(ws)
    .fetch_one(&pool)
    .await
    .unwrap();

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
    let resp = get_workspace_graph(
        Path((tenant, ws)),
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
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(body["edges"].as_array().unwrap().len(), 1);
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
        Extension(TenantContext(tenant_a)),
        Extension(auth_user(user)),
        Extension(conn),
    )
    .await;
    assert!(matches!(err, Err(ApiError::NotFound)));
}
