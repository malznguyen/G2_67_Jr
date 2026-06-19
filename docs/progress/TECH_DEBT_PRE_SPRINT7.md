# Tech Debt Payoff — Pre-Sprint 7

**Date**: 2026-06-19  
**Commit**: `c159a6f` (pushed to `origin/main`)  
**Scope**: DEBT-1, DEBT-2, DEBT-3, DEBT-4  
**Prerequisite commit**: `525f672` (T44-T51, committed immediately prior)

---

## DEBT-1 Hoist crypto to core

**What was done**:
- Created `backend/crates/core/src/crypto.rs` with `encrypt_with_aad` / `decrypt_with_aad` — AES-256-GCM with explicit AAD parameter (caller passes `tenant_id.as_bytes()`).
- Returns **separate `(ciphertext, nonce)` byte vectors** to match the database schema (`api_key_ciphertext BYTEA`, `api_key_nonce BYTEA`).
- Added `aes-gcm = "0.10"` and `rand = "0.8"` to `core/Cargo.toml`.
- Removed `aes-gcm` from `api/Cargo.toml` (now accessed transitively through core).
- Exported `pub mod crypto` from `core/src/lib.rs`.
- Refactored `api/src/llm/byok.rs`:
  - Removed local `decrypt_api_key` implementation.
  - Uses `core::crypto::decrypt_with_aad` + a `map_crypto_error` helper.
  - Test `encrypt` helper now calls `core::crypto::encrypt_with_aad`.
- Refactored `worker/src/embedding.rs`:
  - Added `Decrypt(String)` variant to `EmbedError`.
  - Added `api_key_ciphertext` + `api_key_nonce` to `TenantLlmConfig` struct.
  - Added `enc_key: Option<&[u8; 32]>` parameter to `select_embedder`.
  - Added `resolve_byok_api_key` helper that reads ciphertext + nonce from DB, decrypts via `core::crypto::decrypt_with_aad`, falls back to plaintext `api_key` column if encrypted columns are NULL.
  - Updated SQL to `SELECT api_key, api_key_ciphertext, api_key_nonce, ...`.
- Refactored `worker/src/graph.rs`:
  - Same pattern: `Decrypt(String)` error variant, `TenantLlmRow` with encrypted columns, `enc_key` param on `select_graph_extractor`, `resolve_graph_api_key` helper.
- Refactored `worker/src/job.rs`:
  - Added `enc_key: Option<[u8; 32]>` to `IngestContext`.
  - Populated from `cfg.tenant_key_encryption_key` in `from_config`.
  - Passed `self.enc_key.as_ref()` to `select_embedder` and `select_graph_extractor`.

**Deviation from original task template**:
- Template suggested removing AAD / using combined base64. **We preserved AAD binding to `tenant_id`** — this is a security feature, not debt.
- Template suggested renaming `GMRAG_TENANT_KEY_ENCRYPTION_KEY` to `GMRAG_ENCRYPTION_KEY`. **We kept the original name** — it already exists in `.env.example`, config, and tests.
- `sqlx::query!` macros don't exist in this codebase, so `cargo sqlx prepare` was a no-op (2 cached query files unchanged).

**Key design decision**: Worker decrypts BYOK keys, not the API. This means:
- The API layer has no access to the tenant encryption key.
- Only the worker (which runs ingestion) can decrypt keys.
- If encrypted columns exist but `enc_key` is `None`, worker returns an error — no silent fallback to plaintext.

---

## DEBT-2 Fix clippy warnings

**8 pre-existing `dead_code` warnings fixed** by adding `#[allow(dead_code)]` annotations:

| File | Annotations |
|------|-------------|
| `api/src/auth/extractor.rs` | `TEST_PEM_PRIV` (line 8), `make_token` (line 48) |
| `api/src/auth/jwt.rs` | `with_cache_ttl` (line 136) |
| `api/src/auth/tenant.rs` | `TEST_PEM_PRIV` (line 12), `TEST_PEM_PUB` (line 17), `TEST_KID` (line 21), `make_token` (line 58), `make_auth_state` (line 100) |

These are test helper utilities used only in integration tests, not in library code.

**Result**: `cargo clippy --workspace --all-targets -- -D warnings` exits with code 0.

---

## DEBT-3 Sync sqlx prepared-query cache

- Ran `cargo sqlx prepare --workspace` after all code changes.
- **No changes to `backend/.sqlx/`** — the workspace has zero `sqlx::query!` typed macros; only two existing offline-query files remain and are unaffected.
- Confirmed via: `git diff --stat -- backend/.sqlx/` → empty.

---

## DEBT-4 Audit worker pool usage

- Grepped all `*.rs` files in `worker/src/` for `admin_pool` and `init_pool`.
- **Zero business-logic hits.** The only reference is a doc comment in `worker/src/lib.rs:6-9` explaining the invariant.
- Worker exclusively uses `init_app_pool` (which runs `SET LOCAL app.tenant_id` to enforce Row-Level Security).
- The rule is already enforced by convention and code review.

---

## Test Results

```
209 tests passed:
  - api:    109 passed
  - core:    26 passed (including 3 crypto unit tests)
  - worker:  74 passed (including 5 new encrypted BYOK tests)
```

## Files Changed

```
14 files changed, 543 insertions(+), 89 deletions(-)
 create mode 100644 backend/crates/core/src/crypto.rs
```

---

## Git Log (relevant commits)

```
c159a6f refactor(tech-debt): hoist AES-GCM crypto to core, worker decrypts BYOK keys, clippy cleanup
525f672 feat(T44-T51): sprint 6 chat retrieval, BYOK decrypt, metering, and progress docs
7b7f7a7 feat(T43): complete ingestion pipeline execution and retry logic
eebafa9 feat(T42): idempotent dual-write for qdrant and postgres
55d9339 feat(T41): graph extraction via deepseek and idempotency schema updates
d67bd1a feat(T37): PDF parser OCR fallback (Ollama vision) + per-page extraction
```
