//! Schema verification tests for T21 (graph_nodes + graph_edges).

use sqlx::PgPool;

#[sqlx::test(migrations = "../../migrations")]
async fn graph_nodes_table_exists_with_expected_columns(pool: PgPool) {
    let cols: Vec<(String,)> = sqlx::query_as(
        "SELECT column_name FROM information_schema.columns
         WHERE table_name = 'graph_nodes' ORDER BY column_name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let names: Vec<String> = cols.into_iter().map(|c| c.0).collect();
    for required in [
        "id",
        "tenant_id",
        "kind",
        "label",
        "properties",
        "created_at",
    ] {
        assert!(
            names.contains(&required.to_string()),
            "graph_nodes missing column '{required}'"
        );
    }

    // properties must be JSONB
    let row: (String,) = sqlx::query_as(
        "SELECT data_type FROM information_schema.columns
         WHERE table_name = 'graph_nodes' AND column_name = 'properties'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.0, "jsonb", "graph_nodes.properties must be JSONB");
}

#[sqlx::test(migrations = "../../migrations")]
async fn graph_edges_table_exists_with_expected_columns(pool: PgPool) {
    let cols: Vec<(String,)> = sqlx::query_as(
        "SELECT column_name FROM information_schema.columns
         WHERE table_name = 'graph_edges' ORDER BY column_name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let names: Vec<String> = cols.into_iter().map(|c| c.0).collect();
    for required in [
        "id",
        "tenant_id",
        "src_node_id",
        "dst_node_id",
        "kind",
        "weight",
        "properties",
        "created_at",
    ] {
        assert!(
            names.contains(&required.to_string()),
            "graph_edges missing column '{required}'"
        );
    }

    // unique constraint on (src_node_id, dst_node_id, kind)
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pg_indexes
         WHERE tablename = 'graph_edges'
           AND indexdef LIKE '%src_node_id, dst_node_id, kind%'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "graph_edges must have UNIQUE(src, dst, kind)");
}
