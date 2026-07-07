# System Overview

Generated from current source on 2026-07-07.

Verified from:
- `backend/Cargo.toml`
- `backend/crates/api/Cargo.toml`
- `backend/crates/core/Cargo.toml`
- `backend/crates/worker/Cargo.toml`
- `backend/crates/api/src/lib.rs`
- `backend/crates/api/src/auth/*.rs`
- `backend/crates/api/src/middleware/*.rs`
- `backend/crates/api/src/authz.rs`
- `backend/crates/api/src/routes/*.rs`
- `backend/crates/core/src/config.rs`
- `backend/crates/core/src/db.rs`
- `backend/crates/core/src/qdrant/store.rs`
- `backend/crates/worker/src/*.rs`
- `frontend/package.json`
- `frontend/lib/api/client.ts`
- `frontend/lib/store/tenant.ts`
- `frontend/src/middleware.ts`
- `frontend/auth.ts`
- `infra/docker-compose.yml`
- `infra/openfga/model.fga`
- `.env.example`

## Repository Shape

Backend is a Rust workspace (`backend/Cargo.toml`) with three members:

| Crate | Role |
|---|---|
| `crates/core` | Shared config, DB pool helpers, crypto, Qdrant wrapper, status constants. |
| `crates/api` | Axum HTTP API, OpenAPI/Swagger, Keycloak JWT middleware, OpenFGA authorization, RLS middleware, routes, metrics, reconcile binaries. |
| `crates/worker` | Background ingestion worker, outbox relay, stuck-job sweeper, retention loop, PDF parsing, chunking, embeddings, graph extraction, Qdrant writes. |

Frontend is a Next.js app (`frontend/package.json`) using Next 16, React 19 RC, NextAuth v5, `next-intl`, Zustand, TanStack Query, `openapi-fetch`, and generated OpenAPI types in `frontend/lib/api/schema.d.ts`.

## Runtime Components

| Component | Confirmed wiring |
|---|---|
| PostgreSQL 16 | `infra/docker-compose.yml` service `postgres16`; Rust uses `sqlx`; migrations run at API boot. |
| Qdrant | Docker image `qdrant/qdrant:v1.12.4`; Rust client pinned to `qdrant-client = "=1.12.1"`; API/worker create/search/delete tenant collections. |
| MinIO/S3 | Docker service `minio`; API uses `aws-sdk-s3` through `S3ObjectStore`; document upload/delete and workspace/tenant cleanup use S3 keys/prefixes. |
| Redis | Docker service `redis:7-alpine`; API uses Redis for rate limiting and legacy `JobEnqueuer`; worker relay LPUSHes outbox payloads and worker dispatcher consumes queue. |
| OpenFGA | Docker services `openfga-migrate` and `openfga`, image `openfga/openfga:v1.18.1`; API uses `OpenFgaAuthorizationService` for checks/list/write/read. |
| Keycloak | Docker service `keycloak`, image `quay.io/keycloak/keycloak:25.0.6`; API validates JWTs through OIDC discovery/JWKS; frontend uses NextAuth OIDC provider. |
| Ollama | Docker service `ollama`; worker/API config include `OLLAMA_HOST`, `OLLAMA_EMBED_MODEL`, `OLLAMA_LLM_MODEL`. |
| DeepSeek/OpenAI BYOK | Configured in `Config` and `tenant_llm_config`; chat/settings/worker paths use tenant/global LLM configuration. |

## Backend Boot Flow

`gmrag_api::run()` does the following:

1. Loads `Config::from_env()`.
2. Parses `GMRAG_TENANT_HEADER` into `TenantHeaderName`; default is `X-Tenant-ID`.
3. Creates `AdminPool` with `init_pool()` for migrations and platform operations.
4. Creates `AppPool` with `init_app_pool()`, which runs `SET ROLE gmrag_app` on connections so RLS is enforced.
5. Runs migrations from `backend/migrations`.
6. Initializes `QdrantStore`.
7. Initializes `OpenFgaAuthorizationService` and checks OpenFGA health.
8. Initializes S3 object store, Redis enqueuer, Redis rate limiter, vector/graph cleaners, LLM runtime, JWT validator.
9. Builds routers: public, authenticated pre-tenant, and tenant-scoped.

Public routes: `/health`, `/healthz`, `/metrics`, `/swagger`, `/openapi.json`.

Authenticated pre-tenant routes: `/users/me`, `/tenants`.

Tenant-scoped routes: `/tenants/:tid/...` resources for tenants, members, workspaces, documents, ACL, chat, graph, settings, usage, quotas, and audit logs.

## Auth Flow

Frontend:

1. `frontend/auth.ts` configures NextAuth with Keycloak OIDC.
2. It separates server issuer (`KEYCLOAK_ISSUER`) from browser/public issuer (`KEYCLOAK_ISSUER_PUBLIC` or `NEXT_PUBLIC_KEYCLOAK_URL/...`).
3. The JWT callback stores `account.access_token` in the session.
4. `frontend/lib/api/client.ts` attaches `Authorization: Bearer <token>` when `getClientToken()` returns a token.

Backend:

1. `auth_middleware` requires `Authorization: Bearer <token>`.
2. It validates JWT through `JwtValidator`, parses `sub` as UUID, and provisions the user row via `AdminPool`.
3. Tenant routes then run `tenant_middleware`, which reads the configured tenant header, parses a UUID, checks tenant existence with `AdminPool`, and checks OpenFGA `member` on `tenant:{tid}`.
4. Tenant-scoped handlers also call `ensure_path_matches_context(tid, ctx)` so the `{tid}` path cannot differ from the header-derived tenant.

## Multi-Tenancy and RLS

The active tenant is never loaded from the URL alone. The path `{tid}` is validated against `TenantContext`, and `TenantContext` is derived from the configured tenant header after auth.

Tenant-scoped handlers use `Extension<SharedConnection>` from `rls_middleware`. That middleware:

```sql
BEGIN;
SET LOCAL app.tenant_id = '<tenant_uuid>';
-- handler queries through SharedConnection
COMMIT;
```

The app pool connections already run as `gmrag_app`, so PostgreSQL RLS applies. Pre-tenant/platform operations use `AdminPool`; examples are migrations, `/users/me`, `/tenants`, auth provisioning, tenant existence checks, and some worker maintenance/reconcile paths.

## Authorization Model

Current production authorization is OpenFGA-backed; PostgreSQL remains metadata/RLS storage but is not the authorization fallback.

OpenFGA object types and relations from `infra/openfga/model.fga`:

| Type | Relations |
|---|---|
| `user` | none |
| `tenant` | `owner`, `admin`, `member = user or owner or admin` |
| `workspace` | `tenant`, `owner`, `admin`, `member = user or owner or admin`, `accessor = member or owner from tenant`, `manager = owner or admin or owner from tenant` |
| `document` | `tenant`, `workspace`, `owner`, `editor = direct user/workspace member or owner`, `viewer = direct user/workspace member/tenant member or editor or workspace member` |
| `chat_session` | `tenant`, `workspace`, `owner`, `editor = direct user/workspace member or owner`, `viewer = direct user/workspace member or editor or workspace member` |

Stored tenant/workspace membership roles are `owner`, `admin`, `member`. `admin` is a valid stored role. Current tenant-level owner-only route guards still check `owner`. Workspace manager checks grant tenant owners, workspace owners, and workspace admins; tenant admins do not receive implicit workspace authority.

## Data Flow: Document Upload and Ingest

Upload route:

1. Validates tenant path/header match.
2. Parses multipart fields `file`, `visibility`, `workspace_id`, optional `title`.
3. Requires workspace access through OpenFGA.
4. Checks quota row when present.
5. Uploads the object to S3 key `{tid}/{workspace_id}/{document_id}.pdf`.
6. Inside the RLS transaction, inserts `documents`, `ingest_jobs`, and `ingest_outbox`.
7. Writes OpenFGA tuples for document tenant, workspace, owner, and optional shared tenant-member viewer.

The handler intentionally does not LPUSH Redis. `ingest_outbox` is the transactional queue boundary.

Worker path:

1. Relay polls `ingest_outbox` and LPUSHes payloads to Redis after committed DB rows exist.
2. Dispatcher consumes Redis jobs with bounded concurrency.
3. Worker downloads the S3 object, parses PDF/text, chunks text, embeds chunks, extracts graph data, embeds graph nodes, writes Postgres rows and Qdrant points.
4. Sweeper requeues stuck `processing` jobs using `claimed_at`.
5. Retention loop deletes old dispatched outbox rows, usage events, and audit rows in bounded batches.

## Data Flow: Chat RAG

`POST /tenants/{tid}/chat_sessions/{sid}/chat`:

1. Rejects empty message.
2. Enforces tenant path/header match.
3. Applies concurrent SSE limit if rate limiting is enabled.
4. Resolves tenant/global LLM configuration.
5. Loads chat session and requires OpenFGA `viewer`; missing or denied returns `404`.
6. Inserts the user message before retrieval/LLM call.
7. If the session has `workspace_id`, retrieves accessible chunks and graph context using OpenFGA/RLS/Qdrant.
8. Loads recent chat history using `GMRAG_CHAT_HISTORY_LIMIT` (default 10).
9. Streams LLM output as SSE.
10. Persists assistant message and usage before emitting final `done`; stream or persistence failures are emitted as SSE `error` events.

## Qdrant Layout

`QdrantStore` creates two tenant-scoped collections:

| Collection | Vector config | Payload indexes |
|---|---|---|
| `chunks_{tenant_id}` | 768 dimensions, cosine | `workspace_id`, `document_id`, `chunk_index`, `filename`, `owner_id`, `visibility` |
| `graph_{tenant_id}` | 768 dimensions, cosine | `node_id`, `workspace_id`, `entity_name` |

Tenant deletion best-effort deletes both collections. Workspace deletion best-effort deletes chunk points by `workspace_id`. Document deletion best-effort deletes chunk points by `document_id`; graph nodes are deleted only when no remaining `graph_node_documents` provenance rows reference them.

## Configuration

Required backend config from `Config::from_process_env()`:

- `DATABASE_URL`
- `KEYCLOAK_ISSUER`
- `KEYCLOAK_CLIENT_ID`
- `KEYCLOAK_CLIENT_SECRET`
- `S3_ENDPOINT`
- `S3_ACCESS_KEY`
- `S3_SECRET_KEY`
- `S3_BUCKET`
- `OPENFGA_API_URL`
- `OPENFGA_STORE_ID`
- `OPENFGA_AUTHORIZATION_MODEL_ID`

Important defaults:

| Variable | Default |
|---|---|
| `GMRAG_HTTP_BIND` | `0.0.0.0:8080` |
| `GMRAG_TENANT_HEADER` | `X-Tenant-ID` |
| `QDRANT_URL` | `http://localhost:6334` |
| `QDRANT_COLLECTION_DEFAULT` | `gmrag_chunks` |
| `S3_PUBLIC_ENDPOINT` | `http://localhost:9000` |
| `S3_REGION` | `us-east-1` |
| `S3_FORCE_PATH_STYLE` | `true` |
| `REDIS_URL` | `redis://localhost:6379/0` |
| `OLLAMA_HOST` | `http://localhost:11434` |
| `OLLAMA_EMBED_MODEL` | `nomic-embed-text` |
| `OLLAMA_LLM_MODEL` | `llama3.1:8b` |
| `DEEPSEEK_BASE_URL` | `https://api.deepseek.com/v1` |
| `DEEPSEEK_MODEL` | `deepseek-v4-flash` |
| `DEEPSEEK_TIMEOUT_S` | `60` |
| `OPENFGA_REQUEST_TIMEOUT_MS` | `1500` |
| `OPENFGA_HIGHER_CONSISTENCY_WINDOW_SECS` | `5` |
| `DATABASE_MAX_CONNECTIONS` | `10` |
| `GMRAG_CHAT_HISTORY_LIMIT` | `10` |
| `GMRAG_WORKER_CONCURRENCY` | `4` |
| `GMRAG_OUTBOX_POLL_INTERVAL_SECS` | `3` |
| `GMRAG_SWEEP_INTERVAL_SECS` | `60` |
| `GMRAG_RETENTION_INTERVAL_SECS` | `86400` |
| `GMRAG_OUTBOX_RETENTION_DAYS` | `30` |
| `GMRAG_USAGE_RETENTION_DAYS` | `90` |
| `GMRAG_AUDIT_RETENTION_DAYS` | `365` |
| `GMRAG_RETENTION_BATCH_SIZE` | `1000` |
| `GMRAG_RECONCILE_INTERVAL_SECS` | `3600` |
| `GMRAG_RECONCILE_AUTO_FIX` | `false` |
| `GMRAG_RATELIMIT_ENABLED` | `true` |
| `GMRAG_RATELIMIT_AUTH_PER_MIN` | `10` |
| `GMRAG_RATELIMIT_JOB_CREATE_PER_MIN` | `20` |
| `GMRAG_RATELIMIT_CHAT_CREATE_PER_MIN` | `30` |
| `GMRAG_RATELIMIT_CHAT_CONCURRENT_PER_TENANT` | `50` |
| `GMRAG_RATELIMIT_GENERAL_PER_MIN` | `300` |
| `GMRAG_WORKER_METRICS_BIND` | `0.0.0.0:9091` |

Frontend config confirmed in `frontend/lib/api/client.ts`, `frontend/auth.ts`, and `.env.example`:

- `NEXT_PUBLIC_API_BASE_URL`
- `NEXT_PUBLIC_TENANT_HEADER`
- `NEXT_PUBLIC_KEYCLOAK_URL`
- `NEXT_PUBLIC_KEYCLOAK_REALM`
- `NEXT_PUBLIC_KEYCLOAK_CLIENT_ID`
- `KEYCLOAK_ISSUER`
- `KEYCLOAK_ISSUER_PUBLIC`
- `KEYCLOAK_FRONTEND_CLIENT_SECRET`
- `AUTH_SECRET`
- `AUTH_TRUST_HOST`
- `AUTH_URL`

## Current Drift From Older Architecture Docs

- Old docs described `SET LOCAL app.current_tenant_id`; current code and migrations use `SET LOCAL app.tenant_id` and `gmrag_current_tenant()`.
- Old docs described ReBAC tuples in PostgreSQL `relation_tuples`; current production authorization uses OpenFGA. The old `resource_acl` table is dropped, and direct grants live in OpenFGA.
- Old docs described `resource_acl`/`/acl/grants` route shapes; current routes are `/tenants/{tid}/acl` and `/tenants/{tid}/acl/{grant_id}`.
- Old docs listed workspace/document/graph columns not present in migrations, including `description`, `size_bytes`, `qdrant_point_id` on graph nodes, and `payload_json`/`dispatched` on outbox.
- Old docs listed usage/quota paths as `/usage` and `/quota`; OpenAPI/current routes are `/metering/usage` and `/quotas`.
- Old docs omitted OpenFGA as a locked-in infra dependency; current docker-compose and API boot require OpenFGA.
