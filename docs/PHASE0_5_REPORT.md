# Phase 0.5 — OpenFGA v1.18.1 Response Compatibility + Bootstrap Gaps

Date: 2026-07-06

## Summary

All four live blockers from the Phase 0 verification report are resolved.
A fresh clone + reused-volume run now reaches a working end-to-end state
(tenant → workspace → PDF upload → indexed → streamed chat `done`) without
any manual patching. Phase 1 scope (worker concurrency, sweeper, retry logic)
was not touched.

Go / No-Go: **GO for Phase 1.**

---

## Bug 1 — OpenFGA check response deserialization (BLOCKING, fixed)

### Before

OpenFGA v1.18.1 returns `{"allowed":true,"resolution":""}` from
`POST /stores/:id/check`. The Rust `Response` struct in
`backend/crates/api/src/authz.rs` was annotated `#[serde(deny_unknown_fields)]`,
so any unknown top-level field (like `resolution`) caused deserialization to
fail with:

```text
authorization response malformed: unknown field `resolution`, expected `allowed` at line 1 column 28
```

The API then failed closed with `authorization-unavailable` (503) on **every**
authz check, blocking workspace authorization, document upload, and browser
E2E.

### After

The `check` and `list-objects` `Response` structs no longer use
`deny_unknown_fields`. The check `Response` explicitly captures `resolution`
as an optional ignored field plus `allowed`; the list-objects `Response`
parses `objects` and silently ignores any other future top-level keys (the
default serde behavior). The `read` response already tolerated unknown fields
(no `deny_unknown_fields`) — verified, left untouched.

Diff (code):

```rust
// check response
#[derive(Deserialize)]
struct Response {
    #[allow(dead_code)]
    #[serde(default)]
    resolution: Option<String>,
    allowed: bool,
}

// list-objects response
#[derive(Deserialize)]
struct Response {
    objects: Vec<String>,
}
```

The fix is defensive and forward-compatible: it does not pin the struct to
exactly today's response shape, so a future OpenFGA release adding more
top-level fields will not break the API again.

### Regression tests

Four new tests added in `authz::tests` (`backend/crates/api/src/authz.rs`):

```text
test authz::tests::check_response_tolerates_resolution_field ... ok
test authz::tests::check_response_tolerates_unknown_future_fields ... ok
test authz::tests::check_response_parses_minimal_shape ... ok
test authz::tests::list_objects_response_tolerates_unknown_fields ... ok
```

Each deserializes a representative OpenFGA response (including the exact live
v1.18.1 shape, a hypothetical future shape with extra keys, and the minimal
shape) and asserts the meaningful field still parses. All 8 authz tests pass:

```text
running 8 tests
test authz::tests::check_response_parses_minimal_shape ... ok
test authz::tests::check_response_tolerates_resolution_field ... ok
test authz::tests::check_response_tolerates_unknown_future_fields ... ok
test authz::tests::list_objects_response_tolerates_unknown_fields ... ok
test authz::tests::typed_uuid_rejects_wrong_type_and_userset ... ok
test authz::tests::malformed_grant_id_is_rejected ... ok
test authz::tests::workspace_member_principal_parses ... ok
test authz::tests::grant_id_round_trips ... ok
test result: ok. 8 passed; 0 failed; 0 ignored
```

### Live evidence

After the fix, the API log shows OpenFGA checks completing `200 OK` instead of
failing closed:

```text
openfga request completed status=200 OK path="/stores/.../list-objects"
openfga request completed status=200 OK path="/stores/.../write"
openfga request completed status=200 OK path="/stores/.../check"
```

`GET /tenants` previously returned HTTP 503 `authorization-unavailable`; it now
returns HTTP 200 with the tenant list (see E2E smoke test below).

---

## Bug 2 — Local dev environment bootstrap gaps

### 2.1 `.env.example` missing OpenFGA runtime keys

Already present in the working tree from Phase 0 (added
`OPENFGA_DATASTORE_URI`, `OPENFGA_API_URL`, `OPENFGA_API_TOKEN`,
`OPENFGA_STORE_NAME`, `OPENFGA_STORE_ID` + `OPENFGA_AUTHORIZATION_MODEL_ID`
placeholders, timeout/consistency vars). Phase 0.5 added a clear **bootstrap
ordering** comment block above the OpenFGA section so the operator knows the
`ensure-openfga-db` → `openfga-migrate` → `openfga-bootstrap.ps1` sequence and
why the store/model IDs must be pasted in.

### 2.2 `openfga` database missing on reused volumes

`infra/postgres/init.sql` only runs on first volume init via
`docker-entrypoint-initdb.d`. On a reused `gmrag-pgdata` volume the `openfga`
database silently does not exist, and `openfga-migrate` fails.

Fix: new idempotent script `scripts/ensure-openfga-db.ps1`. It runs `psql`
inside the postgres container with the same `CREATE DATABASE ... WHERE NOT
EXISTS \gexec` idiom as `init.sql`, so it is safe to re-run. README Quick
Start documents running it **before** `openfga-migrate`:

```text
[ensure-openfga-db] ensuring database 'openfga' exists in container 'gmrag-postgres16' (owner=gmrag)
[ensure-openfga-db] OK — 'openfga' database is present.
```

### 2.3 Keycloak `gmrag-backend` audience mapper

Before: the backend client was created confidential + service-accounts-enabled
but with no audience mapper, so service-account tokens carried no `aud` claim
the backend JWT validator would accept — `InvalidAudience`.

After: `infra/keycloak/bootstrap.sh` now resolves the backend client UUID
(whether pre-existing or just created) and attaches an
`oidc-audience-mapper` named `aud-gmrag-backend` idempotently (skips if the
mapper already exists by name). The mapper forces `aud=<BACKEND_CLIENT>` into
the access token. `BACKEND_AUDIENCE` env override is supported.

Live token decode after the mapper exists:

```text
iss= http://localhost:8080/realms/gmrag
aud= ['gmrag-backend', 'account']
azp= gmrag-backend
sub= e641152a-fc64-48ee-90d5-b9f5d3ffcfc4
```

`gmrag-backend` is present in `aud`, so JWT validator accepts the token.

### 2.4 Ollama models + chat/graph LLM provider

Ollama ships empty; without a pulled embed model the worker embedding call
404s, and without a chat/graph provider the chat and graph extraction paths
cannot run.

Fix: new script `scripts/setup-ollama.ps1` pulls:
- `OLLAMA_EMBED_MODEL` (`nomic-embed-text`) — always required;
- `OLLAMA_LLM_MODEL` (`llama3.1:8b`) — only when `DEEPSEEK_API_KEY` is empty.

When `DEEPSEEK_API_KEY` is set, chat + graph go to DeepSeek and only the embed
model is required locally. `.env.example` and README document this contract
clearly.

Live evidence (this run used DeepSeek for chat/graph, Ollama-only for embedding):

```text
$ curl http://localhost:11434/api/tags  (before)
{"models":[]}

$ pwsh ./scripts/setup-ollama.ps1   (equivalently: docker exec gmrag-ollama ollama pull nomic-embed-text)
pulling 970aa74c0a90... 100%   274 MB
writing manifest
success

$ curl http://localhost:11434/api/tags  (after)
{"models":[{"name":"nomic-embed-text:latest", ...}]}
```

Worker embedding succeeded on the first attempt:

```text
processing job job_id=cd581562-... tenant_id=78d96c46-...
job completed job_id=cd581562-... attempt=0
```

---

## Documentation updates

- `.env.example`: added bootstrap-ordering comment block for OpenFGA, Ollama
  pull instructions, and Keycloak audience-mapper note pointing at
  `bootstrap.sh`.
- `README.md` Quick Start: added Bước 3b (OpenFGA bootstrap), Bước 3c (Ollama
  model pull), Bước 3d (Keycloak realm + audience mapper), and a volume-reuse
  warning about `init.sql` vs `ensure-openfga-db.ps1`.
- `scripts/ensure-openfga-db.ps1`: new idempotent ensure-DB script.
- `scripts/setup-ollama.ps1`: new Ollama model pull script.

---

## E2E smoke test — PASS

Run against the live stack (postgres, qdrant, minio, redis, openfga, keycloak,
ollama — all the containers from Phase 0 plus `nomic-embed-text` pulled).
API + worker run on the host with `KEYCLOAK_ISSUER=http://localhost:8080/...`
and `DATABASE_URL=...@localhost:5432/...` overrides (host-side smoke). DeepSeek
provided chat + graph; Ollama provided embedding.

| Step | Endpoint / action | Result |
|------|-------------------|--------|
| 1. Health | `GET /health`, `GET /healthz` | 200 `{"db":"ok","openfga":"ok","status":"ready"}` |
| 2. Auth | Keycloak client_credentials → token | 200, `aud=gmrag-backend` |
| 3. Tenant list | `GET /tenants` | **200** (was 503 `authorization-unavailable` before fix) |
| 4. Create tenant | `POST /tenants` | 200 `id=78d96c46-...` |
| 5. Create workspace | `POST /tenants/:tid/workspaces` | 200 `id=7e469a1f-...` |
| 6. Upload PDF | `POST /tenants/:tid/documents` (multipart) | 200 `id=53e88748-...` |
| 7. Index | worker `processing job` → `job completed attempt=0` | `status=indexed` |
| 8. Create chat session | `POST /tenants/:tid/chat_sessions` | 200 `id=97e7e475-...` |
| 9. Streamed chat | `POST /tenants/:tid/chat_sessions/:sid/chat` | streamed `text` chunks + `citation` + `done` |

Chat SSE terminal events (conclusive):

```text
data: {"type":"citation","index":1,"point_id":"56941fa5-...","document_id":"53e88748-...","chunk_index":0,"filename":"phase0-smoke.pdf","page_start":1,"page_end":1}
data: {"type":"done","finish_reason":"stop"}
```

`"type":"done"` present → streaming completed cleanly.

---

## Scope adherence

- No changes to Phase 1 scope (worker concurrency, sweeper, retry logic).
- The OpenFGA struct fix is defensive/forward-compatible, not a one-off patch
  tied to exactly today's response shape: unknown top-level fields are
  ignored, and the regression test suite covers a hypothetical future shape
  with extra fields.

## Go / No-Go

**GO for Phase 1.**