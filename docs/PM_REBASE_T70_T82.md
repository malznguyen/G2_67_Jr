# PM Workbook Rebase — T70–T82 (Post T84C)

**Date:** 2026-06-23  
**Workbook:** `docs/GMRAG2_Project_Management.xlsx` (sheet **Kế hoạch Task**, rows 75–87)  
**Architecture reference:** [FRONTEND_ARCHITECTURE.md](./FRONTEND_ARCHITECTURE.md), [T85_IMPLEMENTATION_PLAN.md](./T85_IMPLEMENTATION_PLAN.md)  
**Audit prerequisites:** [T84A](./progress/T84A.md), [T84B](./FRONTEND_READINESS.md), [T84C](./progress/T84C.md)

This document records the **content rebase** of frontend and ops tasks T70–T82 after OpenAPI (T84A), API contract audit (T84B), and frontend architecture audit (T84C). Task IDs, priorities, owners, phases, and statuses were **not** changed.

---

## T70 — Frontend foundation

### Old meaning

- **Công việc:** `frontend/ package.json + Auth.js Keycloak provider + layout root + test page /`
- **Mục tiêu/đầu ra:** `Hoàn thành theo TDD: frontend/ package.json + Auth.js Keycloak provider + layout root + test page /`
- **Phụ thuộc:** `T52-T69`

### New meaning

- **Công việc:** Frontend foundation: Next.js 16 App Router, root layout, dev shell, providers (QueryClientProvider, theme/toast), shadcn/ui init, Auth.js v5 Keycloak
- **Mục tiêu/đầu ra:** Frontend: Next.js 16 + App Router + QueryClientProvider + shadcn/ui + Auth.js Keycloak + root layout + development shell
- **Khu vực chính:** `frontend/src/app + src/lib/query + src/components/ui`
- **Phụ thuộc:** `T84A, T84B, T84C`

### Reason for change

T84C locked the frontend stack (Next.js 16, React Query, shadcn/ui, Auth.js v5). Foundation is broader than auth + test page alone. Frontend work is gated on the completed audit trilogy (OpenAPI spec, contract audit, architecture doc), not merely backend API completion (T52–T69).

---

## T71 — API layer

### Old meaning

- **Công việc:** `lib/api.ts apiFetch (Bearer + X-Tenant-Id + error envelope parse) + type defs`
- **Mục tiêu/đầu ra:** `Frontend: lib/api.ts apiFetch (Bearer + X-Tenant-Id + error envelope parse) + type defs`

### New meaning

- **Công việc:** API layer: openapi-typescript + openapi-fetch — generate schema.d.ts từ /openapi.json; auth headers; X-Tenant-ID injection; HTTP error envelope `{ error: { code, message } }` parse
- **Mục tiêu/đầu ra:** Frontend: openapi-typescript + openapi-fetch + generated types + headers/errors helpers (per FRONTEND_ARCHITECTURE.md#API Client)
- **Khu vực chính:** `frontend/src/lib/api/`

### Reason for change

T84A delivers `/openapi.json` with 34 documented operations. Hand-written `apiFetch` and manual type defs are superseded by `openapi-typescript` + `openapi-fetch` middleware, matching the T84C recommendation and eliminating drift from the backend spec.

---

## T72 — Tenant context

### Old meaning

- **Công việc:** `context/TenantContext.tsx (fetch tenants, active tenant, switch) + test component`
- **Mục tiêu/đầu ra:** `Hoàn thành theo TDD: context/TenantContext.tsx (fetch tenants, active tenant, switch) + test component` *(copy-paste error — status was still "Chưa bắt đầu")*

### New meaning

- **Công việc:** Tenant context: active tenant, persistence (`gmrag:activeTenantId` localStorage + Zustand), tenant switching
- **Mục tiêu/đầu ra:** Frontend: tenant bootstrap via GET /users/me; active tenant state; switch tenant
- **Khu vực chính:** `frontend/src/lib/store/ + tenant selection routes`

### Reason for change

T84C splits client global state (Zustand for active tenant id) from server state (React Query). Persistence and switching are explicit requirements. The erroneous "Hoàn thành theo TDD:" prefix was removed (audit finding C16).

---

## T73 — Application shell

### Old meaning

- **Công việc:** `app/tenants/[tid]/workspaces/[wid]/layout.tsx + TenantSwitcher component (mẫu)`
- **Mục tiêu/đầu ra:** `Frontend: app/tenants/[tid]/workspaces/[wid]/layout.tsx + TenantSwitcher component (mẫu)`

### New meaning

- **Công việc:** Application shell: Next.js middleware protected routes, tenant layout `/t/[tid]/`, workspace layout, sidebar navigation, TenantSwitcher
- **Mục tiêu/đầu ra:** Frontend: auth middleware + tenant/workspace layouts + sidebar + TenantSwitcher (not sample layout only)
- **Khu vực chính:** `frontend/src/app/`

### Reason for change

T84C defines the route model (`/t/[tid]/…`), middleware session guard, and tenant guard — not a sample layout stub. The shell is a first-class deliverable with protected routes and navigation.

---

## T74 — Generated SDK integration

### Old meaning

- **Công việc:** `lib/{tenants,workspaces,documents,chat,acl,settings}.ts client wrappers`
- **Mục tiêu/đầu ra:** `Frontend: lib/{tenants,workspaces,documents,chat,acl,settings}.ts client wrappers`

### New meaning

- **Công việc:** Generated SDK integration: typed openapi-fetch client, thin domain wrappers, React Query keys/helpers, POST-SSE wrapper (sse.ts), multipart upload wrapper (upload.ts)
- **Mục tiêu/đầu ra:** Frontend: generated client + domain modules (tenants/workspaces/documents/chat/settings/metering) + query helpers + sse.ts + upload.ts
- **Khu vực chính:** `frontend/src/lib/`

### Reason for change

OpenAPI codegen is the primary transport layer. Hand-written wrappers remain only where the spec cannot cover POST-SSE and multipart upload. React Query helpers replace ad-hoc fetch patterns. Existing `lib/acl.ts` (T84) stays as reference implementation.

---

## T75 — Documents & ACL UI

### Old meaning

- **Công việc:** `components/AclShareDialog.tsx (mẫu modal share) + UploadDropzone skeleton`
- **Mục tiêu/đầu ra:** `Frontend: components/AclShareDialog.tsx (mẫu modal share) + UploadDropzone skeleton`

### New meaning

- **Công việc:** Documents & ACL UI: mount AclShareDialog (T84) trên document/chat; UploadDropzone; document status polling (React Query); permission UI
- **Mục tiêu/đầu ra:** Frontend: wire T84 AclShareDialog + UploadDropzone + status polling + permission UI
- **Khu vực chính:** `frontend/src/features/documents + components/`

### Reason for change

T84 already delivered `AclShareDialog` and `lib/acl.ts` out-of-order. T75 is now integration work — mounting the existing component, adding upload UX, status polling (requires worker status lifecycle), and permission affordances — not rebuilding the modal.

---

## T76 — Streaming chat

### Old meaning

- **Công việc:** `components/ChatPanel.tsx skeleton (SSE consumer port từ cũ)`
- **Mục tiêu/đầu ra:** `Frontend: components/ChatPanel.tsx skeleton (SSE consumer port từ cũ)`

### New meaning

- **Công việc:** Streaming chat UI: POST-SSE via @microsoft/fetch-event-source, AbortController, citation rendering, chat session handling
- **Mục tiêu/đầu ra:** Frontend: ChatPanel — POST-SSE events (text|citation|citation_unknown|done|error), AbortController, session CRUD
- **Khu vực chính:** `frontend/src/features/chat/`

### Reason for change

T84C mandates POST-SSE with Authorization and X-Tenant-ID headers. Native `EventSource` cannot send these headers. Full streaming chat with citations and abort support replaces a skeleton port.

---

## T77 — Design system primitives

### Old meaning

- **Công việc:** `Component list document: KnowledgeGraphView, QuotaIndicator, LlmSettingsForm (spec only, implement plan riêng)`
- **Mục tiêu/đầu ra:** `Frontend: Component list document: KnowledgeGraphView, QuotaIndicator, LlmSettingsForm (spec only, implement plan riêng)`

### New meaning

- **Công việc:** Design system primitives: QuotaIndicator, StatusBadge, CitationCard, EmptyState, ErrorState, LlmSettingsForm (+ shadcn base components)
- **Mục tiêu/đầu ra:** Frontend: shared UI primitives for quota, ingestion status, citations, empty/error states, LLM settings
- **Khu vực chính:** `frontend/src/components/ui + src/components/`

### Reason for change

MVP needs reusable primitives across documents, chat, and settings — not a spec-only component list. Graph visualization is deferred to T85 backlog (P2). shadcn/ui base components are part of the design system foundation.

---

## T78 — E2E integration test

### Old meaning

- **Công việc:** `Integration test E2E: tạo tenant → upload → poll ingest → chat → verify citation → share ACL`
- **Mục tiêu/đầu ra:** `Kiểm thử & vận hành: Integration test E2E: tạo tenant → upload → poll ingest → chat → verify citation → share ACL`

### New meaning

- **Công việc:** E2E integration test (OpenAPI contract + frontend flow): login → select tenant → upload → poll ingest status → chat stream → verify citation → ACL share
- **Mục tiêu/đầu ra:** Kiểm thử & vận hành: E2E full flow theo OpenAPI contract + frontend (Playwright): login/tenant/upload/poll/chat/citation/ACL
- **Kiểm thử/verify:** Playwright E2E + cargo test --workspace; validate against OpenAPI contract

### Reason for change

E2E must exercise the browser auth path (Auth.js), tenant selection, generated API client, and POST-SSE chat — not curl-only backend integration. OpenAPI contract validation ensures FE and BE stay aligned.

---

## T79 — RLS pentest + ReBAC validation

### Old meaning

- **Công việc:** `RLS pentest: negative tests cross-tenant leak (2 tenants, cố đọc data nhau → 403/empty)`
- **Mục tiêu/đầu ra:** `Kiểm thử & vận hành: RLS pentest: negative tests cross-tenant leak (2 tenants, cố đọc data nhau → 403/empty)`

### New meaning

- **Công việc:** RLS pentest + ReBAC validation: negative tests cross-tenant leak; share→access, revoke→denied, workspace inheritance (bổ sung tests/rebac_e2e.rs T85)
- **Mục tiêu/đầu ra:** Kiểm thử & vận hành: RLS pentest cross-tenant + ReBAC validation (share/revoke/inheritance); 403/empty/404 semantics

### Reason for change

T85 already delivered backend ReBAC E2E tests. T79 should explicitly cover ReBAC validation scenarios (share, revoke, inheritance) in addition to RLS cross-tenant isolation, and extend to frontend E2E when T78 harness exists.

---

## T80 — Quota enforcement test

### Old meaning

- **Công việc:** `Quota enforcement test (vượt → 429)`
- **Mục tiêu/đầu ra:** `Kiểm thử & vận hành: Quota enforcement test (vượt → 429)`

### New meaning

*(No change — functional scope unchanged.)*

### Reason for change

Quota enforcement test scope remains valid post-T84C. No architecture impact on this task.

---

## T81 — Full-stack smoke test

### Old meaning

- **Công việc:** `docker-compose full stack smoke test + README deploy guide`
- **Mục tiêu/đầu ra:** `Kiểm thử & vận hành: docker-compose full stack smoke test + README deploy guide`

### New meaning

- **Công việc:** docker-compose full-stack smoke test: frontend + backend + worker + postgres + qdrant + keycloak (+ minio/redis/ollama) + README deploy guide
- **Mục tiêu/đầu ra:** Kiểm thử & vận hành: docker-compose full-stack smoke (frontend container + backend + postgres + qdrant + keycloak) + README deploy guide
- **Ghi chú:** Extended to verify frontend container healthy; 9/9 services; recovery entry point for T82

### Reason for change

Frontend is now a first-class compose service. Full-stack smoke must include the frontend container plus core dependencies (postgres, qdrant, keycloak), not backend-only verification.

---

## T82 — Backup & recovery

### Old meaning

- **Công việc:** `Qdrant snapshot backup script + Postgres pg_dump per-tenant script`
- **Mục tiêu/đầu ra:** `Kiểm thử & vận hành: Qdrant snapshot backup script + Postgres pg_dump per-tenant script`

### New meaning

- **Công việc:** Qdrant snapshot backup script + Postgres pg_dump per-tenant script + recovery documentation (restore runbook)
- **Mục tiêu/đầu ra:** Kiểm thử & vận hành: Qdrant snapshot + Postgres pg_dump per-tenant + recovery documentation (restore procedure)
- **Ghi chú:** Document restore/runbook alongside scripts

### Reason for change

Backup scripts without recovery documentation are incomplete for ops readiness. Runbook documents per-tenant restore procedure.

---

## Final validation

| Check | Result |
|-------|--------|
| T70 gated on T84A, T84B, T84C (Ghi chú explains non-row IDs) | Yes |
| T71–T77 sequential dependency chain (T70→…→T76) | Yes |
| OpenAPI / codegen reflected in T71, T74 | Yes — openapi-typescript, openapi-fetch, generated client |
| React Query in T70 (QueryClientProvider), T74 (query helpers), T75 (status polling) | Yes |
| Auth.js in T70 | Yes — Auth.js v5 Keycloak |
| POST-SSE in T76 | Yes — @microsoft/fetch-event-source |
| Generated SDK in T71, T74 | Yes |
| T78 OpenAPI contract + frontend flow | Yes — Playwright E2E |
| T79 ReBAC validation | Yes |
| T81 full stack incl. frontend container | Yes |
| T82 recovery documentation | Yes |
| Task IDs T70–T82 unchanged | Yes |
| Priorities, owners, phases, statuses unchanged | Yes |

### Known inconsistencies (documented, not fixed in this pass)

- Workbook **T84** row still lists dependency `T67, T75` though T84 component was completed out-of-order; **T75** now correctly *mounts* T84.
- **T84A/B/C** are complete in progress docs but not separate workbook rows — T70 **Ghi chú** explains this.

### Cross-references

- [FRONTEND_ARCHITECTURE.md](./FRONTEND_ARCHITECTURE.md) — locked stack and folder structure
- [API_INVENTORY.md](./API_INVENTORY.md) — 34 endpoints, response shapes
- [FRONTEND_READINESS.md](./FRONTEND_READINESS.md) — T84B Go/No-Go audit
- [progress/T84C.md](./progress/T84C.md) — architecture audit summary

---

*Workbook rows updated: 2026-06-23. No code changes in this rebase.*
