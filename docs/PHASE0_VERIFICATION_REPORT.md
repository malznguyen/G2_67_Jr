# Phase 0 Verification Report

Date: 2026-07-06

## Summary

Recommendation: **NO-GO for Phase 1** until the live OpenFGA response parsing bug is fixed. The stack boots and tests pass, but the real HTTP upload/workspace path fails closed against live OpenFGA v1.18.1.

## 1. Bring Up Infra - PASS after env/bootstrap fixes

Command:

```powershell
docker compose --env-file .env -f infra/docker-compose.yml up -d --force-recreate postgres16 qdrant minio redis openfga-migrate openfga ollama keycloak
```

Result excerpt:

```text
gmrag-openfga      Up ... (healthy)  0.0.0.0:8089->8080/tcp
gmrag-postgres16   Up ... (healthy)  0.0.0.0:5432->5432/tcp
gmrag-redis        Up ... (healthy)  0.0.0.0:6379->6379/tcp
gmrag-minio        Up ... (healthy)  0.0.0.0:9000-9001->9000-9001/tcp
gmrag-ollama       Up ... (healthy)  0.0.0.0:11434->11434/tcp
gmrag-qdrant       Up ... (healthy)  0.0.0.0:6333-6334->6333-6334/tcp
gmrag-keycloak     Up ... (healthy)  0.0.0.0:8080->8080/tcp
```

Live-only setup issues found and fixed for this run:

- `.env` did not include `OPENFGA_*` values needed by compose. Injected local dev values for this verification run.
- Existing Postgres volume skipped `infra/postgres/init.sql`, so the `openfga` database was missing. Created the local `openfga` database.

## 2. Run Real Migrations - PASS with caveat

Command:

```powershell
cargo sqlx migrate run
```

Result excerpt:

```text
Applied 20260624020000/migrate phase 1 security authorization
Applied 20260702000000/migrate openfga cutover drop resource acl
```

The existing database already had 20 migrations recorded before this run; this run applied the remaining 2. Final `_sqlx_migrations` count is 22/22 successful.

RLS verification as `gmrag_app`:

```text
without app.tenant_id:
tenants_visible = 0
workspaces_visible = 0

with app.tenant_id:
tenants_visible = 1
workspaces_visible = 1
```

## 3. Bootstrap OpenFGA - PASS

Bootstrap command used the repo script via the official OpenFGA CLI container against local `gmrag-openfga`:

```powershell
.\scripts\openfga-bootstrap.ps1
```

Result excerpt:

```text
OPENFGA_STORE_ID=01KWV24CRQT27N44XFQQCXYKCK
OPENFGA_AUTHORIZATION_MODEL_ID=01KWV24EH82RX67WSBFY55SX55
```

Model tests:

```text
Tests 7/7 passing
Checks 43/43 passing
ListObjects 2/2 passing
```

API authz health was confirmed by running `openfga_backfill --dry-run`, which constructs `OpenFgaAuthorizationService` and calls `authz.health()`:

```text
dry-run: would write 21 OpenFGA tuples
deduped_total: 21
```

## 4. Run Real Test Suites - PASS after syntax fix

Command:

```powershell
cargo test --workspace
```

Initial blocker:

```text
error: expected one of `,`, `:`, or `}`, found `s3_key`
crates\worker\src\sweeper.rs:209
document_id: Uuid::nil(),no
```

Fix applied: removed the stray `no` token in `backend/crates/worker/src/sweeper.rs`.

Final result excerpt:

```text
gmrag_api: 99 passed
API integration suites: all passed
gmrag_core: 27 passed
gmrag_worker: 58 passed
worker integration suites: all passed
Doc-tests gmrag_api/core/worker: all passed
```

No runtime test failures remained.

## 5. Boot API and Worker Live - PASS

Started:

```text
gmrag-api.exe bind=127.0.0.1:8088
gmrag-worker.exe
```

Health checks:

```text
GET /health  -> HTTP/1.1 200 OK
{"service":"gmrag-api","status":"ok",...}

GET /healthz -> HTTP/1.1 200 OK
{"db":"ok","openfga":"ok","status":"ready"}
```

OpenAPI:

```text
paths=25 operations=35
```

This matches `backend/crates/api/tests/openapi.rs`.

## 6. End-to-End Smoke Test - FAIL / BLOCKED

Completed:

- Created a valid one-page PDF fixture: `backend/target/phase0-logs/phase0-smoke.pdf`.
- Keycloak token issuance works after adding a local `gmrag-backend` audience mapper.
- Authenticated `GET /tenants` returns `200`.
- `POST /tenants` created tenant `d2def7dc-226e-45a9-be84-1798f208a0f4`.

Blocked at workspace/upload authorization:

```text
upload failed:
{"error":{"code":"authorization-unavailable","message":"authorization unavailable: authorization response malformed: unknown field `resolution`, expected `allowed` at line 1 column 28"}}
```

API log excerpt:

```text
openfga request completed status=200 OK path="/stores/.../check"
authorization check failed closed error=authorization response malformed:
unknown field `resolution`, expected `allowed` at line 1 column 28
```

OpenFGA log excerpt confirms v1.18.1 returns:

```json
{"allowed":true,"resolution":""}
```

Additional live blocker: Ollama is healthy but has no models installed:

```json
{"models":[]}
```

So even after the OpenFGA parse bug is fixed, indexing/chat will still need `nomic-embed-text` and a working graph/chat provider (`DeepSeek` key or local equivalent).

## 7. Frontend Live Build / Browser Check - BUILD PASS, E2E BLOCKED

Commands:

```powershell
pnpm install
pnpm build
```

Result excerpt:

```text
pnpm install: Already up to date
next build: Compiled successfully
Finished TypeScript
Route (app) generated successfully
```

Warnings/non-fatal output:

```text
The "middleware" file convention is deprecated. Please use "proxy" instead.
Error: ENVIRONMENT_FALLBACK
```

`pnpm build` exited 0.

Dev server:

```text
Next.js ready at http://localhost:3000
GET /api/health -> HTTP/1.1 200 OK
{"status":"ok","service":"gmrag-frontend"}
```

Browser check:

```text
http://localhost:3000/en/login
visible text includes:
GMRAG2
Sign in to continue
Sign in with Keycloak
```

The requested browser upload/delete flow is blocked by the backend OpenFGA `check` response parsing failure in Step 6.

## Live-Only Bugs / Blockers

1. **OpenFGA v1.18.1 check response is incompatible with API deserialization.**
   - Live response includes `resolution`.
   - API treats it as malformed and fails closed with `authorization-unavailable`.
   - Blocks workspace authorization, document upload, and browser document E2E.

2. **Local `.env` is missing required OpenFGA compose/runtime keys.**
   - `OPENFGA_DATASTORE_URI`, `OPENFGA_API_URL`, `OPENFGA_API_TOKEN`, store/model IDs were absent.

3. **Existing Postgres volume did not have the `openfga` database.**
   - `infra/postgres/init.sql` only runs on first volume initialization.

4. **Keycloak backend client did not emit `aud=gmrag-backend`.**
   - Service-account token initially failed API auth with `InvalidAudience`.
   - Added a local hardcoded audience mapper for this verification run.

5. **Ollama has no models installed.**
   - `nomic-embed-text` is required for indexing.
   - Chat/graph extraction also needs a valid DeepSeek key or local model path.

6. **Worker test syntax typo blocked initial test compilation.**
   - Fixed `document_id: Uuid::nil(),no` to `document_id: Uuid::nil(),`.

## Go / No-Go

**NO-GO for Phase 1.** Fix Phase 0 blockers first:

- Allow/ignore OpenFGA `resolution` in check responses.
- Commit or document required local OpenFGA env/bootstrap values.
- Make Keycloak bootstrap create the backend audience mapper.
- Ensure local model/bootstrap instructions cover `nomic-embed-text` and the graph/chat provider.
