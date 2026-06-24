# GMRAG API Inventory (T84B)

**Generated:** 2026-06-23  
**Source of truth:** `backend/crates/api/src/lib.rs`  
**OpenAPI spec:** `GET /openapi.json` (34 operations)  
**Swagger UI:** `GET /swagger`

---

## Summary

| Metric | Count |
|--------|------:|
| Production HTTP endpoints | **34** |
| Documentation routes (meta) | **2** (`/swagger`, `/openapi.json`) |
| OpenAPI-documented operations | **34 / 34** |
| Auth tiers | 3 (public, JWT-only, JWT + tenant + RLS) |
| Route modules | 12 |

### Endpoints by domain

| Domain | Tag | Count | Auth tier |
|--------|-----|------:|-----------|
| Health | Health | 2 | Public |
| Users | Users | 1 | JWT |
| Tenants | Tenants | 4 | JWT (2) / Tenant (2) |
| Tenant members | TenantMembers | 3 | Tenant |
| Workspaces | Workspaces | 4 | Tenant |
| Workspace members | WorkspaceMembers | 3 | Tenant |
| Documents | Documents | 4 | Tenant |
| ACL (ReBAC sharing) | ACL | 3 | Tenant |
| Chat | Chat | 4 | Tenant |
| Graph | Graph | 1 | Tenant |
| Settings (LLM/BYOK) | Settings | 2 | Tenant |
| Metering & audit | Metering | 3 | Tenant |

### Middleware chain (tenant-scoped routes)

```
Request → auth_middleware → tenant_middleware → rls_middleware → handler
```

| Layer | Requirement |
|-------|-------------|
| `auth_middleware` | `Authorization: Bearer <JWT>` (Keycloak OIDC) |
| `tenant_middleware` | `X-Tenant-Id: <uuid>` — user must be in `tenant_members` |
| `rls_middleware` | Sets PostgreSQL `app.tenant_id` on per-request transaction |

Path `{tid}` must equal `X-Tenant-Id` (enforced by `ensure_path_matches_context`).

---

## Full endpoint inventory

### Health (public)

| Method | Path | Auth | Permission | Query | Request | Response (200/201) | Success | Errors |
|--------|------|------|------------|-------|---------|-------------------|---------|--------|
| GET | `/health` | None | — | — | — | `{ status, service, uptime_ms }` | 200 | — |
| GET | `/healthz` | None | — | — | — | `{ status, db }` | 200 ready / **503** degraded | 503 uses non-error shape |

**OpenAPI schemas:** `HealthResponse`, `HealthzResponse`

---

### Users (JWT only)

| Method | Path | Auth | Permission | Query | Request | Response | Success | Errors |
|--------|------|------|------------|-------|---------|----------|---------|--------|
| GET | `/users/me` | Bearer JWT | Any authenticated user | — | — | `{ user: UserProfile, tenants: UserTenantMembership[] }` | 200 | 401, 404, 500 |

**Sort:** tenants unordered (no `ORDER BY` in SQL).  
**OpenAPI:** `MeResponse`

---

### Tenants

| Method | Path | Auth | Permission | Query | Request | Response | Success | Errors |
|--------|------|------|------------|-------|---------|----------|---------|--------|
| GET | `/tenants` | Bearer JWT | Any member (lists own tenants) | — | — | `{ tenants: TenantListItem[] }` | 200 | 401, 500 |
| POST | `/tenants` | Bearer JWT | Creator becomes `owner` | — | `{ name }` | `{ id, name, role: "owner" }` | **201** | 400, 401, 500 |
| PATCH | `/tenants/{tid}` | JWT + `X-Tenant-Id` | **Tenant owner** | — | `{ name }` | `{ id, name }` | 200 | 400, 401, 403, 404, 500 |
| DELETE | `/tenants/{tid}` | JWT + `X-Tenant-Id` | **Tenant owner** | — | — | empty | **204** | 400, 401, 403, 404, 500 |

**Sort (list):** `created_at ASC`  
**OpenAPI:** `TenantsResponse`, `CreateTenantRequest/Response`, `UpdateTenantRequest/Response`

---

### Tenant members

| Method | Path | Auth | Permission | Query | Request | Response | Success | Errors |
|--------|------|------|------------|-------|---------|----------|---------|--------|
| GET | `/tenants/{tid}/members` | JWT + tenant | Any tenant member | — | — | `{ members: TenantMemberItem[] }` | 200 | 400, 401, 500 |
| POST | `/tenants/{tid}/members` | JWT + tenant | **Tenant owner** (invite) | — | `{ email, role? }` | `{ id, email, role, token, status }` | **201** | 400, 401, 403, 500 |
| DELETE | `/tenants/{tid}/members/{user_id}` | JWT + tenant | **Tenant owner** | — | — | empty | **204** | 400, 401, 403, 404, 500 |

**Sort (list):** `email ASC`  
**Note:** Invite creates `invitations` row; no accept endpoint yet (see AUDIT_ACTION_ITEMS C10).  
**OpenAPI:** `TenantMembersResponse`, `InviteMemberRequest/Response`

---

### Workspaces

| Method | Path | Auth | Permission | Query | Request | Response | Success | Errors |
|--------|------|------|------------|-------|---------|----------|---------|--------|
| GET | `/tenants/{tid}/workspaces` | JWT + tenant | Any tenant member | — | — | `{ workspaces: WorkspaceItem[] }` | 200 | 400, 401, 500 |
| POST | `/tenants/{tid}/workspaces` | JWT + tenant | Any tenant member | — | `{ name, slug }` | full workspace object | **201** | 400, 401, 500 |
| PATCH | `/tenants/{tid}/workspaces/{wid}` | JWT + tenant | Any tenant member (RLS) | — | `{ name, slug }` | `{ id, name, slug }` | 200 | 400, 401, 404, 500 |
| DELETE | `/tenants/{tid}/workspaces/{wid}` | JWT + tenant | Any tenant member (RLS) | — | — | empty | **204** | 400, 401, 404, 500 |

**Sort (list):** `created_at ASC`  
**OpenAPI:** `WorkspacesResponse`, `CreateWorkspaceRequest/Response`, `UpdateWorkspaceRequest/Response`

---

### Workspace members

| Method | Path | Auth | Permission | Query | Request | Response | Success | Errors |
|--------|------|------|------------|-------|---------|----------|---------|--------|
| GET | `/tenants/{tid}/workspaces/{wid}/members` | JWT + tenant | Any tenant member | — | — | `{ members: WorkspaceMemberItem[] }` | 200 | 400, 401, 500 |
| POST | `/tenants/{tid}/workspaces/{wid}/members` | JWT + tenant | Any tenant member | — | `{ user_id, role? }` | `{ workspace_id, user_id, role }` | **201** | 400, 401, 500 |
| DELETE | `/tenants/{tid}/workspaces/{wid}/members/{user_id}` | JWT + tenant | Any tenant member | — | — | empty | **204** | 400, 401, 404, 500 |

**Sort (list):** `email ASC`  
**Default role:** `member`  
**OpenAPI:** `WorkspaceMembersResponse`, `AddWorkspaceMemberRequest/Response`

---

### Documents

| Method | Path | Auth | Permission | Query | Request | Response | Success | Errors |
|--------|------|------|------------|-------|---------|----------|---------|--------|
| GET | `/tenants/{tid}/documents` | JWT + tenant | ReBAC **viewer** (SQL predicate) | `workspace_id?` | — | `{ documents: DocumentItem[] }` | 200 | 400, 401, 500 |
| POST | `/tenants/{tid}/documents` | JWT + tenant | Any tenant member | — | **multipart:** `file`, `visibility`, `workspace_id`, `title?` | `{ id }` | **201** | 400, 401, **429**, 500 |
| DELETE | `/tenants/{tid}/documents/{did}` | JWT + tenant | ReBAC **owner** | — | — | empty | **204** | 400, 401, 403, 404, 500 |
| GET | `/tenants/{tid}/documents/{did}/preview` | JWT + tenant | ReBAC **viewer** (404 if denied) | — | — | `{ document, chunks[] }` | 200 | 400, 401, 404, 500 |

**Sort (list):** `created_at DESC`  
**Preview chunks:** max **50**, `chunk_index ASC` (not paginated)  
**Visibility values:** `shared` \| `private`  
**Status values:** `uploaded`, `processing`, `indexed`, `failed`  
**Body limit:** 50 MiB multipart  
**OpenAPI:** `DocumentsResponse`, `CreateDocumentResponse`, `DocumentPreviewResponse`, `UploadDocumentForm`

---

### ACL (ReBAC grants)

| Method | Path | Auth | Permission | Query | Request | Response | Success | Errors |
|--------|------|------|------------|-------|---------|----------|---------|--------|
| GET | `/tenants/{tid}/acl` | JWT + tenant | ReBAC **viewer** on resource | `resource_type`, `resource_id` (**required**) | — | `{ grants: GrantItem[] }` | 200 | 400, 401, 404, 500 |
| POST | `/tenants/{tid}/acl` | JWT + tenant | ReBAC **owner** on resource | — | `{ resource_type, resource_id, principal_type, principal_id, relation }` | full grant object | **201** | 400, 401, 403, 500 |
| DELETE | `/tenants/{tid}/acl/{grant_id}` | JWT + tenant | ReBAC **owner** on resource | — | — | empty | **204** | 400, 401, 403, 404, 500 |

**Shareable `resource_type`:** `document`, `chat_session`  
**Grantable `relation`:** `editor`, `viewer` (not `owner` or `member`)  
**Principal types:** `user`, `workspace`  
**Sort (list):** `created_at ASC`  
**OpenAPI:** `GrantsResponse`, `CreateGrantRequest/Response`

---

### Chat

| Method | Path | Auth | Permission | Query | Request | Response | Success | Errors |
|--------|------|------|------------|-------|---------|----------|---------|--------|
| GET | `/tenants/{tid}/chat_sessions` | JWT + tenant | ReBAC **viewer** (SQL predicate) | — | — | `{ sessions: ChatSessionItem[] }` | 200 | 400, 401, 500 |
| POST | `/tenants/{tid}/chat_sessions` | JWT + tenant | Any tenant member (caller owns session) | — | `{ title?, workspace_id?, model? }` | `{ id }` | **201** | 400, 401, 500 |
| DELETE | `/tenants/{tid}/chat_sessions/{sid}` | JWT + tenant | ReBAC **owner** | — | — | empty | **204** | 400, 401, 403, 404, 500 |
| POST | `/tenants/{tid}/chat_sessions/{sid}/chat` | JWT + tenant | ReBAC **viewer** | — | `{ message }` | **SSE** `text/event-stream` | 200 | 400, 401, 404, 500 |

**Sort (list):** `updated_at DESC`  
**SSE events:** see Appendix B  
**Missing endpoint:** `GET .../chat_sessions/{sid}/messages` (planned R2)  
**OpenAPI:** `ChatSessionsResponse`, `CreateChatSessionRequest/Response`, `PostChatRequest`, `ChatSseEvent`

---

### Graph

| Method | Path | Auth | Permission | Query | Request | Response | Success | Errors |
|--------|------|------|------------|-------|---------|----------|---------|--------|
| GET | `/tenants/{tid}/workspaces/{wid}/graph` | JWT + tenant | ReBAC **member** on workspace | — | — | `{ nodes[], edges[] }` | 200 | 400, 401, 404, 500 |

**Sort:** nodes/edges `created_at ASC`  
**404:** workspace missing or caller not a member (existence hidden)  
**OpenAPI:** `WorkspaceGraphResponse`

---

### Settings (LLM / BYOK)

| Method | Path | Auth | Permission | Query | Request | Response | Success | Errors |
|--------|------|------|------------|-------|---------|----------|---------|--------|
| GET | `/tenants/{tid}/settings/llm` | JWT + tenant | **Tenant owner** | — | — | flat `LlmSettingsResponse` | 200 | 400, 401, 403, 500 |
| PUT | `/tenants/{tid}/settings/llm` | JWT + tenant | **Tenant owner** | — | `PutLlmSettingsRequest` | flat `LlmSettingsResponse` | 200 | 400, 401, 403, 500 |

**Providers:** `ollama`, `openai`  
**API key:** encrypted at rest; GET returns masked key only  
**OpenAPI:** `LlmSettingsResponse`, `PutLlmSettingsRequest`

---

### Metering & audit

| Method | Path | Auth | Permission | Query | Request | Response | Success | Errors |
|--------|------|------|------------|-------|---------|----------|---------|--------|
| GET | `/tenants/{tid}/metering/usage` | JWT + tenant | **Tenant owner** | — | — | `{ usage: UsageMetricItem[] }` | 200 | 400, 401, 403, 500 |
| GET | `/tenants/{tid}/quotas` | JWT + tenant | **Tenant owner** | — | — | flat `QuotaResponse` | 200 | 400, 401, 403, 500 |
| GET | `/tenants/{tid}/audit_logs` | JWT + tenant | **Tenant owner** | — | — | `{ logs: AuditLogItem[] }` | 200 | 400, 401, 403, 500 |

**Usage sort:** `metric ASC`  
**Audit sort:** `created_at DESC`, **hard limit 100** (not client-configurable)  
**Quota defaults** (when unconfigured): docs=100, workspaces=10, storage=10GB, members=50  
**OpenAPI:** `UsageResponse`, `QuotaResponse`, `AuditLogsResponse`

---

## Response envelope cheat sheet

| Pattern | Endpoints |
|---------|-----------|
| `{ "<plural>": [...] }` | tenants, workspaces, documents, sessions, members, grants, usage, logs |
| `{ user, tenants }` | GET `/users/me` |
| `{ document, chunks }` | GET `.../preview` |
| `{ nodes, edges }` | GET `.../graph` |
| Flat root object | settings, quotas |
| `{ id }` only on create | documents, chat_sessions |
| Full entity on create | tenants, workspaces, ACL grants, invitations |
| Empty body | All DELETE → 204 |
| SSE stream | POST `.../chat` |

There is **no** unified `{ "data": ... }` wrapper.

---

## Appendix A — Error codes

### Standard HTTP JSON envelope

```json
{ "error": { "code": "kebab-case", "message": "human-readable" } }
```

| HTTP | Code | Source |
|------|------|--------|
| 400 | `bad-request` | Handlers, tenant middleware (missing/invalid `X-Tenant-Id`, path mismatch) |
| 401 | `missing-header` | Auth middleware |
| 401 | `invalid-token` | Auth middleware / JWT validation |
| 401 | `user-not-found` | Auth provisioning |
| 403 | `forbidden` | Tenant role, ReBAC owner actions, not-a-member |
| 404 | `not-found` | Missing resource or intentional authz hide |
| 429 | `quota-exceeded` | Document upload storage quota |
| 500 | `internal-error` | Catch-all handler errors |
| 500 | `database-error`, `config-error`, `qdrant-error`, … | `gmrag_core::Error` variants |
| 500 | `tenant-missing-auth`, `tenant-missing-pool` | Tenant middleware |
| 500 | `rls-missing-tenant`, `rls-missing-pool`, `rls-begin-failed`, `rls-set-tenant-failed`, `rls-commit-failed` | RLS middleware |
| 503 | `jwks-fetch-failed` | Auth middleware (Keycloak unreachable) |
| 503 | `rls-connection-failed` | RLS middleware |

**Not used:** 409 Conflict, 422 Unprocessable Entity.

**Exceptions:** `/healthz` 503 returns `{ status: "degraded", db: "down" }` (not error envelope).

---

## Appendix B — SSE chat events

Content-Type: `text/event-stream`. Each `data:` line is tagged JSON:

| `type` | Fields | Description |
|--------|--------|-------------|
| `text` | `content` | LLM token chunk |
| `citation` | `index`, `point_id`, `document_id`, `chunk_index`, `filename?` | Retrieved chunk reference |
| `citation_unknown` | `index` | Citation index without resolved metadata |
| `done` | `finish_reason?` | Stream complete |
| `error` | `code`, `message` | In-stream failure (**not** nested under `error` key) |

**In-stream error codes:** `stream-failed`, `persist-failed`

HTTP status remains **200** once the SSE stream starts; pre-stream failures use standard HTTP error envelope.

---

## Appendix C — Multipart upload fields

`POST /tenants/{tid}/documents` — `multipart/form-data`:

| Field | Required | Type | Notes |
|-------|----------|------|-------|
| `file` | Yes | binary | Uploaded file |
| `visibility` | Yes | string | `shared` or `private` |
| `workspace_id` | Yes | UUID | Target workspace |
| `title` | No | string | Defaults from filename |

---

## Appendix D — ReBAC permission matrix

| Route | Gate |
|-------|------|
| List documents / sessions | SQL compiles ReBAC **viewer** predicate |
| Preview document, POST chat, list ACL grants | **viewer** via `check_relation` |
| Delete document/session, create/revoke ACL | **owner** via `check_relation` |
| GET graph | **member** on workspace |
| PATCH/DELETE tenant, settings, metering, invite/remove members | **tenant owner** via `require_owner()` |
| List/create workspaces, ws members | Any **tenant member** (RLS) |

**403 vs 404:** Viewer-denied reads return **404** (no existence leak). Owner-only write failures return **403**.

---

## Validation

- OpenAPI operation count: **≥ 34** (asserted in `backend/crates/api/tests/openapi.rs`)
- Production routes in `lib.rs`: **34** handlers
- Undocumented production handlers: **0**
