//! Integration tests for metering read routes (T69).

use axum::extract::{Extension, Path};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use gmrag_api::auth::extractor::AuthUser;
use gmrag_api::auth::jwt::JwtClaims;
use gmrag_api::auth::tenant::TenantContext;
use gmrag_api::error::ApiError;
use gmrag_api::metering::{record_embedding_usage, record_llm_usage, METRIC_EMBEDDING_TOKENS, METRIC_LLM_TOKENS};
use gmrag_api::middleware::rls::SharedConnection;
use gmrag_api::routes::metering::{get_audit_logs, get_quotas, get_usage};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

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

async fn add_member(pool: &PgPool, tenant_id: Uuid, user_id: Uuid, role: &str) {
    sqlx::query("INSERT INTO tenant_members (tenant_id, user_id, role) VALUES ($1, $2, $3)")
        .bind(tenant_id)
        .bind(user_id)
        .bind(role)
        .execute(pool)
        .await
        .unwrap();
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

async fn rls_conn(pool: &PgPool, tenant_id: Uuid) -> SharedConnection {
    let mut conn = pool.acquire().await.unwrap().detach();
    sqlx::Executor::execute(&mut conn, "BEGIN").await.unwrap();
    sqlx::Executor::execute(&mut conn, "SET LOCAL ROLE gmrag_app")
        .await
        .unwrap();
    sqlx::query(&format!("SET LOCAL app.tenant_id = '{}'", tenant_id))
        .execute(&mut conn)
        .await
        .unwrap();
    SharedConnection::new(conn)
}

async fn parts(result: Result<impl IntoResponse, ApiError>) -> (StatusCode, Value) {
    let resp = result.into_response();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, body)
}

async fn begin_rls_tx(pool: &PgPool) -> sqlx::Transaction<'static, sqlx::Postgres> {
    let mut tx = pool.begin().await.unwrap();
    sqlx::Executor::execute(&mut *tx, "SET LOCAL ROLE gmrag_app")
        .await
        .unwrap();
    tx
}

async fn set_tenant(tx: &mut sqlx::Transaction<'static, sqlx::Postgres>, tenant_id: Uuid) {
    sqlx::query(&format!("SET LOCAL app.tenant_id = '{tenant_id}'"))
        .execute(&mut **tx)
        .await
        .unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn non_owner_get_usage_forbidden(pool: PgPool) {
    let member = create_user(&pool, "member@t69u.com").await;
    let tenant = insert_tenant(&pool, "T69 Usage Forbidden").await;
    add_member(&pool, tenant, member, "member").await;

    let result = get_usage(
        Path(tenant),
        Extension(TenantContext(tenant)),
        Extension(auth_user(member)),
        Extension(rls_conn(&pool, tenant).await),
    )
    .await;

    assert!(matches!(result, Err(ApiError::Forbidden(_))));
}

#[sqlx::test(migrations = "../../migrations")]
async fn non_owner_get_quotas_forbidden(pool: PgPool) {
    let member = create_user(&pool, "member@t69q.com").await;
    let tenant = insert_tenant(&pool, "T69 Quota Forbidden").await;
    add_member(&pool, tenant, member, "member").await;

    let result = get_quotas(
        Path(tenant),
        Extension(TenantContext(tenant)),
        Extension(auth_user(member)),
        Extension(rls_conn(&pool, tenant).await),
    )
    .await;

    assert!(matches!(result, Err(ApiError::Forbidden(_))));
}

#[sqlx::test(migrations = "../../migrations")]
async fn non_owner_get_audit_logs_forbidden(pool: PgPool) {
    let member = create_user(&pool, "member@t69a.com").await;
    let tenant = insert_tenant(&pool, "T69 Audit Forbidden").await;
    add_member(&pool, tenant, member, "member").await;

    let result = get_audit_logs(
        Path(tenant),
        Extension(TenantContext(tenant)),
        Extension(auth_user(member)),
        Extension(rls_conn(&pool, tenant).await),
    )
    .await;

    assert!(matches!(result, Err(ApiError::Forbidden(_))));
}

#[sqlx::test(migrations = "../../migrations")]
async fn get_usage_aggregates_metrics(pool: PgPool) {
    let owner = create_user(&pool, "owner@t69agg.com").await;
    let tenant = insert_tenant(&pool, "T69 Aggregate").await;
    add_member(&pool, tenant, owner, "owner").await;

    {
        let mut tx = begin_rls_tx(&pool).await;
        set_tenant(&mut tx, tenant).await;
        record_embedding_usage(&mut tx, tenant, "query one", "ollama")
            .await
            .unwrap();
        record_embedding_usage(&mut tx, tenant, "query two", "ollama")
            .await
            .unwrap();
        record_llm_usage(
            &mut tx,
            tenant,
            "user question",
            "assistant answer",
            "deepseek-v4-flash",
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }

    let (status, body) = parts(
        get_usage(
            Path(tenant),
            Extension(TenantContext(tenant)),
            Extension(auth_user(owner)),
            Extension(rls_conn(&pool, tenant).await),
        )
        .await,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let usage = body["usage"].as_array().unwrap();
    assert_eq!(usage.len(), 2);

    let embed = usage
        .iter()
        .find(|row| row["metric"] == METRIC_EMBEDDING_TOKENS)
        .unwrap();
    let llm = usage
        .iter()
        .find(|row| row["metric"] == METRIC_LLM_TOKENS)
        .unwrap();
    assert!(embed["total"].as_i64().unwrap() > 0);
    assert!(llm["total"].as_i64().unwrap() > 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn get_quotas_returns_row_or_defaults(pool: PgPool) {
    let owner = create_user(&pool, "owner@t69qt.com").await;
    let tenant = insert_tenant(&pool, "T69 Quotas").await;
    add_member(&pool, tenant, owner, "owner").await;

    let (status, body) = parts(
        get_quotas(
            Path(tenant),
            Extension(TenantContext(tenant)),
            Extension(auth_user(owner)),
            Extension(rls_conn(&pool, tenant).await),
        )
        .await,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["configured"], false);
    assert_eq!(body["max_documents"], 100);
    assert_eq!(body["max_workspaces"], 10);

    sqlx::query(
        "INSERT INTO tenant_quotas (tenant_id, max_documents, max_workspaces, max_storage_bytes, max_members)
         VALUES ($1, 50, 5, 1000, 20)",
    )
    .bind(tenant)
    .execute(&pool)
    .await
    .unwrap();

    let (status, body) = parts(
        get_quotas(
            Path(tenant),
            Extension(TenantContext(tenant)),
            Extension(auth_user(owner)),
            Extension(rls_conn(&pool, tenant).await),
        )
        .await,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["configured"], true);
    assert_eq!(body["max_documents"], 50);
    assert_eq!(body["max_workspaces"], 5);
}

#[sqlx::test(migrations = "../../migrations")]
async fn get_audit_logs_respects_limit_and_order(pool: PgPool) {
    let owner = create_user(&pool, "owner@t69al.com").await;
    let tenant = insert_tenant(&pool, "T69 Audit").await;
    add_member(&pool, tenant, owner, "owner").await;

    for i in 0..110 {
        sqlx::query(
            "INSERT INTO audit_log (tenant_id, actor_id, action, metadata)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(tenant)
        .bind(owner)
        .bind(format!("test.action.{i}"))
        .bind(serde_json::json!({ "seq": i }))
        .execute(&pool)
        .await
        .unwrap();
    }

    let (status, body) = parts(
        get_audit_logs(
            Path(tenant),
            Extension(TenantContext(tenant)),
            Extension(auth_user(owner)),
            Extension(rls_conn(&pool, tenant).await),
        )
        .await,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let logs = body["logs"].as_array().unwrap();
    assert_eq!(logs.len(), 100);
    let first_seq = logs[0]["metadata"]["seq"].as_i64().unwrap();
    let last_seq = logs[99]["metadata"]["seq"].as_i64().unwrap();
    assert!(first_seq > last_seq, "newest audit entries must come first");
}

#[sqlx::test(migrations = "../../migrations")]
async fn usage_rls_isolates_tenants(pool: PgPool) {
    let owner_a = create_user(&pool, "owner-a@t69r.com").await;
    let tenant_a = insert_tenant(&pool, "Tenant A Usage").await;
    add_member(&pool, tenant_a, owner_a, "owner").await;

    let owner_b = create_user(&pool, "owner-b@t69r.com").await;
    let tenant_b = insert_tenant(&pool, "Tenant B Usage").await;
    add_member(&pool, tenant_b, owner_b, "owner").await;

    {
        let mut tx = begin_rls_tx(&pool).await;
        set_tenant(&mut tx, tenant_a).await;
        record_embedding_usage(&mut tx, tenant_a, "tenant a only", "ollama")
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    let (status, body) = parts(
        get_usage(
            Path(tenant_b),
            Extension(TenantContext(tenant_b)),
            Extension(auth_user(owner_b)),
            Extension(rls_conn(&pool, tenant_b).await),
        )
        .await,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let usage = body["usage"].as_array().unwrap();
    assert!(usage.is_empty(), "tenant B must not see tenant A usage");
}
