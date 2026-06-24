# T84D Refactor Plan — GMRAG 2.0 Backend

**Status:** Ready to execute
**Mode:** Build
**Date:** 2026-06-23

Source audit documents (read in full before planning):
`docs/Pre/RAG_DATAFLOW.md`, `RAG_FIX_PLAN.md`, `RAG_SCALABILITY_AUDIT.md`,
`RAG_SECURITY_AUDIT.md`, `RAG_SYSTEM_AUDIT.md`.

---

## Architecture Decisions (confirmed by user)

| Decision | Choice | Rationale |
|---|---|---|
| Race fix (P0) | **Outbox table + worker relay → Redis** | RLS middleware owns BEGIN/COMMIT, so the handler cannot LPUSH after commit; the outbox row rides inside the tx (atomic with documents/ingest_jobs), and a worker relay drains it post-COMMIT. Preserves the fast Redis BRPOP queue. |
| OCR path (P1) | **Feature-flagged `PdfiumRenderer` behind `ocr-pdfium` (off by default)** | Avoids baking native libpdfium into the Docker image on day 1. Per-page text extraction is always on (powers page metadata); OCR gated by `GMRAG_OCR_ENABLED`. Docker build stays intact. |
| Graph provenance (P0) | **Join table `graph_node_documents`** | Graph nodes are deduped per `(tenant, workspace, label, kind)` and shared across documents, so a single `document_id` column doesn't fit. A node is ACL-visible iff ANY source document is in the caller's `accessible_document_ids`. |
| Sweeper pool (P1) | **`admin_pool` with audit note** | Sweeping stuck jobs is a cross-tenant maintenance op, not business logic. Documented as an explicit exception to the app-pool invariant. |

---

## Phase 1 — P0 Ingest Race & P1 Resiliency

### 1.1 Race fix: Outbox pattern
- **Migration** `20260623100000_ingest_outbox.sql`:
  - `CREATE TABLE ingest_outbox (id uuid pk, tenant_id uuid, document_id uuid, payload jsonb, status text default 'pending', created_at timestamptz default now(), dispatched_at timestamptz null)`.
  - `CREATE INDEX idx_ingest_outbox_status_created ON ingest_outbox (status, created_at)`.
  - RLS: `ENABLE/FORCE ROW LEVEL SECURITY` + policy `ingest_outbox_isolation USING (tenant_id = gmrag_current_tenant())` (T25-style). `GRANT SELECT, INSERT, UPDATE, DELETE ON ingest_outbox TO gmrag_app`.
- **`api/src/routes/documents.rs::upload_document`**: replace the `enqueuer.enqueue(&payload)` block with an `INSERT INTO ingest_outbox (id, tenant_id, document_id, payload)` (payload = `IngestJobPayload` as JSONB). Remove the S3-rollback-on-enqueue-failure path (the outbox insert is in-tx, cannot fail independently of COMMIT). 201 response shape unchanged.
- **`worker/src/lib.rs`**: spawn a relay task alongside the BRPOP loop: every `GMRAG_OUTBOX_POLL_INTERVAL_SECS` (default 3s) it runs `relay_outbox_once(admin_pool, redis)`:
  - `SELECT id, tenant_id, payload FROM ingest_outbox WHERE status='pending' ORDER BY created_at LIMIT N FOR UPDATE SKIP LOCKED` inside an admin tx.
  - For each row: `redis.LPUSH(INGEST_JOBS_KEY, payload)`, then `UPDATE ingest_outbox SET status='dispatched', dispatched_at=now() WHERE id=$1`.
  - `COMMIT`. `FOR UPDATE SKIP LOCKED` makes multi-worker replicas safe.
- Keep `JobEnqueuer`/`RedisEnqueuer` types for tests; add a `relay_outbox_once` unit test using `MockQueue` to assert payloads are LPUSHed and rows flipped to `dispatched`.

### 1.2 Job recovery sweeper (P1)
- **Migration** `20260623101000_ingest_jobs_claim.sql`:
  - `ALTER TABLE ingest_jobs ADD COLUMN claimed_at TIMESTAMPTZ NULL`.
  - `CREATE INDEX idx_ingest_jobs_claim ON ingest_jobs (claimed_at) WHERE status='processing'`.
- **`worker/src/job.rs`**: in `process_job_with_retry`, `UPDATE ingest_jobs SET ... claimed_at = now()` alongside the `processing` status update.
- **New `worker/src/sweeper.rs`**: `requeue_stuck_jobs(admin_pool, redis, lease=15min)`:
  - `SELECT j.id, j.document_id, j.tenant_id FROM ingest_jobs j WHERE j.status IN ('pending','processing') AND (j.claimed_at IS NULL OR j.claimed_at < now() - $lease)`.
  - For each row: re-fetch the document row (s3_key, workspace_id, owner_id, visibility, title) from `documents` (tenant_id-scoped query), build an `IngestJobPayload { id: j.id, attempts: j.attempts+1, ... }`, `redis.LPUSH(...)`, then `UPDATE ingest_jobs SET status='pending', claimed_at=NULL, attempts=$attempts+1 WHERE id=$1`.
- **`worker/src/lib.rs`**: run the sweeper every `GMRAG_SWEEP_INTERVAL_SECS` (default 60s) inside the `tokio::select!` loop.
- Comment the `admin_pool` usage as an explicit invariant exception (sweeper is platform maintenance, not business logic).

### 1.3 OCR + per-page text path (also feeds Phase 3 page metadata)
- **`worker/src/pdf_parser.rs`**: add `pub struct PdfiumRenderer { ... }` gated by `#[cfg(feature = "ocr-pdfium")]` (calls `pdfium-render`). Add `pdfium-render` to `worker/Cargo.toml` under an optional feature `ocr-pdfium` (off by default).
- **`worker/src/job.rs::process_job`**: replace `let parsed = parse_pdf(...)` + `vec![parsed.text]` with a new `parse_pdf_for_ingest` dispatcher:
  - Always extract **per-page** text via the existing `extract_pages_blocking` (already a project dep: lopdf + pdf_extract) → returns `Vec<(page_number, text)>`.
  - If `cfg.ocr_enabled` is false OR the `ocr-pdfium` feature is off → join the per-page texts, set `ExtractionMethod` accordingly.
  - If enabled → use `parse_pdf_with_ocr` with `PdfiumRenderer` + `OllamaVisionOcr::new(&cfg.ollama)`, returning per-page text including OCR text appended to scanned pages.
  - Return `Vec<{page_min, page_max, text}>` (page-aware) for chunking (consumed in Phase 3).
- **`core/src/config.rs`**: add `pub ocr_enabled: bool` from `optional_env("GMRAG_OCR_ENABLED", "false")` (parse "true"/"1"). Re-exported to the worker via `Config`.
- **`infra/docker-compose.yml` + `.env.example`**: add `GMRAG_OCR_ENABLED=false`, `GMRAG_OUTBOX_POLL_INTERVAL_SECS=3`, `GMRAG_SWEEP_INTERVAL_SECS=60`. The `ocr-pdfium` Cargo feature stays off so the Docker build is identical to today.

---

## Phase 2 — P0 Security & Data Teardown

### 2.1 Graph ACL leak (SEC-1)
- **Migration** `20260623102000_graph_node_documents.sql`:
  - `CREATE TABLE graph_node_documents (node_id uuid REFERENCES graph_nodes(id) ON DELETE CASCADE, document_id uuid REFERENCES documents(id) ON DELETE CASCADE, tenant_id uuid NOT NULL, created_at timestamptz NOT NULL DEFAULT now(), PRIMARY KEY (node_id, document_id))`.
  - `CREATE INDEX idx_graph_node_documents_doc  ON graph_node_documents (document_id)`.
  - `CREATE INDEX idx_graph_node_documents_node ON graph_node_documents (node_id)`.
  - RLS: `ENABLE/FORCE ROW LEVEL SECURITY` + policy `graph_node_documents_isolation USING (tenant_id = gmrag_current_tenant())`. `GRANT SELECT, INSERT, UPDATE, DELETE ON graph_node_documents TO gmrag_app`.
- **`worker/src/graph.rs`**: `GraphExtraction`/`ExtractedNode` unchanged; provenance is recorded in `dual_write_ingestion`.
- **`worker/src/qdrant_writer.rs`**: after each `graph_nodes` upsert (which returns `node_id` via `RETURNING id`), `INSERT INTO graph_node_documents (node_id, document_id, tenant_id) VALUES (...) ON CONFLICT (node_id, document_id) DO NOTHING`. The provenance inserts run in the same Postgres tx as the node upserts (graph dedup semantics unchanged).
- **`api/src/chat/retrieval.rs::retrieve_graph_context`**:
  - Accept `accessible_document_ids: &[Uuid]` param; after kNN + hydration, **post-filter** hydrated nodes: keep a node iff `EXISTS (SELECT 1 FROM graph_node_documents gnd WHERE gnd.node_id = $node_id AND gnd.document_id = ANY($accessible))`.
  - Same filter applied to the ILIKE fallback path.
  - Drop edges whose src or dst node was filtered out.
  - Thread `accessible` through `retrieve_all_with_metering` / `retrieve_all_with_provider` / `retrieve_all` (compute `accessible_document_ids` once before the graph call — the chunk path already does it).
- **`api/src/routes/graph.rs::get_workspace_graph`**: filter returned nodes by `accessible_document_ids` (nodes whose provenance set intersects the accessible set), and edges whose both endpoints survive. Reuses `accessible_document_ids` from `retrieval.rs`.
- **New regression test** (`api/src/chat/retrieval.rs` tests, sqlx::test, no Qdrant needed): a graph node extracted only from a private document is NOT returned to a non-member; it IS returned to a user holding a `viewer` grant on that document.

### 2.2 Tenant teardown (SEC-4)
- **`api/src/routes/tenants.rs::delete_tenant`**: after `require_owner`, before the Postgres `DELETE FROM tenants`:
  1. `qdrant.teardown_tenant_collections(tid)` — inject `Extension<QdrantStore>` (already added globally in `lib.rs`) and `Extension<Arc<dyn ObjectStore>>` (also already global). `warn!`-log on failure, never block the cascade delete.
  2. S3 prefix delete: add `delete_prefix(prefix)` to `api/src/storage.rs::ObjectStore` trait (default impl returns `Err("not implemented")` so the mock stays trivial) + `S3ObjectStore` impl using `list_objects_v2` paginated + `delete_objects` in batches of 1000; prefix = `format!("{tid}/")`. Best-effort, `warn!`-logged on failure.
- No router change (the `QdrantStore` + `Arc<dyn ObjectStore>` layers are already installed globally in `lib.rs`, so `Extension<...>` extractors pick them up in `delete_tenant`).

---

## Phase 3 — Unblock T85 Frontend (P1 UX)

### 3.1 Page metadata
- **Migration** `20260623103000_document_chunks_pages.sql`:
  - `ALTER TABLE document_chunks ADD COLUMN page_start INT NULL, ADD COLUMN page_end INT NULL`.
  - `CREATE INDEX idx_document_chunks_point ON document_chunks (qdrant_point_id)` (also fixes the P2 retrieval-hydration gap from the scalability audit).
- **`worker/src/chunking.rs`**: change `chunk_page_texts` to return `Vec<Chunk>` where `pub struct Chunk { pub text: String, pub page_start: i32, pub page_end: i32 }`. Each input page carries a 1-based `page_number`; the splitter yields spans of the joined text; map each emitted chunk's char range back to the page-ranges index to compute `page_start`/`page_end` (implementation: track per-page substring offsets in the joined buffer; for each chunk, find the min/max page whose substring overlaps the chunk's text range). Update existing chunking unit tests to the new signature.
- **`worker/src/job.rs`**: build `DualWriteInput.chunks: &[Chunk]` instead of `&[String]`; pass `page_start`/`page_end`.
- **`worker/src/qdrant_writer.rs`**: insert `page_start`/`page_end` into the `document_chunks` row and add them to the Qdrant chunk payload (Qdrant is schemaless — no Qdrant-schema change, but I will add keyword payload indexes for `page_start`/`page_end` is **not** required for T85, so skipped to keep the collection idempotency logic stable).
- **`api/src/chat/retrieval.rs`**: hydrate `page_start`/`page_end` from `document_chunks` into `ChunkHit`; add `pub page_start: Option<i32>, pub page_end: Option<i32>` to `ChunkHit`.
- **`api/src/chat/mod.rs::ResolvedCitation` + `ChatSsePayload::Citation`**: add `page_start: Option<i32>`, `page_end: Option<i32>` (`#[serde(skip_serializing_if = "Option::is_none")]`). This is the citation SSE payload — fulfills the Phase 3 "Expose page field in ResolvedCitation SSE" requirement.
- **`api/src/openapi/schemas.rs::ChatSseEvent::Citation`**: add the two nullable page fields to mirror `ChatSsePayload::Citation`.

### 3.2 Chat history messages endpoint
- **`api/src/routes/chat.rs`**: add `list_messages` handler — `GET /tenants/{tid}/chat_sessions/{sid}/messages`, viewer-gated via `authorize_chat_session`, returns `chat_messages` ordered by `created_at ASC`.
- **`lib.rs`**: register `.route("/tenants/:tid/chat_sessions/:sid/messages", get(routes::chat::list_messages))` inside the `tenant_scoped` router.
- **`api/src/openapi/schemas.rs`**: add `ChatMessageItem { id: Uuid, role: String, content: String, token_count: Option<i32>, created_at: DateTime<Utc> }` and `ChatMessagesResponse { messages: Vec<ChatMessageItem> }`.

### 3.3 LLM context history
- **`api/src/chat/streaming.rs::stream_rag_response`**: add `history: &[ChatMessage]` param; prepend history between the system message and the current user message in the `messages` array passed to `provider.chat_stream`.
- **`api/src/routes/chat.rs::post_chat_sse` / `post_chat_sse_with_context_inner`**: in Phase A, after inserting the user message, load the last `GMRAG_CHAT_HISTORY_LIMIT` (default 10) messages for the session (`SELECT role, content FROM chat_messages WHERE session_id=$1 ORDER BY created_at DESC LIMIT $2` then reverse) and thread them into `stream_rag_response`.
- **`core/src/config.rs`**: add `chat_history_limit: usize` from `optional_env("GMRAG_CHAT_HISTORY_LIMIT", "10")`. Thread through `LlmRuntime`.

---

## Phase 4 — Scalability & Graph API (P1)

### 4.1 Configurable Postgres pool
- **`core/src/db.rs`**: `init_pool` / `init_app_pool` read `DATABASE_MAX_CONNECTIONS` from the process env internally (default 10). Existing call sites `init_pool(&cfg.database_url)` / `init_app_pool(&cfg.database_url)` stay unchanged (no signature change → no caller churn).
- **`core/src/config.rs`**: add `pub database_max_connections: u32` from `optional_env("DATABASE_MAX_CONNECTIONS", "10")` so the value is explicit and testable (config test asserts parse).
- **`infra/docker-compose.yml` + `.env.example`**: add `DATABASE_MAX_CONNECTIONS=10` to both `backend` and `worker` services.

### 4.2 Graph API cursor pagination
- **`api/src/routes/graph.rs::get_workspace_graph`**: accept query `?cursor=<iso8601:uuid>&limit=200` (cursor encodes `created_at:uuid`).
  - Query nodes: `WHERE workspace_id=$1 AND (created_at, id) > ($cursor_ts, $cursor_id) ORDER BY created_at, id LIMIT $limit+1` (or `LIMIT $limit` when no cursor).
  - If `$limit+1` rows returned, set `next_cursor` from the last row; else `next_cursor = null`.
  - Edges loaded for the returned node set only.
  - ACL filter (Phase 2.1) applied to the returned nodes; edges dropped if either endpoint is filtered out.
- **`lib.rs`**: route unchanged (query params are handled in the handler).
- **`api/src/openapi/schemas.rs`**: `WorkspaceGraphResponse` → `{ nodes: Vec<GraphNodeItem>, edges: Vec<GraphEdgeItem>, next_cursor: Option<String> }`.
- Backward compat: when `cursor` omitted, returns the first page — existing clients unbroken since `nodes`/`edges` keys remain.

---

## Phase 5 — Verification & Deliverable (MANDATORY)

1. **Migrations check:** list `backend/migrations/`; confirm the 4 new migrations sort after `20260622000000_rebac_relation_tuples.sql` and introduce no index/sequence/RLS-policy name collisions. New index names: `idx_ingest_outbox_status_created`, `idx_ingest_jobs_claim`, `idx_graph_node_documents_doc`, `idx_graph_node_documents_node`, `idx_document_chunks_point`. New RLS policies: `ingest_outbox_isolation`, `graph_node_documents_isolation`. Verified via `grep` of existing migration files.
2. **Tests:** run `cargo test` (workspace) after each phase; fix breakage. Expected affected tests: `worker/src/chunking.rs` unit tests (signature change), `worker/tests/qdrant_writer.rs` integration test (new `page_start`/`page_end` columns), `api/src/chat/retrieval.rs` tests (new `ChunkHit` fields, new `accessible` param to `retrieve_graph_context`), `api/src/chat/streaming.rs` tests (new `history` param to `stream_rag_response`), `worker/tests/process_job_retry.rs` (OCR/page path). All SQL is runtime sqlx (`sqlx::query` / `query_as::<_, T>`), so no `.sqlx` offline-data regeneration is needed; `SQLX_OFFLINE=true` Docker build stays valid.
3. **Docker:** run `docker compose -f infra/docker-compose.yml config` to confirm env interpolation; new env vars (`GMRAG_OCR_ENABLED`, `DATABASE_MAX_CONNECTIONS`, `GMRAG_OUTBOX_POLL_INTERVAL_SECS`, `GMRAG_SWEEP_INTERVAL_SECS`, `GMRAG_CHAT_HISTORY_LIMIT`) added to both `backend` and `worker` services with safe defaults. Run `cargo build --release --bin gmrag-api --bin gmrag-worker` (features off) to confirm the Dockerfile's build step still succeeds without `pdfium-render`.
4. **Git:** stage all modified files; create a semantic commit: `fix: resolve P0 and P1 RAG backend issues for T84D`. (Only commit when the user explicitly approves — the default is to stage + show `git status`.)
5. **Report:** write `T84D_REFACTOR_REPORT.md` at the repo root with the required sections:
   - `## What was fixed` — detail the technical solutions for Phases 1–4.
   - `## Modified Files` — bulleted list.
   - `## Database Migrations` — details of the 4 new migrations + zero-conflict confirmation.
   - `## Testing & Docker` — exact commands run (`cargo test`, `docker compose config`, `cargo build --release ...`) and results.
   - `## Version Control` — git commit hash + message.
   - `## Remaining/Known Issues` — the leftover P2 list verbatim from `RAG_FIX_PLAN.md` (token_count never written, assistant message strips citation tags, no chunk score threshold, deep health `/healthz`, rate limiting, graph GC on document delete, upload content-hash dedup, graph LLM input truncation, MIME validation, ILIKE trigram index, `usage_events` partitioning, invitation accept flow, etc.).

---

## Modified Files (planned)

### Migrations (4 new)
- `backend/migrations/20260623100000_ingest_outbox.sql`
- `backend/migrations/20260623101000_ingest_jobs_claim.sql`
- `backend/migrations/20260623102000_graph_node_documents.sql`
- `backend/migrations/20260623103000_document_chunks_pages.sql`

### API crate
- `backend/crates/api/src/routes/documents.rs` — outbox insert replaces Redis enqueue
- `backend/crates/api/src/routes/tenants.rs` — teardown Qdrant + S3 prefix delete
- `backend/crates/api/src/routes/chat.rs` — new `list_messages` handler; chat history into `stream_rag_response`
- `backend/crates/api/src/routes/graph.rs` — cursor pagination + graph ACL filter
- `backend/crates/api/src/chat/retrieval.rs` — `accessible` param + graph ACL post-filter; `ChunkHit` page fields
- `backend/crates/api/src/chat/mod.rs` — `ResolvedCitation` + `ChatSsePayload` page fields
- `backend/crates/api/src/chat/streaming.rs` — `history` param in `stream_rag_response`
- `backend/crates/api/src/lib.rs` — register `messages` route
- `backend/crates/api/src/storage.rs` — `delete_prefix` trait method + S3 impl
- `backend/crates/api/src/openapi/schemas.rs` — page fields, `ChatMessageItem`, `ChatMessagesResponse`, `next_cursor`

### Core crate
- `backend/crates/core/src/config.rs` — `ocr_enabled`, `database_max_connections`, `chat_history_limit`
- `backend/crates/core/src/db.rs` — env-driven `max_connections`

### Worker crate
- `backend/crates/worker/src/lib.rs` — relay task + sweeper task spawn
- `backend/crates/worker/src/job.rs` — per-page parse path + `claimed_at`
- `backend/crates/worker/src/chunking.rs` — `Chunk` struct with `page_start`/`page_end`
- `backend/crates/worker/src/pdf_parser.rs` — feature-gated `PdfiumRenderer`
- `backend/crates/worker/src/qdrant_writer.rs` — `graph_node_documents` inserts; page fields in chunk upsert
- `backend/crates/worker/src/sweeper.rs` (new) — `requeue_stuck_jobs`
- `backend/crates/worker/src/relay.rs` (new) — `relay_outbox_once`
- `backend/crates/worker/Cargo.toml` — optional `ocr-pdfium` feature

### Infra / config
- `infra/docker-compose.yml` — new env vars for `backend` + `worker`
- `.env.example` — new env var documentation

### Report
- `T84D_REFACTOR_REPORT.md` (repo root)

---

## Risks / Open Notes

- sqlx integration tests that hit live Qdrant/MinIO will be skipped if those services aren't local (existing behavior; the new graph ACL regression test is pure SQL + needs no Qdrant).
- The per-page → chunk mapping in `chunking.rs` is the trickiest correctness change; it will be covered by a dedicated unit test asserting page boundaries for chunks that span page breaks.
- The `ocr-pdfium` Cargo feature is off by default; once a production image with libpdfium is available, flip the feature on in `Cargo.toml` and `Dockerfile`, then set `GMRAG_OCR_ENABLED=true`.
- `delete_prefix` for S3 uses `list_objects_v2` + `delete_objects`; operators should ensure MinIO/s3 lifecycle policies don't race this on tenant delete (best-effort by design).
- The sweeper's `admin_pool` usage is a documented exception to the project invariant "worker uses app_pool for business logic". The sweeper re-enqueues jobs platform-wide (not tenant business data) and is the only sanctioned admin_pool path in the worker.

---

## Execution Order

Phase 1 (migrations + worker relay + sweeper + OCR plumbing) → `cargo check` / `cargo test -p gmrag-worker`.
Phase 2 (graph provenance + tenant teardown) → `cargo check` / `cargo test -p gmrag-api`.
Phase 3 (page metadata + messages endpoint + chat history) → `cargo check` / `cargo test`.
Phase 4 (pool env + graph pagination) → `cargo check` / `cargo test`.
Phase 5 (`docker compose config`, `cargo build --release`, `git status`, commit, write report).