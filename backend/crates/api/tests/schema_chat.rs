//! Schema verification tests for T22 (chat_sessions + chat_messages).

use sqlx::PgPool;

#[sqlx::test(migrations = "../../migrations")]
async fn chat_sessions_table_exists_with_expected_columns(pool: PgPool) {
    let cols: Vec<(String,)> = sqlx::query_as(
        "SELECT column_name FROM information_schema.columns
         WHERE table_name = 'chat_sessions' ORDER BY column_name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let names: Vec<String> = cols.into_iter().map(|c| c.0).collect();
    for required in [
        "id",
        "tenant_id",
        "workspace_id",
        "user_id",
        "title",
        "model",
        "created_at",
        "updated_at",
    ] {
        assert!(
            names.contains(&required.to_string()),
            "chat_sessions missing column '{required}'"
        );
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn chat_messages_table_exists_with_expected_columns(pool: PgPool) {
    let cols: Vec<(String,)> = sqlx::query_as(
        "SELECT column_name FROM information_schema.columns
         WHERE table_name = 'chat_messages' ORDER BY column_name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let names: Vec<String> = cols.into_iter().map(|c| c.0).collect();
    for required in [
        "id",
        "tenant_id",
        "session_id",
        "role",
        "content",
        "token_count",
        "created_at",
    ] {
        assert!(
            names.contains(&required.to_string()),
            "chat_messages missing column '{required}'"
        );
    }

    // FK cascade: deleting a session must cascade to its messages.
    // Functional test — more robust than checking pg_constraint internals.
    let tenant: (uuid::Uuid,) =
        sqlx::query_as("INSERT INTO tenants (name) VALUES ('t') RETURNING id")
            .fetch_one(&pool)
            .await
            .unwrap();
    let user: (uuid::Uuid,) =
        sqlx::query_as("INSERT INTO users (email) VALUES ('u@t') RETURNING id")
            .fetch_one(&pool)
            .await
            .unwrap();
    let session: (uuid::Uuid,) = sqlx::query_as(
        "INSERT INTO chat_sessions (tenant_id, user_id) VALUES ($1, $2) RETURNING id",
    )
    .bind(tenant.0)
    .bind(user.0)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chat_messages (tenant_id, session_id, role, content) VALUES ($1, $2, 'user', 'hi')")
        .bind(tenant.0)
        .bind(session.0)
        .execute(&pool)
        .await
        .unwrap();

    // Delete the session → message must be cascade-deleted.
    sqlx::query("DELETE FROM chat_sessions WHERE id = $1")
        .bind(session.0)
        .execute(&pool)
        .await
        .unwrap();

    let remaining: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM chat_messages WHERE session_id = $1")
            .bind(session.0)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        remaining, 0,
        "chat_messages must cascade-delete with session"
    );
}
