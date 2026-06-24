//! Integration tests for BYOK LLM settings routes (T68).

use axum::extract::{Extension, Path};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use gmrag_core::config::{DeepSeekConfig, OllamaConfig};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use gmrag_api::auth::extractor::AuthUser;
use gmrag_api::auth::jwt::JwtClaims;
use gmrag_api::auth::tenant::TenantContext;
use gmrag_api::error::ApiError;
use gmrag_api::middleware::rls::SharedConnection;
use gmrag_api::routes::chat::LlmRuntime;
use gmrag_api::routes::settings::{get_llm_settings, put_llm_settings, PutLlmSettingsBody};

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

fn test_llm_runtime(key: Option<[u8; 32]>) -> LlmRuntime {
    LlmRuntime {
        deepseek: DeepSeekConfig {
            api_key: Some("sk-global".into()),
            base_url: "https://api.deepseek.com/v1".into(),
            model: "deepseek-v4-flash".into(),
            timeout_s: 60,
        },
        ollama: OllamaConfig {
            host: "http://ollama:11434".into(),
            embed_model: "nomic-embed-text".into(),
            llm_model: "llama3.1:8b".into(),
            keep_alive: "30m".into(),
        },
        tenant_key_encryption_key: key,
        // T84D Phase 3.3: chat history limit (default 10) — keeps the
        // pre-T84D tests passing without touching history behaviour.
        chat_history_limit: 10,
    }
}

const ENC_KEY: [u8; 32] = [7_u8; 32];

#[sqlx::test(migrations = "../../migrations")]
async fn non_owner_get_llm_settings_forbidden(pool: PgPool) {
    let member = create_user(&pool, "member@t68.com").await;
    let tenant = insert_tenant(&pool, "T68 Forbidden").await;
    add_member(&pool, tenant, member, "member").await;

    let result = get_llm_settings(
        Path(tenant),
        Extension(TenantContext(tenant)),
        Extension(auth_user(member)),
        Extension(rls_conn(&pool, tenant).await),
        Extension(test_llm_runtime(Some(ENC_KEY))),
    )
    .await;

    assert!(matches!(result, Err(ApiError::Forbidden(_))));
}

#[sqlx::test(migrations = "../../migrations")]
async fn non_owner_put_llm_settings_forbidden(pool: PgPool) {
    let member = create_user(&pool, "member@t68p.com").await;
    let tenant = insert_tenant(&pool, "T68 Put Forbidden").await;
    add_member(&pool, tenant, member, "member").await;

    let result = put_llm_settings(
        Path(tenant),
        Extension(TenantContext(tenant)),
        Extension(auth_user(member)),
        Extension(rls_conn(&pool, tenant).await),
        Extension(test_llm_runtime(Some(ENC_KEY))),
        Json(PutLlmSettingsBody {
            provider: "openai".into(),
            model: "text-embedding-3-small".into(),
            base_url: None,
            api_key: Some("sk-test-key".into()),
            dimensions: None,
            enabled: None,
            llm_model: None,
            llm_base_url: None,
        }),
    )
    .await;

    assert!(matches!(result, Err(ApiError::Forbidden(_))));
}

#[sqlx::test(migrations = "../../migrations")]
async fn get_llm_settings_unconfigured_returns_default(pool: PgPool) {
    let owner = create_user(&pool, "owner@t68u.com").await;
    let tenant = insert_tenant(&pool, "T68 Unconfigured").await;
    add_member(&pool, tenant, owner, "owner").await;

    let (status, body) = parts(
        get_llm_settings(
            Path(tenant),
            Extension(TenantContext(tenant)),
            Extension(auth_user(owner)),
            Extension(rls_conn(&pool, tenant).await),
            Extension(test_llm_runtime(Some(ENC_KEY))),
        )
        .await,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["configured"], false);
    assert_eq!(body["has_api_key"], false);
}

#[sqlx::test(migrations = "../../migrations")]
async fn put_llm_settings_encrypts_and_get_masks(pool: PgPool) {
    let owner = create_user(&pool, "owner@t68e.com").await;
    let tenant = insert_tenant(&pool, "T68 Encrypt").await;
    add_member(&pool, tenant, owner, "owner").await;

    let api_key = "sk-proj-abc123xyz789";
    let conn = rls_conn(&pool, tenant).await;
    let (status, put_body) = parts(
        put_llm_settings(
            Path(tenant),
            Extension(TenantContext(tenant)),
            Extension(auth_user(owner)),
            Extension(conn.clone()),
            Extension(test_llm_runtime(Some(ENC_KEY))),
            Json(PutLlmSettingsBody {
                provider: "openai".into(),
                model: "text-embedding-3-small".into(),
                base_url: Some("https://api.openai.com/v1".into()),
                api_key: Some(api_key.into()),
                dimensions: Some(768),
                enabled: Some(true),
                llm_model: Some("gpt-4o-mini".into()),
                llm_base_url: None,
            }),
        )
        .await,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(put_body["configured"], true);
    assert_eq!(put_body["has_api_key"], true);
    assert!(put_body["api_key_masked"]
        .as_str()
        .unwrap()
        .contains("789"));
    assert!(!put_body["api_key_masked"]
        .as_str()
        .unwrap()
        .contains("abc123"));

    {
        let mut guard = conn.lock().await;
        sqlx::Executor::execute(&mut *guard, "COMMIT").await.unwrap();
    }

    let (ct, nonce, plaintext): (Option<Vec<u8>>, Option<Vec<u8>>, Option<String>) =
        sqlx::query_as(
            "SELECT api_key_ciphertext, api_key_nonce, api_key
             FROM tenant_llm_config WHERE tenant_id = $1",
        )
        .bind(tenant)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(ct.is_some() && nonce.is_some());
    assert!(plaintext.is_none());

    let conn = rls_conn(&pool, tenant).await;
    let (status, get_body) = parts(
        get_llm_settings(
            Path(tenant),
            Extension(TenantContext(tenant)),
            Extension(auth_user(owner)),
            Extension(conn),
            Extension(test_llm_runtime(Some(ENC_KEY))),
        )
        .await,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(get_body["configured"], true);
    assert_eq!(get_body["has_api_key"], true);
    let masked = get_body["api_key_masked"].as_str().unwrap();
    assert!(masked.contains("789"));
    assert!(!masked.contains("abc123"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn put_without_encryption_key_returns_error(pool: PgPool) {
    let owner = create_user(&pool, "owner@t68k.com").await;
    let tenant = insert_tenant(&pool, "T68 No Key").await;
    add_member(&pool, tenant, owner, "owner").await;

    let result = put_llm_settings(
        Path(tenant),
        Extension(TenantContext(tenant)),
        Extension(auth_user(owner)),
        Extension(rls_conn(&pool, tenant).await),
        Extension(test_llm_runtime(None)),
        Json(PutLlmSettingsBody {
            provider: "openai".into(),
            model: "text-embedding-3-small".into(),
            base_url: None,
            api_key: Some("sk-test-key".into()),
            dimensions: None,
            enabled: None,
            llm_model: None,
            llm_base_url: None,
        }),
    )
    .await;

    assert!(result.is_err());
}

#[sqlx::test(migrations = "../../migrations")]
async fn rls_isolates_llm_settings(pool: PgPool) {
    let owner_a = create_user(&pool, "owner-a@t68r.com").await;
    let tenant_a = insert_tenant(&pool, "Tenant A LLM").await;
    add_member(&pool, tenant_a, owner_a, "owner").await;

    let owner_b = create_user(&pool, "owner-b@t68r.com").await;
    let tenant_b = insert_tenant(&pool, "Tenant B LLM").await;
    add_member(&pool, tenant_b, owner_b, "owner").await;

    parts(
        put_llm_settings(
            Path(tenant_a),
            Extension(TenantContext(tenant_a)),
            Extension(auth_user(owner_a)),
            Extension(rls_conn(&pool, tenant_a).await),
            Extension(test_llm_runtime(Some(ENC_KEY))),
            Json(PutLlmSettingsBody {
                provider: "openai".into(),
                model: "text-embedding-3-small".into(),
                base_url: None,
                api_key: Some("sk-tenant-a-only".into()),
                dimensions: None,
                enabled: None,
                llm_model: None,
                llm_base_url: None,
            }),
        )
        .await,
    )
    .await;

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM tenant_llm_config WHERE tenant_id = $1",
    )
    .bind(tenant_b)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 0, "tenant B must not see tenant A config via superuser without filter");

    let (status, body) = parts(
        get_llm_settings(
            Path(tenant_b),
            Extension(TenantContext(tenant_b)),
            Extension(auth_user(owner_b)),
            Extension(rls_conn(&pool, tenant_b).await),
            Extension(test_llm_runtime(Some(ENC_KEY))),
        )
        .await,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["configured"], false);
}
