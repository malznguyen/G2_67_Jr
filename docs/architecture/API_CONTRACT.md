# API Contract

Generated from current source on 2026-07-07.

Verified from:
- `backend/openapi.json` (read as the route/schema/status source of truth)
- `backend/crates/api/src/lib.rs` (mounted routes and middleware)
- `backend/crates/api/src/routes/*.rs` (handler behavior)
- `backend/crates/api/src/auth/middleware.rs`
- `backend/crates/api/src/auth/tenant.rs`
- `backend/crates/api/src/middleware/rls.rs`
- `backend/crates/api/src/middleware/rate_limit.rs`
- `backend/crates/api/src/openapi/mod.rs`
- `backend/crates/api/src/openapi/schemas.rs`
- `backend/crates/core/src/config.rs`
- `frontend/lib/api/schema.d.ts`
- `frontend/lib/api/client.ts`
- `frontend/lib/store/tenant.ts`

## Global Contract

All authenticated routes require `Authorization: Bearer <JWT>`. Tenant-scoped routes are mounted under `/tenants/:tid/...` in Axum and published as `/tenants/{tid}/...` in OpenAPI. They run through:

1. `auth_middleware`: validates the bearer token with Keycloak/OIDC and provisions the user row through `AdminPool`.
2. `tenant_middleware`: reads the configured tenant header, parses it as UUID, checks tenant existence, checks OpenFGA `member` on `tenant:{tid}`, and stores `TenantContext`.
3. `rate_limit_middleware`: applies Redis-backed per-category request limits when enabled.
4. `rls_middleware`: opens a transaction, sets `SET LOCAL app.tenant_id = '<tid>'`, stores `SharedConnection`, and commits after the handler.

The configured tenant header default is `X-Tenant-ID` from `Config::DEFAULT_TENANT_HEADER` and `.env.example` (`GMRAG_TENANT_HEADER=X-Tenant-ID`). OpenAPI, `frontend/lib/api/schema.d.ts`, and direct frontend header call sites now use the same spelling.

Common error body:

```json
{ "error": { "code": "kebab-case", "message": "human readable" } }
```

## Drift Found

- Resolved: tenant header casing is now `X-Tenant-ID` in runtime config, OpenAPI, generated frontend schema, and direct frontend calls.
- Resolved: tenant-member operations are `list_tenant_members` and `remove_tenant_member`; workspace-member operations are `list_workspace_members` and `remove_workspace_member`.
- Resolved: OpenAPI now declares the verified missing workspace `403`/`404` responses and OpenFGA outage `503 authorization-unavailable` responses found in route handlers.
- OpenAPI omits `/metrics`, but `backend/crates/api/src/lib.rs` mounts it as a public Prometheus endpoint.
- Runtime Axum routes use `{did}` for document id (`/tenants/:tid/documents/:did`); OpenAPI matches this as `{did}`. Old references to `{doc_id}` are stale.

## Health

| Method | Path | Auth | Request | Success | Handler notes |
|---|---|---:|---|---|---|
| GET | `/health` | No | none | `200 HealthResponse { status, service, uptime_ms }` | Liveness only. |
| GET | `/healthz` | No | none | `200/503 HealthzResponse { status, db, openfga }` | Checks Postgres through `AdminPool` and OpenFGA health. |
| GET | `/metrics` | No | none | Prometheus text | Mounted in Rust, not present in OpenAPI. |

## Users

| Method | Path | Headers | Request | Responses | Auth/behavior |
|---|---|---|---|---|---|
| GET | `/users/me` | `Authorization` | none | `200 MeResponse { user, tenants[] }`; `401`; `404`; `500`; `503` | Uses `AdminPool`. Lists tenant memberships by OpenFGA `ListObjects(user, member, tenant)` and intersects with `tenant_members`/`tenants`. No tenant header. |

`UserProfile`: `id`, `email`, `name`, `created_at`. `UserTenantMembership`: `id`, `name`, `role`.

## Tenants

| Method | Path | Headers | Request | Responses | Auth/behavior |
|---|---|---|---|---|---|
| GET | `/tenants` | `Authorization` | none | `200 TenantsResponse { tenants[] }`; `401`; `500`; `503` | Uses `AdminPool`. OpenFGA `ListObjects(user, member, tenant)` is authoritative. |
| POST | `/tenants` | `Authorization` | `CreateTenantRequest { name }` | `201 CreateTenantResponse { id, name, role }`; `400`; `401`; `500`; `503` | Trims and rejects empty name. Creates tenant/member as `owner`; writes OpenFGA owner tuple; compensates DB row if OpenFGA write fails. |
| PATCH | `/tenants/{tid}` | `Authorization`, tenant header | `UpdateTenantRequest { name }` | `200 UpdateTenantResponse { id, name }`; `400`; `401`; `403`; `404`; `500`; `503` | Requires path tenant to match header-derived `TenantContext`. OpenFGA `owner` required. Uses `SharedConnection`. |
| DELETE | `/tenants/{tid}` | `Authorization`, tenant header | none | `204`; `400`; `401`; `403`; `404`; `500`; `503` | Owner-only. Deletes OpenFGA objects for tenant/workspaces/documents/chats, best-effort tears down Qdrant tenant collections and S3 prefix `{tid}/`, then deletes tenant row. |

Tenant roles are `owner`, `admin`, `member`; current owner guards check only `owner`.

## Tenant Members

| Method | Path | Headers | Request | Responses | Auth/behavior |
|---|---|---|---|---|---|
| GET | `/tenants/{tid}/members` | `Authorization`, tenant header | none | `200 TenantMembersResponse { members[] }`; `400`; `401`; `500` | Any tenant member can list members under RLS. |
| POST | `/tenants/{tid}/members` | `Authorization`, tenant header | `InviteMemberRequest { email, role? }` | `201 InviteMemberResponse { id, email, role, token, status }`; `400`; `401`; `403`; `500`; `503` | Owner-only. Empty role defaults to `member`; role must be `owner`, `admin`, or `member`. Inserts invitation, not membership. |
| DELETE | `/tenants/{tid}/members/{user_id}` | `Authorization`, tenant header | none | `204`; `400`; `401`; `403`; `404`; `500`; `503` | Owner-only. Refuses to remove the last tenant owner. Deletes OpenFGA owner/admin/member tuples before deleting membership. |

`TenantMemberItem`: `user_id`, `role`, `email`, `name`.

## Workspaces

| Method | Path | Headers | Request | Responses | Auth/behavior |
|---|---|---|---|---|---|
| GET | `/tenants/{tid}/workspaces` | `Authorization`, tenant header | none | `200 WorkspacesResponse { workspaces[] }`; `400`; `401`; `500`; `503` | Lists OpenFGA `accessor` workspaces, then intersects with RLS-scoped SQL. |
| POST | `/tenants/{tid}/workspaces` | `Authorization`, tenant header | `CreateWorkspaceRequest { name, slug }` | `201 CreateWorkspaceResponse { id, name, slug, created_by, created_at }`; `400`; `401`; `500`; `503` | Trims and rejects empty `name`/`slug`. Creates workspace and creator workspace member as `owner`; writes OpenFGA tenant and owner tuples. |
| PATCH | `/tenants/{tid}/workspaces/{wid}` | `Authorization`, tenant header | `UpdateWorkspaceRequest { name, slug }` | `200 UpdateWorkspaceResponse { id, name, slug }`; `400`; `401`; `403`; `404`; `500`; `503` | Requires OpenFGA workspace `manager`; denied manager returns `403`. |
| DELETE | `/tenants/{tid}/workspaces/{wid}` | `Authorization`, tenant header | none | `204`; `400`; `401`; `403`; `404`; `500`; `503` | Requires workspace `manager`. Deletes OpenFGA workspace object and document/chat workspace tuples, best-effort deletes Qdrant workspace chunks and S3 prefix `{tid}/{wid}/`, then deletes workspace row. |

`WorkspaceItem`: `id`, `name`, `slug`, `created_by`, `created_at`.

## Workspace Members

| Method | Path | Headers | Request | Responses | Auth/behavior |
|---|---|---|---|---|---|
| GET | `/tenants/{tid}/workspaces/{wid}/members` | `Authorization`, tenant header | none | `200 WorkspaceMembersResponse { members[] }`; `400`; `401`; `404`; `500`; `503` | Requires workspace access; denied access is hidden as `404` by helper. |
| POST | `/tenants/{tid}/workspaces/{wid}/members` | `Authorization`, tenant header | `AddWorkspaceMemberRequest { user_id, role? }` | `201 AddWorkspaceMemberResponse { workspace_id, user_id, role }`; `400`; `401`; `403`; `404`; `500`; `503` | Requires workspace `manager`. Target user must already be a tenant member. Role defaults to `member` and must be `owner`, `admin`, or `member`. |
| DELETE | `/tenants/{tid}/workspaces/{wid}/members/{user_id}` | `Authorization`, tenant header | none | `204`; `400`; `401`; `403`; `404`; `500`; `503` | Requires workspace `manager`. Refuses to remove the last workspace owner/admin. Deletes the matching OpenFGA workspace role tuple. |

`WorkspaceMemberItem`: `user_id`, `role`, `email`, `name`.

## Documents

| Method | Path | Headers | Request | Responses | Auth/behavior |
|---|---|---|---|---|---|
| GET | `/tenants/{tid}/documents?workspace_id=` | `Authorization`, tenant header | optional query `workspace_id` | `200 DocumentsResponse { documents[] }`; `400`; `401`; `500`; `503` | Lists OpenFGA `viewer` documents and intersects with RLS SQL; optional workspace filter. |
| POST | `/tenants/{tid}/documents` | `Authorization`, tenant header; multipart | `UploadDocumentForm { file, visibility, workspace_id, title? }` | `201 CreateDocumentResponse { id }`; `400`; `401`; `403`; `404`; `429`; `500`; `503` | Max body 50 MiB. `visibility` must be `shared` or `private`. Requires workspace access. Checks `tenant_quotas` if configured. Writes S3 key `{tid}/{workspace_id}/{document_id}.pdf`, inserts `documents`, `ingest_jobs`, and `ingest_outbox`, then writes OpenFGA tuples. Does not LPUSH Redis directly. |
| DELETE | `/tenants/{tid}/documents/{did}` | `Authorization`, tenant header | none | `204`; `400`; `401`; `403`; `404`; `500`; `503` | Owner-only via OpenFGA `owner`. Deletes OpenFGA object; best-effort deletes S3 object and Qdrant chunks; deletes DB row; then deletes orphan graph nodes and their Qdrant graph points. |
| GET | `/tenants/{tid}/documents/{did}/preview` | `Authorization`, tenant header | none | `200 DocumentPreviewResponse { document, chunks[] }`; `400`; `401`; `404`; `500`; `503` | Loads metadata under RLS, requires OpenFGA `viewer`, returns at most 50 chunks ordered by `chunk_index`. Denied viewer is `404`. |

`DocumentItem`: `id`, `title`, `visibility`, `owner_id`, `workspace_id?`, `status`, `created_at`.

`DocumentPreviewMeta`: `id`, `title`, `status`, `visibility`, `owner_id`, `workspace_id?`, `mime_type?`, `byte_size`, `created_at`.

`DocumentChunkPreview`: `chunk_index`, `content`, `token_count?`.

Document status enum in OpenAPI/DB: `uploaded`, `processing`, `indexed`, `failed`. Visibility enum: `shared`, `private`.

## ACL / Sharing

ACL routes are backed by OpenFGA direct tuples. The old `resource_acl` table is dropped in the current migration set.

| Method | Path | Headers | Request | Responses | Auth/behavior |
|---|---|---|---|---|---|
| GET | `/tenants/{tid}/acl?resource_type=&resource_id=` | `Authorization`, tenant header | query `resource_type=document\|chat_session`, `resource_id` | `200 GrantsResponse { grants[] }`; `400`; `401`; `404`; `503` | Resource must exist under RLS. Caller must be OpenFGA `viewer`; denied is `404`. Reads direct OpenFGA relationships for the object. |
| POST | `/tenants/{tid}/acl` | `Authorization`, tenant header | `CreateGrantRequest { resource_type, resource_id, principal_type, principal_id, relation }` | `201 CreateGrantResponse`; `400`; `401`; `403`; `404`; `503` | `resource_type` must be `document` or `chat_session`. `principal_type` must be `user` or `workspace`. `relation` must be `editor` or `viewer`. Caller must be resource `owner`. Principal must belong to current tenant. Writes `audit_log` action `acl.grant`. |
| DELETE | `/tenants/{tid}/acl/{grant_id}` | `Authorization`, tenant header | none | `204`; `400`; `401`; `403`; `404`; `503` | `grant_id` is an opaque base64url encoded OpenFGA tuple payload. Only `editor`/`viewer` grants are revocable. Caller must be resource `owner`. Writes `audit_log` action `acl.revoke`. |

`GrantItem`: `id`, `principal_type`, `principal_id`, `relation`, `created_at?` (always `null` for OpenFGA direct tuples).

## Chat

| Method | Path | Headers | Request | Responses | Auth/behavior |
|---|---|---|---|---|---|
| GET | `/tenants/{tid}/chat_sessions` | `Authorization`, tenant header | none | `200 ChatSessionsResponse { sessions[] }`; `400`; `401`; `500`; `503` | Lists OpenFGA `viewer` chat sessions and intersects with RLS SQL. |
| POST | `/tenants/{tid}/chat_sessions` | `Authorization`, tenant header | `CreateChatSessionRequest { title?, workspace_id?, model? }` | `201 CreateChatSessionResponse { id }`; `400`; `401`; `403`; `404`; `500`; `503` | If `workspace_id` is present, requires workspace access. Writes OpenFGA tenant, owner, and optional workspace tuple. Empty/missing title becomes empty string. |
| DELETE | `/tenants/{tid}/chat_sessions/{sid}` | `Authorization`, tenant header | none | `204`; `400`; `401`; `403`; `404`; `500`; `503` | Owner-only via OpenFGA `owner`; deletes OpenFGA object then DB row. |
| GET | `/tenants/{tid}/chat_sessions/{sid}/messages` | `Authorization`, tenant header | none | `200 ChatMessagesResponse { messages[] }`; `400`; `401`; `404`; `500`; `503` | Requires OpenFGA `viewer`; missing or denied is `404`. Returns messages ordered by `created_at ASC`. |
| POST | `/tenants/{tid}/chat_sessions/{sid}/chat` | `Authorization`, tenant header | `PostChatRequest { message }` | `200 text/event-stream ChatSseEvent`; `400`; `401`; `404`; `429`; `500`; `503` | Rejects empty message. Requires viewer on chat session. Resolves tenant BYOK/global LLM config, inserts durable user message, retrieves RAG context if session has workspace, streams SSE, persists assistant message and usage before emitting `done`. |

`ChatSessionItem`: `id`, `title`, `workspace_id?`, `model?`, `created_at`, `updated_at`.

`ChatMessageItem`: `id`, `role`, `content`, `token_count?`, `created_at`.

SSE `ChatSseEvent` variants:
- `{"type":"text","content":string}`
- `{"type":"citation","index":number,"point_id":uuid,"document_id":uuid,"chunk_index":number,"filename":string|null,"page_start":number|null,"page_end":number|null}`
- `{"type":"citation_unknown","index":number}`
- `{"type":"done","finish_reason":string|null}`
- `{"type":"error","code":string,"message":string}` where known in-stream codes include `stream-failed` and `persist-failed`.

## Graph

| Method | Path | Headers | Request | Responses | Auth/behavior |
|---|---|---|---|---|---|
| GET | `/tenants/{tid}/workspaces/{wid}/graph?cursor=&limit=` | `Authorization`, tenant header | query `cursor?`, `limit?` | `200 WorkspaceGraphResponse { nodes[], edges[], next_cursor? }`; `400`; `401`; `404`; `500`; `503` | Requires workspace access, denied as `404`. `limit` defaults to 200 and clamps to 1..500. Cursor format is `RFC3339:UUID`. Nodes are post-filtered by document provenance visibility; `next_cursor` is based on fetched graph node ordering before provenance filtering. |

`GraphNodeItem`: `id`, `kind`, `label`, `properties`, `created_at`.

`GraphEdgeItem`: `id`, `src_node_id`, `dst_node_id`, `kind`, `weight`, `properties`, `created_at`.

## Settings

| Method | Path | Headers | Request | Responses | Auth/behavior |
|---|---|---|---|---|---|
| GET | `/tenants/{tid}/settings/llm` | `Authorization`, tenant header | none | `200 LlmSettingsResponse`; `400`; `401`; `403`; `500`; `503` | Owner-only. Reads `tenant_llm_config`; returns raw API key never, only `has_api_key` and optional masked key. |
| PUT | `/tenants/{tid}/settings/llm` | `Authorization`, tenant header | `PutLlmSettingsRequest { provider, model, base_url?, api_key?, dimensions?, enabled?, llm_model?, llm_base_url? }` | `200 LlmSettingsResponse`; `400`; `401`; `403`; `500`; `503` | Owner-only. `provider` must be `ollama` or `openai`; `model` must be non-empty; OpenAI requires `api_key`. Storing an API key requires `GMRAG_TENANT_KEY_ENCRYPTION_KEY`; key is encrypted with AES-256-GCM and stored in `api_key_ciphertext/api_key_nonce`. |

`LlmSettingsResponse`: `configured`, `provider?`, `model?`, `base_url?`, `dimensions?`, `enabled?`, `llm_model?`, `llm_base_url?`, `has_api_key`, `api_key_masked?`.

## Metering, Quotas, Audit

| Method | Path | Headers | Request | Responses | Auth/behavior |
|---|---|---|---|---|---|
| GET | `/tenants/{tid}/metering/usage` | `Authorization`, tenant header | none | `200 UsageResponse { usage[] }`; `400`; `401`; `403`; `500`; `503` | Owner-only. Aggregates `usage_events` by `metric` with `SUM(delta)`. |
| GET | `/tenants/{tid}/quotas` | `Authorization`, tenant header | none | `200 QuotaResponse`; `400`; `401`; `403`; `500`; `503` | Owner-only. If no `tenant_quotas` row exists, returns defaults: 100 documents, 10 workspaces, 10,737,418,240 bytes, 50 members, `configured=false`. |
| GET | `/tenants/{tid}/audit_logs` | `Authorization`, tenant header | none | `200 AuditLogsResponse { logs[] }`; `400`; `401`; `403`; `500`; `503` | Owner-only. Returns newest 100 audit rows. |

`UsageMetricItem`: `metric`, `total`.

`QuotaResponse`: `configured`, `max_documents`, `max_workspaces`, `max_storage_bytes`, `max_members`, `updated_at?`.

`AuditLogItem`: `id`, `actor_id?`, `action`, `resource_type?`, `resource_id?`, `metadata?`, `created_at`.

## Frontend Type Sync

`frontend/lib/api/schema.d.ts` path set matches `backend/openapi.json` exactly. It was generated by `openapi-typescript` and includes `X-Tenant-ID` for tenant-scoped header parameters. Member operation IDs are unique (`list_tenant_members`, `remove_tenant_member`, `list_workspace_members`, `remove_workspace_member`), so the generated `operations` map no longer has duplicate member keys. The runtime frontend API middleware uses `NEXT_PUBLIC_TENANT_HEADER ?? "X-Tenant-ID"` and reads the active tenant from the Zustand tenant store.
