# PHASE 5 Report - Tenant Header Config Fix & Schema Sync

## Summary

Fixed the tenant-header config/code mismatch: tenant resolution now reads the configured `GMRAG_TENANT_HEADER` value via an injected `TenantHeaderName` instead of a hardcoded `x-tenant-id` literal.

Also regenerated the backend OpenAPI artifact at `backend/openapi.json` and regenerated `frontend/lib/api/schema.d.ts` from that artifact with the existing `openapi-typescript` dependency.

## Tenant Header Fix

Before:
- `Config::tenant_header` parsed `GMRAG_TENANT_HEADER`, but `tenant_middleware` read only hardcoded `x-tenant-id`.

After:
- API startup parses `cfg.tenant_header` into an HTTP `HeaderName`.
- `tenant_middleware` reads that configured header from request extensions.
- Invalid configured header names fail at startup.
- The CORS default allowed headers now include the configured tenant header instead of hardcoding `X-Tenant-ID`.

Regression evidence:
- `cargo test -p gmrag-api tenant_middleware_honors_configured_header_and_ignores_default`
- Result: `1 passed; 0 failed`
- The test configures `x-gmrag-tenant`, verifies that header resolves the tenant, and verifies old `x-tenant-id` is rejected.

Other hardcoded-config instances checked/fixed:
- Fixed the adjacent CORS default header list so browser requests using a custom tenant header are allowed by default.
- No other runtime tenant-header hardcoding was found in `auth/tenant.rs`.

## OpenAPI And Frontend Schema

Generated artifact:
- `backend/openapi.json`

Generated frontend schema:
- `frontend/lib/api/schema.d.ts`

OpenAPI counts:
- 35 operations
- 25 paths
- `backend/crates/api/tests/openapi.rs`: `6 passed; 0 failed`

Requested route-area coverage in generated spec:
- Chat: present (`/tenants/{tid}/chat_sessions`, session delete, messages, chat SSE)
- Graph: present (`/tenants/{tid}/workspaces/{wid}/graph`)
- Settings: present (`/tenants/{tid}/settings/llm`)
- Members: present for tenant and workspace members
- Quota: present (`/tenants/{tid}/quotas`)
- Audit: present (`/tenants/{tid}/audit_logs`)

Frontend schema drift found and aligned:
- Tenant roles now include `admin`; frontend tenant store now derives membership type from generated schema.
- Generated schema uses standard `components["schemas"]` exports; stale `components_schemas` imports were updated.
- Document delete path is `/tenants/{tid}/documents/{did}`, not `{doc_id}`.
- Tenant-scoped frontend calls now pass generated required `X-Tenant-Id` header params.
- Generated `DocumentsResponse` does not include `filename`; the stale filename table column was removed.
- Multipart upload still sends `FormData`; a narrow type cast remains because generated multipart binary fields type as strings.

## Verification

Commands run:
- `cargo test -p gmrag-api tenant_middleware_honors_configured_header_and_ignores_default`
- `cargo test -p gmrag-api --test openapi`
- `pnpm build`
- `cargo test --workspace`

Results:
- Tenant-header regression: `1 passed; 0 failed`
- OpenAPI test: `6 passed; 0 failed`
- Frontend build: passed. Next emitted its existing `middleware` deprecation warning and an `ENVIRONMENT_FALLBACK` log during static generation, but exited successfully.
- Full backend workspace: `410 passed; 0 failed; 0 ignored`

Notes:
- Rust tests were run with a host-local `DATABASE_URL` pointing at the running Docker PostgreSQL service on `localhost:5432`.
- No `SQLX_OFFLINE` and no `--no-run` were used.
