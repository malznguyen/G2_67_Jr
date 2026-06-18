//! Ingest job model + processing stub.
//!
//! T34: `IngestJob` is the wire format for jobs pushed to the Redis list
//! `gmrag:ingest_jobs` by the API. `process_job` is a stub returning `Ok(())`
//! — the real ingestion pipeline (S3 download → PDF parse → chunk → embed →
//! Qdrant upsert → Postgres dual-write) lands in T37+.

use anyhow::Result;

/// A single ingestion job dequeued from Redis (`gmrag:ingest_jobs`).
///
/// Serialized as JSON. Every field is required — the API enqueuer must
/// populate all of them before `LPUSH`.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct IngestJob {
    pub id: uuid::Uuid,
    pub tenant_id: uuid::Uuid,
    pub workspace_id: uuid::Uuid,
    pub document_id: uuid::Uuid,
    pub s3_key: String,
    pub filename: String,
    pub attempts: u32,
}

/// Process a single ingest job.
///
/// **Stub** — returns `Ok(())` immediately. The real implementation
/// (T37+) must:
/// 1. Download the document from S3 (`s3_key`).
/// 2. Parse PDF → text.
/// 3. Chunk text → embed via Ollama.
/// 4. Upsert vectors to Qdrant (`upsert_chunks`).
/// 5. Dual-write metadata to Postgres via **`app_pool`** with
///    `SET LOCAL app.tenant_id = $job.tenant_id` (NOT `admin_pool`).
pub async fn process_job(_job: &IngestJob) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingest_job_deserializes_from_json() {
        let json = r#"{
            "id": "550e8400-e29b-41d4-a716-446655440000",
            "tenant_id": "660e8400-e29b-41d4-a716-446655440000",
            "workspace_id": "770e8400-e29b-41d4-a716-446655440000",
            "document_id": "880e8400-e29b-41d4-a716-446655440000",
            "s3_key": "tenant-66/document-88/report.pdf",
            "filename": "report.pdf",
            "attempts": 0
        }"#;
        let job: IngestJob = serde_json::from_str(json).expect("deserialize");
        assert_eq!(job.attempts, 0);
        assert_eq!(job.filename, "report.pdf");
        assert_eq!(job.s3_key, "tenant-66/document-88/report.pdf");
    }

    #[test]
    fn ingest_job_roundtrips_serialize_deserialize() {
        let job = IngestJob {
            id: uuid::Uuid::new_v4(),
            tenant_id: uuid::Uuid::new_v4(),
            workspace_id: uuid::Uuid::new_v4(),
            document_id: uuid::Uuid::new_v4(),
            s3_key: "uploads/doc.pdf".into(),
            filename: "doc.pdf".into(),
            attempts: 3,
        };
        let json = serde_json::to_string(&job).expect("serialize");
        let back: IngestJob = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.id, job.id);
        assert_eq!(back.tenant_id, job.tenant_id);
        assert_eq!(back.attempts, 3);
    }

    #[tokio::test]
    async fn process_job_stub_returns_ok() {
        let job = IngestJob {
            id: uuid::Uuid::new_v4(),
            tenant_id: uuid::Uuid::new_v4(),
            workspace_id: uuid::Uuid::new_v4(),
            document_id: uuid::Uuid::new_v4(),
            s3_key: "k".into(),
            filename: "f.pdf".into(),
            attempts: 0,
        };
        process_job(&job).await.expect("stub must return Ok");
    }
}
