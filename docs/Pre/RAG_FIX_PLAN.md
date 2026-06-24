# RAG Fix Plan — GMRAG 2.0 (T84D)

**Audit date:** 2026-06-23  
**Source:** [RAG_SYSTEM_AUDIT.md](./RAG_SYSTEM_AUDIT.md) · [RAG_SECURITY_AUDIT.md](./RAG_SECURITY_AUDIT.md) · [RAG_SCALABILITY_AUDIT.md](./RAG_SCALABILITY_AUDIT.md)  
**Note:** Documentation only — no code changes in T84D

---

## Master Issue Table

| Issue | Severity | Impact | Fix | Complexity |
|-------|----------|--------|-----|------------|
| Worker crash after BRPOP loses job permanently | P0 | Ingest never completes; doc stuck `processing`/`uploaded` | Persist queue with ACK pattern (Redis Streams/RabbitMQ) or sweeper re-enqueues `pending`/`processing` rows | High |
| Graph retrieval exposes entities from private documents to workspace members | P0 | Confidential entity/relationship leak via chat + graph API | Add `document_id` provenance to graph nodes; filter `retrieve_graph_context` by `accessible_document_ids` | High |
| Tenant delete leaves Qdrant collections + S3 objects | P0 | Compliance / data retention failure | Call `teardown_tenant_collections` + S3 prefix delete in `delete_tenant` | Medium |
| Redis LPUSH before Postgres COMMIT | P0 | Worker race; failed or flaky ingest | Move enqueue after commit (outbox pattern) or commit before LPUSH | Medium |
| No OCR in production ingest path | P1 | Scanned PDFs → empty text → empty index | Wire `parse_pdf_with_ocr` + PdfiumRenderer (T37) | High |
| No page metadata on chunks | P1 | PDF citation UX impossible in T85 | Per-page extract; store `page_start`/`page_end` on chunks + Qdrant payload + citation SSE | High |
| Grant-only users cannot chat (workspace member required) | P1 | Broken share→chat flow for external collaborators | Relax `ensure_workspace_member` when user has document grant OR auto-add workspace guest role | Medium |
| Stuck ingest jobs — no sweeper or reindex API | P1 | Ops cannot recover failed/stuck docs without re-upload | `POST .../documents/{id}/reindex` + cron for stale `processing` | Medium |
| Qdrant orphan vectors on failed delete cleanup | P1 | Storage leak; stale vectors | Retry cleanup; background orphan scanner; block PG delete until Qdrant OK | Medium |
| Graph orphan accumulation on document delete | P1 | Unbounded graph growth; stale entities in answers | Reference-count nodes by document; GC on delete; or periodic prune | High |
| Postgres pool `max_connections=10` | P1 | 503 under 500-user load | Configurable pool size; PgBouncer; horizontal API scaling | Low |
| Single sequential worker | P1 | 10k-doc ingest backlog | Parallel job processing; multiple worker replicas | Medium |
| Default `max_documents=100` quota | P1 | Blocks large tenants | Raise defaults or admin quota API | Low |
| `tenant_llm_config.dimensions` ignored | P1 | BYOK dimension mismatch silent | Pass dimensions to embedder or reject ≠768 | Low |
| `token_count` never written on ingest | P2 | Preview/metering incomplete | Compute on chunk insert | Low |
| No chat history in LLM prompt | P2 | Multi-turn chat incoherent | Load last N messages into `chat_stream` messages | Medium |
| No `GET .../chat_sessions/{sid}/messages` | P2 | T85 history reload blocked | Add list messages endpoint | Low |
| Assistant message strips citation tags | P2 | History loses citation linkage | Persist tags or separate citation JSON column | Medium |
| Graph API unpaginated full load | P2 | Timeout/memory at 100k nodes | Cursor pagination + filters | Medium |
| No chunk score threshold in retrieval | P2 | Irrelevant context → hallucination | Min score cutoff; configurable top_k | Low |
| No Prometheus / retrieval metrics | P2 | Cannot debug empty answers | Export latency, hit counts, zero-result rate | Medium |
| `/healthz` checks DB only | P2 | False ready during Qdrant/Redis outage | Deep health: Qdrant, Redis, S3 head | Low |
| `ingest_jobs.last_error` not exposed | P2 | Frontend cannot show fail reason | Add job status to document detail or sub-resource | Low |
| Upload duplicate files create duplicate docs | P2 | Storage/index bloat | Optional content-hash dedup per workspace | Medium |
| No rate limiting on upload/chat | P2 | Abuse / cost explosion | axum-governor or reverse-proxy limits | Medium |
| Graph LLM sent full doc with no truncation | P2 | Large doc extract fails silently or truncates at provider | Chunk-summarize graph extract or cap input tokens | Medium |
| MIME type not validated (always `.pdf` key) | P2 | Worker parse failure on non-PDF | Validate content-type; reject or branch parser | Low |
| No index on `document_chunks.qdrant_point_id` | P2 | Slow hydration at scale | `CREATE INDEX idx_document_chunks_point ON document_chunks(qdrant_point_id)` | Low |
| ILIKE graph fallback without trigram index | P2 | Slow graph fallback at 100k nodes | pg_trgm index or disable fallback at scale | Medium |
| Citation SSE missing snippet/score | P2 | Poor frontend UX | Add optional snippet (first 200 chars) + score to SSE | Low |
| `usage_events` unbounded growth | P2 | Table bloat | Partition/archival job | Medium |
| Invitation accept flow missing | P2 | Cannot onboard invited users | T52 follow-up accept endpoint | Medium |

---

## Priority Roadmap

### Before production (P0)

1. Job recovery (outbox or sweeper)
2. Graph ACL alignment with document ReBAC
3. Tenant delete data teardown
4. Commit-then-enqueue ordering

### Before T85 frontend (P1 affecting UX)

1. Page metadata decision (implement or defer PDF viewer)
2. Ingest error surface (`last_error` or status detail)
3. Grant-only chat policy decision
4. Document status polling contract (already on list — document in FRONTEND_ARCHITECTURE)

### Post-MVP scale (P1–P2)

1. Pool/worker horizontal scaling
2. Graph GC + pagination
3. Observability stack
4. Chat history API + prompt loading

---

## T85 Frontend Impact Matrix

| Issue | UI component affected | Workaround for T85 |
|-------|----------------------|-------------------|
| No page metadata | PDF citation viewer | Show chunk_index + link to preview endpoint |
| No messages list | Chat history sidebar | Session-only view; no reload |
| Grant-only chat block | Share dialog → chat | Document in copy: "must be workspace member to chat" |
| Graph workspace-wide | Graph explorer | Show all workspace entities; no per-doc filter |
| Ingest fail reason hidden | Upload progress | Poll `documents.status`; generic error on `failed` |
| Citation no snippet | Citation popover | Fetch preview chunk by document_id + chunk_index |
| Assistant history no tags | Message bubble | Don't show inline citations in history |

---

## Final Verdicts (from fix plan)

| Gate | Verdict |
|------|---------|
| RAG READY | **CONDITIONAL** — fix P0 ingest recovery + P1 OCR/pages before production RAG |
| GRAPH READY | **CONDITIONAL** — fix graph ACL + GC before GraphRAG production |
| SECURITY READY | **CONDITIONAL** — fix graph leak + tenant delete teardown |
| SCALABILITY READY | **CONDITIONAL** — pool/worker/quota/graph pagination |
| FRONTEND READY | **CONDITIONAL** — proceed T85 with documented UX limits; pages API optional |

---

## Issue Count by Category

| Category | P0 | P1 | P2 |
|----------|----|----|-----|
| Security | 3 | 2 | 0 |
| Performance | 0 | 3 | 4 |
| Scalability | 1 | 3 | 3 |
| Correctness | 2 | 4 | 5 |
| UX | 0 | 2 | 6 |
| Operations | 1 | 3 | 4 |

---

## Evidence References

Each issue traces to code paths documented in:
- `docs/RAG_SYSTEM_AUDIT.md` — Phases 2–12
- `docs/RAG_SECURITY_AUDIT.md` — SEC-1 through SEC-10
- `docs/RAG_SCALABILITY_AUDIT.md` — PostgreSQL, Qdrant, ingest throughput
- `docs/RAG_DATAFLOW.md` — sequence diagrams and constants
