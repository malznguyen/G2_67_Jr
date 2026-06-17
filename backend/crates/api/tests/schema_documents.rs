//! Schema verification tests for T20 (documents + document_chunks).
//!
//! Confirms the tables exist with the expected columns and that the
//! qdrant_point_id column is present (vectors live in Qdrant, PG holds
//! only the point reference). Uses `#[sqlx::test]` for an isolated DB.

use sqlx::PgPool;

#[sqlx::test(migrations = "../../migrations")]
async fn documents_table_exists_with_expected_columns(pool: PgPool) {
    // documents core columns
    let cols: Vec<(String,)> = sqlx::query_as(
        "SELECT column_name FROM information_schema.columns
         WHERE table_name = 'documents' ORDER BY column_name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let names: Vec<String> = cols.into_iter().map(|c| c.0).collect();
    for required in [
        "id", "tenant_id", "workspace_id", "owner_id", "title", "status",
        "visibility", "share_token", "mime_type", "byte_size", "s3_key",
        "created_at", "updated_at",
    ] {
        assert!(
            names.contains(&required.to_string()),
            "documents missing column '{required}'"
        );
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn document_chunks_table_has_qdrant_point_id(pool: PgPool) {
    let row: (String,) = sqlx::query_as(
        "SELECT data_type FROM information_schema.columns
         WHERE table_name = 'document_chunks' AND column_name = 'qdrant_point_id'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.0, "uuid", "qdrant_point_id must be UUID type");

    // document_chunks core columns
    let cols: Vec<(String,)> = sqlx::query_as(
        "SELECT column_name FROM information_schema.columns
         WHERE table_name = 'document_chunks' ORDER BY column_name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let names: Vec<String> = cols.into_iter().map(|c| c.0).collect();
    for required in [
        "id", "tenant_id", "document_id", "chunk_index", "content",
        "qdrant_point_id", "token_count", "created_at",
    ] {
        assert!(
            names.contains(&required.to_string()),
            "document_chunks missing column '{required}'"
        );
    }
}
