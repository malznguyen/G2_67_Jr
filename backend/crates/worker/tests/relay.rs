//! T84D Phase 1.1 — integration test for the outbox relay.
//!
//! Seeds an `ingest_outbox` row + supporting documents/ingest_jobs, runs
//! `relay_outbox_once` against a `MockQueue`, and asserts:
//! - the payload is LPUSHed onto the queue, and
//! - the row is flipped to `status='dispatched'`.

use gmrag_worker::{relay_outbox_once, MockQueue};
use sqlx::PgPool;
use uuid::Uuid;

use chrono::{DateTime, Utc};

async fn seed_outbox_row(pool: &PgPool) -> (Uuid, Uuid, Uuid, Uuid) {
    let tenant = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, $2)")
        .bind(tenant)
        .bind("T84D Relay Tenant")
        .execute(pool)
        .await
        .unwrap();

    let owner = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, email, name) VALUES ($1, $2, $3)")
        .bind(owner)
        .bind(format!("u{owner}@relay.test"))
        .bind("Relay Owner")
        .execute(pool)
        .await
        .unwrap();

    let ws = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workspaces (id, tenant_id, name, slug, created_by) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(ws)
    .bind(tenant)
    .bind("Relay WS")
    .bind(format!("relay-ws-{ws}"))
    .bind(owner)
    .execute(pool)
    .await
    .unwrap();

    let doc = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO documents (id, tenant_id, workspace_id, owner_id, title, status, visibility, s3_key)
           VALUES ($1, $2, $3, $4, 'Relay doc', 'uploaded', 'private', 'k')"#,
    )
    .bind(doc)
    .bind(tenant)
    .bind(ws)
    .bind(owner)
    .execute(pool)
    .await
    .unwrap();

    let job_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO ingest_jobs (id, tenant_id, document_id, status, attempts)
         VALUES ($1, $2, $3, 'pending', 0)",
    )
    .bind(job_id)
    .bind(tenant)
    .bind(doc)
    .execute(pool)
    .await
    .unwrap();

    let payload = serde_json::json!({
        "id": job_id,
        "tenant_id": tenant,
        "workspace_id": ws,
        "document_id": doc,
        "s3_key": "k",
        "filename": "relay.pdf",
        "owner_id": owner,
        "visibility": "private",
        "attempts": 0
    });
    sqlx::query(
        "INSERT INTO ingest_outbox (tenant_id, document_id, payload)
         VALUES ($1, $2, $3)",
    )
    .bind(tenant)
    .bind(doc)
    .bind(payload)
    .execute(pool)
    .await
    .unwrap();

    (tenant, ws, doc, job_id)
}

#[sqlx::test(migrations = "../../migrations")]
async fn relay_drains_outbox_pushes_payload_and_marks_dispatched(pool: PgPool) {
    let (_tenant, _ws, _doc, job_id) = seed_outbox_row(&pool).await;

    let mut queue = MockQueue::new(vec![]);
    let dispatched = relay_outbox_once(&pool, &mut queue)
        .await
        .expect("relay pass");
    assert_eq!(dispatched, 1, "one outbox row must be dispatched per pass");

    // The payload was LPUSHed exactly once.
    let pushed = queue.pushed();
    assert_eq!(pushed.len(), 1, "exactly one LPUSH, got {}", pushed.len());

    let job: gmrag_worker::IngestJob = serde_json::from_slice(&pushed[0]).expect("deserialize");
    assert_eq!(job.id, job_id);
    assert_eq!(job.filename, "relay.pdf");

    // The row is now dispatched, with a non-null dispatched_at.
    let (status, dispatched_at): (String, Option<DateTime<Utc>>) =
        sqlx::query_as("SELECT status, dispatched_at FROM ingest_outbox WHERE document_id = $1")
            .bind(job.document_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "dispatched");
    assert!(dispatched_at.is_some(), "dispatched_at must be set");

    // A second relay pass drains nothing (the row is no longer pending).
    let mut queue2 = MockQueue::new(vec![]);
    let again = relay_outbox_once(&pool, &mut queue2)
        .await
        .expect("second pass");
    assert_eq!(again, 0);
    assert!(queue2.pushed().is_empty());
}
