# RAG System Audit — GMRAG 2.0 (T84D)

**Audit date:** 2026-06-23  
**Scope:** Full RAG pipeline — upload through citations  
**Method:** Read-only code audit; no speculation  
**Target scale:** 10,000 documents · 500 active users · multi-tenant SaaS · GraphRAG · ReBAC  
**Companion docs:** [RAG_DATAFLOW.md](./RAG_DATAFLOW.md) · [RAG_SECURITY_AUDIT.md](./RAG_SECURITY_AUDIT.md) · [RAG_SCALABILITY_AUDIT.md](./RAG_SCALABILITY_AUDIT.md) · [RAG_FIX_PLAN.md](./RAG_FIX_PLAN.md)

---

## Executive Summary

The backend implements a complete ingest → dual-write → retrieval → SSE chat pipeline with tenant-scoped Qdrant collections, workspace payload partitioning, document-level ReBAC on chunk retrieval, and workspace-scoped graph retrieval. Core paths are tested (~256 test functions across api/worker).

**Strengths (code-evidenced):**
- Idempotent dual-write on ingest retry (`ON CONFLICT` upserts + stable Qdrant point IDs)
- Defense-in-depth chunk ACL (SQL compile + Qdrant filter + post-filter)
- PostgreSQL RLS on all tenant tables
- Citation streaming with cross-delta tag parsing

**Critical gaps for production target:**
- No job recovery after worker crash (Redis message consumed, no sweeper)
- Graph data not cleaned on document delete; accumulates orphans
- Graph retrieval has no document-level ACL (workspace-wide exposure)
- No page metadata → PDF citation UX blocked
- OCR/scanned PDF path not wired in production ingest
- Tenant delete leaves S3 + Qdrant orphans
- Default tenant quota (100 documents) blocks 10k-doc target without overrides

---

## Phase 1 — System Architecture

See [RAG_DATAFLOW.md](./RAG_DATAFLOW.md) for diagrams.

| Subsystem | Status | Primary evidence |
|-----------|--------|------------------|
| Upload | Implemented | `api/src/routes/documents.rs` |
| Ingest queue | Redis LPUSH/BRPOP | `api/src/queue.rs`, `worker/src/queue.rs` |
| Chunk | cl100k 1200/100 | `worker/src/chunking.rs` |
| Embed | Ollama + OpenAI BYOK, 768-dim | `worker/src/embedding.rs` |
| Graph extraction | LLM JSON extract | `worker/src/graph.rs` |
| Qdrant | 2 collections/tenant | `core/src/qdrant/store.rs` |
| Retrieval | kNN + ACL + graph fallback | `api/src/chat/retrieval.rs` |
| Chat | SSE 3-phase | `api/src/routes/chat.rs` |
| Citations | `[chunk:N]` → SSE enrichment | `api/src/chat/streaming.rs`, `api/src/chat/mod.rs` |

---

## Phase 2 — Ingest Pipeline

### Upload route

- **Endpoint:** `POST /tenants/{tid}/documents` (`lib.rs` line 142)
- **Required fields:** `file`, `visibility` (`shared`|`private`), `workspace_id`
- **Order:** S3 upload → SAVEPOINT → Postgres inserts → Redis enqueue → RELEASE → middleware COMMIT
- **S3 key:** always `{tid}/{workspace_id}/{document_id}.pdf` regardless of uploaded MIME type
- **Returns:** `201 { "id": "<uuid>" }` only — no job id exposed

### Job creation

| Question | Answer (from code) |
|----------|-------------------|
| One job per document? | **Yes, per upload** — each upload creates one `ingest_jobs` row. No UNIQUE on `document_id`; re-upload of same file creates duplicate jobs/documents. |
| One worker per document? | **One worker instance processes one job at a time** — sequential `poll_once` loop in `worker/src/lib.rs`. Multiple worker replicas can run in parallel (separate BRPOP consumers). |
| Duplicate protection at upload? | **No** — new `Uuid::new_v4()` per upload; no content-hash dedup. |
| Retry idempotency? | **Yes** — `dual_write_ingestion` upserts on `(document_id, chunk_index)` and `(tenant_id, workspace_id, label, kind)` for graph nodes. |

### Worker queue

- Key: `gmrag:ingest_jobs`
- Producer: `LPUSH` (`api/src/queue.rs`)
- Consumer: `BRPOP` timeout 5s (`worker/src/queue.rs`)
- Payload `attempts` field always `0` at enqueue; retry counting uses in-process loop index

### Retry behavior

```
MAX_ATTEMPTS = 3
BACKOFF_BASE_MS = 1000, cap 16000
```

- Retries are **in-process** after single BRPOP — no re-LPUSH, no dead-letter queue
- On exhaustion: `ingest_jobs.status = failed`, `documents.status = failed`
- Between retries: `ingest_jobs.status` stays `processing`
- Qdrant failure rolls back Postgres tx (`qdrant_writer.rs` — Qdrant upsert before commit)

### Status transitions

**documents.status** (`job.rs` comment + code):

```
uploaded → processing → indexed | failed
```

**ingest_jobs.status:**

```
pending → processing → completed | failed
```

Exposure: `documents.status` returned by `GET /tenants/{tid}/documents` — suitable for frontend polling. **No dedicated ingest-job API** — `ingest_jobs.last_error` not exposed to clients.

### Race conditions (evidenced)

1. **Redis before COMMIT:** Enqueue inside RLS transaction; worker may start before commit visible.
2. **Worker crash after BRPOP:** Message removed from Redis; no re-enqueue; DB may show `processing` indefinitely.
3. **No stuck-job sweeper:** No code scans `pending`/`processing` rows to re-queue.

### Stuck jobs

| State | Cause | Recovery in code |
|-------|-------|------------------|
| `pending` + empty Redis | Enqueue succeeded, worker down | **None** |
| `processing` | Worker crash mid-job | **None** |
| `failed` | 3 attempts exhausted | Manual re-upload only; **no reindex API** |

---

## Phase 3 — Chunking

| Parameter | Value | File |
|-----------|-------|------|
| Tokenizer | `cl100k_base` (tiktoken) | `worker/src/chunking.rs` |
| Chunk size | 1200 tokens | same |
| Overlap | 100 tokens | same |
| Splitter | `text_splitter` + `ChunkConfig` | same |

### Metadata persisted

**Postgres `document_chunks`:** `tenant_id`, `document_id`, `chunk_index`, `content`, `qdrant_point_id`  
**Not written at ingest:** `token_count` (column exists, always NULL from worker)

**Qdrant payload:** `workspace_id`, `document_id`, `chunk_index`, `filename`, `owner_id`, `visibility`  
**Not stored:** page number, chunk text (text in Postgres only)

### Production PDF handling

```rust
// worker/src/job.rs — process_job
let page_texts = vec![parsed.text];  // single combined blob
let chunks = chunk_page_texts(&page_texts)?;
```

- `parse_pdf` uses `pdf_extract` full-document extraction (`worker/src/pdf_parser.rs`)
- `page_count` computed but **not persisted**
- Per-page extraction exists in `extract_pages_blocking` / `parse_pdf_with_ocr` but **not used** in `process_job`

### Evaluation

| Criterion | Assessment |
|-----------|------------|
| Chunk quality (text PDFs) | Adequate — 1200-token windows with 100 overlap |
| Large PDFs | Full file loaded into memory; 30s parse timeout; **no streaming** |
| Page preservation | **Not implemented** in production path |
| Citation quality | Index + filename only; **no page anchor** for PDF UX |

---

## Phase 4 — Embeddings

### Providers

| Context | Provider | Model default |
|---------|----------|---------------|
| Ingest (platform) | Ollama | `nomic-embed-text` (`config.rs`) |
| Ingest (BYOK) | OpenAI | `text-embedding-3-small` |
| Chat query embed | Same via `resolve_llm_config` | same |

Selection: `select_embedder` / `resolve_llm_config` reads `tenant_llm_config` under RLS.

### Dimensions

- Fixed **768** everywhere (`EMBED_DIM`)
- OpenAI requests include `dimensions: 768`
- `tenant_llm_config.dimensions` column exists but **is not passed to embedders** (stored only)

### Batching

- `DEFAULT_BATCH_SIZE = 32`, `DEFAULT_CONCURRENCY = 2`
- Retries: 1 retry, 250ms base backoff

### Bottlenecks (qualitative, code-based)

- Sequential ingest steps: parse → chunk → embed chunks → graph LLM → embed nodes → dual-write
- Graph extraction sends **entire document text** to LLM with no truncation in `graph.rs` — behavior for documents exceeding model context: **UNKNOWN** (no truncation code found)

### Vector consistency / migration risks

- All collections require 768-dim cosine; changing model without re-embed would break search quality
- No migration/reindex tooling in codebase
- BYOK dimension override in DB does not affect actual vectors

---

## Phase 5 — GraphRAG

### Extraction

- Input: full parsed PDF text (same blob as chunking source)
- LLM: `{base_url}/chat/completions`, JSON schema prompt (`worker/src/graph.rs`)
- Output: nodes `{kind, label, description}`, edges `{source, target, kind}` (label references)
- Edges with unknown source/target labels: **skipped** (`qdrant_writer.rs` `continue`)

### Storage

| Entity | PostgreSQL | Qdrant |
|--------|------------|--------|
| Nodes | `graph_nodes` — dedup `(tenant_id, workspace_id, label, kind)` | `graph_{tenant_id}` — vector + payload |
| Edges | `graph_edges` — `(src, dst, kind)` unique | **Not stored** |

Node properties: `{"description": "..."}` JSONB. Qdrant graph payload: `node_id`, `workspace_id`, `entity_name` (label).

### Retrieval (chat)

1. kNN on `graph_{tenant_id}` with `workspace_id` filter, `top_k = 5`
2. Drop scores `< 0.25`
3. ILIKE fallback on `label` / `properties->>'description'` if weak/empty
4. Load edges from Postgres for retrieved node IDs

### Graph API

- `GET /tenants/{tid}/workspaces/{wid}/graph` — returns **all** nodes and edges for workspace (member-gated via ReBAC)

### Cleanup

- Document delete: **does not** remove graph nodes/edges or graph Qdrant points (`store.rs` comment T59)
- Nodes shared across documents by design (dedup key)
- Re-ingest updates node properties and Qdrant vector for same label

### Evaluation at 100k nodes / 1M edges

| Concern | Code finding |
|---------|--------------|
| Duplicate nodes | Prevented per `(tenant, workspace, label, kind)` |
| Orphan nodes | Accumulate when documents deleted; no GC |
| Orphan edges | Possible if edge references skipped labels; edges persist after doc delete |
| Graph cleanup | **None** per document; tenant teardown only via `teardown_tenant_collections` (not wired to delete_tenant) |
| Graph ACL | **Workspace scope only** — no document-level filtering |
| Graph API at scale | Full graph load — **no pagination** (`routes/graph.rs`) |

---

## Phase 6 — Qdrant

### Collection strategy

- **Tenant-per-collection:** `chunks_{tenant_id}`, `graph_{tenant_id}`
- Created idempotently on first ingest (`setup_tenant_collections`)
- `QDRANT_COLLECTION_DEFAULT` in config: **unused** in runtime collection naming

### Payload indexes

**Chunks:** `workspace_id`, `document_id`, `chunk_index`, `filename`, `owner_id`, `visibility` (all indexed, `.wait(true)`)

**Graph:** `node_id`, `workspace_id`, `entity_name`

### HNSW

- Qdrant defaults only — no explicit `HnswConfig` in code (`store.rs` comment)

### Search filters

- Chunks: caller-built filter (workspace must + document should/min_should)
- Graph: internal `must workspace_id`

### Scale evaluation

| Scenario | Assessment |
|----------|------------|
| 500 tenants | 1000 collections — feasible for Qdrant; metadata overhead **UNKNOWN** without load test |
| 10k docs in one tenant | ~200k chunk vectors if ~20 chunks/doc (estimate, not measured in code) |
| Payload filter cost | Indexed fields — expected efficient; **no benchmarks in repo** |
| tenant-per-collection vs workspace filter | **Both used:** tenant = collection boundary; workspace = payload filter inside collection |

---

## Phase 7 — Chat

### Trace (confirmed)

```
user_message
  → embed_query (single call, metered)
  → retrieve_chunks_with_vector (top_k=5, ACL)
  → retrieve_graph_context (top_k=5, workspace)
  → assemble_system_prompt
  → chat_stream (system + single user message)
  → parse [chunk:N]
  → enrich_stream_events → SSE
  → persist assistant (text only)
```

### Context size

- Up to 5 chunks × ~1200 tokens + graph section + system instructions
- **No chat history** in LLM messages — multi-turn context not supported at retrieval/generation layer
- Stored messages exist in DB but are not read during chat

### Hallucination risks (architecture-level)

- System prompt: "Answer ONLY from the context below"
- Empty retrieval → prompt includes "No document context was retrieved"
- Graph entities included without document provenance
- No score threshold on chunk retrieval (all top_k returned regardless of similarity)

### Token growth

- Per request: embed query + full context in system prompt + streamed output
- Metered via `usage_events` (`embedding_tokens`, `llm_tokens`)

---

## Phase 8 — Citations

### SSE citation payload (`ResolvedCitation` / OpenAPI)

| Field | Present |
|-------|---------|
| `index` | Yes (1-based) |
| `point_id` | Yes |
| `document_id` | Yes |
| `chunk_index` | Yes |
| `filename` | Yes (from `documents.title`) |
| `page` | **No** |
| `snippet` | **No** (full content only in system prompt, not client-facing) |
| `score` | **No** |

### PDF UX impact

- Frontend cannot jump to PDF page without page metadata (not ingested)
- Preview endpoint returns chunk text (`GET .../documents/{did}/preview`, limit 50 chunks) — usable for debug, not citation-linked

### Parser robustness

- `DeepseekTokenParser` handles tags split across SSE deltas (`streaming.rs`)

### Missing citations

- Unknown index → `CitationUnknown` event
- Model may omit `[chunk:N]` tags — no enforcement beyond prompt instruction
- Persisted assistant messages strip citation tags (`assistant_text_from_events`)

---

## Phase 9 — ReBAC (summary)

Detailed findings: [RAG_SECURITY_AUDIT.md](./RAG_SECURITY_AUDIT.md)

| Area | Finding |
|------|---------|
| ACL model | Zanzibar-style namespaces: document, chat_session, workspace |
| Grants | `resource_acl` — owner/editor/viewer; workspace-group principals |
| Inheritance | Document/chat viewer includes workspace member |
| Chunk retrieval | `accessible_document_ids` + Qdrant filter + post-filter |
| Graph retrieval | Workspace filter only |
| Revoke latency | Immediate — SQL visibility on next query |
| Grant-only users | `accessible_document_ids` includes ACL grants, but `ensure_workspace_member` blocks chat retrieval |

---

## Phase 10 — Scalability (summary)

Detailed analysis: [RAG_SCALABILITY_AUDIT.md](./RAG_SCALABILITY_AUDIT.md)

**Latency estimates:** Quantitative P50/P95/P99 **not possible from code alone** — no benchmarks committed.

Qualitative dominant latencies:
- LLM stream (chat): largest component
- Embed query: Ollama/network bound
- Qdrant kNN: expected sub-100ms at target scale with HNSW defaults (unverified)
- Postgres: per-chunk hydration loop (up to 5 sequential queries per retrieval)

---

## Phase 11 — Observability

| Capability | Present | Evidence |
|------------|---------|----------|
| Structured logging | Partial | `tracing` in worker job, delete warnings |
| Usage metering | Yes | `usage_events` — embedding + LLM tokens |
| Audit log | Yes | ACL grant/revoke writes `audit_log` |
| Prometheus/metrics | **No** | grep: no prometheus exporter |
| Ingest job visibility | Partial | DB only; not exposed via API |
| Retrieval tracing | **No** | No debug endpoint or structured retrieval log |
| Health checks | Partial | `/health` uptime; `/healthz` DB only — **no Qdrant/Redis/S3/Ollama** |

### Operator questions

| Question | Can operator answer today? |
|----------|---------------------------|
| Why did ingest fail? | Partially — `ingest_jobs.last_error` in DB; **no API** |
| Why was answer empty? | **No** — no retrieval debug log/scores exposed |
| Why was citation missing? | **No** — no trace of parser events vs model output |

---

## Phase 12 — Recovery

| Operation | Supported | Gaps |
|-----------|-----------|------|
| Delete document | Yes | Graph orphans; best-effort S3/Qdrant |
| Delete tenant | Postgres cascade only | S3 + Qdrant collections orphaned |
| Reindex / rebuild | **No API** | Would require manual re-upload or custom tooling |
| Orphan vectors | Partial cleanup on doc delete | Failures leave orphans (warn-only) |
| Orphan graph | **No cleanup** | Accumulates |
| Full tenant rebuild | **Partial** | S3 objects remain if known keys; no bulk re-enqueue |

Dual-write idempotency supports **safe retry** of same ingest job if manually re-queued.

---

## Phase 13 — Production Risks (summary)

See [RAG_FIX_PLAN.md](./RAG_FIX_PLAN.md) for full table.

| Severity | Count (identified) | Examples |
|----------|-------------------|----------|
| P0 | 4 | Worker crash job loss; graph ACL gap; tenant delete orphans; Redis-before-commit race |
| P1 | 8 | No OCR; no page citations; grant-only chat blocked; stuck jobs; pool size |
| P2 | 6 | No chat history; no metrics; quota defaults; graph API unpaginated |

---

## Phase 14 — Frontend Impact (T85)

| Finding | T85 impact |
|---------|------------|
| No page in citations | PDF viewer cannot highlight page — design without page jump or block PDF UX |
| `documents.status` for polling | Use list/poll on upload — no job endpoint |
| No messages list API | Chat history UI blocked — SSE-only forward path |
| Citation fields | Build citation card from index + filename + chunk_index; link to preview endpoint |
| Graph workspace-wide | Graph UI shows all workspace entities — no per-document graph filter |
| Grant-only share UX | User with doc grant but not workspace member: preview works, **chat fails** — must document or fix |
| Ingest `failed` state | Show error + re-upload; no retry button (no API) |
| Empty retrieval | Expect answers with "no context" — show empty-state in chat |
| SSE citation format | Use OpenAPI `ChatSseEvent` schema; handle `CitationUnknown` |
| 50 MiB upload limit | Client-side size validation |
| Unpaginated lists | Document/session lists may lag at scale — virtual scroll or accept MVP limit |

---

## Final Verdict

| Gate | Verdict | Rationale |
|------|---------|-----------|
| **RAG READY** | **CONDITIONAL** | Core path works; blocked by OCR gap, job recovery, no reindex, citation metadata |
| **GRAPH READY** | **CONDITIONAL** | Extraction + retrieval work; no cleanup, no doc ACL, unpaginated API |
| **SECURITY READY** | **CONDITIONAL** | RLS + chunk ReBAC solid; graph workspace leak + grant/member inconsistency |
| **SCALABILITY READY** | **CONDITIONAL** | pool=10, sequential worker, default quotas, 1000 collections at 500 tenants |
| **FRONTEND READY** | **CONDITIONAL** | OpenAPI stable (T84A); missing messages API, page citations, ingest error surface |

---

## Evidence Index

| Area | Primary files |
|------|---------------|
| Upload / delete | `api/src/routes/documents.rs` |
| Queue | `api/src/queue.rs`, `worker/src/queue.rs` |
| Ingest | `worker/src/job.rs`, `worker/src/qdrant_writer.rs` |
| Chunk / PDF | `worker/src/chunking.rs`, `worker/src/pdf_parser.rs` |
| Embed | `worker/src/embedding.rs`, `api/src/llm/provider.rs` |
| Graph | `worker/src/graph.rs`, `api/src/routes/graph.rs` |
| Qdrant | `core/src/qdrant/store.rs` |
| Retrieval / chat | `api/src/chat/retrieval.rs`, `api/src/routes/chat.rs` |
| Citations | `api/src/chat/mod.rs`, `api/src/chat/streaming.rs` |
| ReBAC | `api/src/rbac/`, `api/src/routes/acl.rs` |
| Schema | `backend/migrations/` |
| Tests | `api/tests/rebac_e2e.rs`, `worker/tests/qdrant_writer.rs`, `worker/tests/process_job_retry.rs` |
