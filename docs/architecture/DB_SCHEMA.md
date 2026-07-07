# Database Schema

Generated from current source on 2026-07-07.

Regenerated after the API contract drift fixes on 2026-07-07; the migration-backed table/column schema did not change.

Verified from:
- every SQL file in `backend/migrations/`
- `backend/crates/api/src/routes/*.rs`
- `backend/crates/api/src/auth/tenant.rs`
- `backend/crates/api/src/middleware/rls.rs`
- `backend/crates/api/src/reconcile/backfill.rs`
- `backend/crates/worker/src/*.rs`
- `backend/crates/core/src/status.rs`

## Foundation

The migration set creates `pgcrypto`, role `gmrag_app`, and function:

```sql
CREATE OR REPLACE FUNCTION gmrag_current_tenant()
RETURNS uuid
LANGUAGE sql
STABLE
AS $$
  SELECT NULLIF(current_setting('app.tenant_id', true), '')::uuid
$$;
```

`init_app_pool()` runs `SET ROLE gmrag_app` on connections. `rls_middleware` starts a transaction and runs `SET LOCAL app.tenant_id = '<tenant_uuid>'`; all tenant-scoped RLS policies compare rows to `gmrag_current_tenant()`.

## Current Tables

`resource_acl` is not a current table. It was created in `20260617144756_acl.sql`, constrained in `20260622000000_rebac_relation_tuples.sql`, and dropped in `20260702000000_openfga_cutover_drop_resource_acl.sql`.

### users

Global table. No RLS policy in migrations.

| Column | Type | Null | Default | Constraints |
|---|---|---:|---|---|
| `id` | `UUID` | no | `gen_random_uuid()` | primary key |
| `email` | `TEXT` | no | | unique |
| `name` | `TEXT` | no | `''` | |
| `created_at` | `TIMESTAMPTZ` | no | `now()` | |

Indexes: `idx_users_email(email)`.

### tenants

Tenant-scoped by `id`.

| Column | Type | Null | Default | Constraints |
|---|---|---:|---|---|
| `id` | `UUID` | no | `gen_random_uuid()` | primary key |
| `name` | `TEXT` | no | | |
| `created_at` | `TIMESTAMPTZ` | no | `now()` | |

RLS: enabled and forced. Policy `tenant_isolation`: `USING (id = gmrag_current_tenant()) WITH CHECK (id = gmrag_current_tenant())`.

### tenant_members

Tenant-scoped.

| Column | Type | Null | Default | Constraints |
|---|---|---:|---|---|
| `tenant_id` | `UUID` | no | | FK `tenants(id) ON DELETE CASCADE`; primary key with `user_id` |
| `user_id` | `UUID` | no | | FK `users(id) ON DELETE CASCADE`; primary key with `tenant_id` |
| `role` | `TEXT` | no | `'member'` | check `role IN ('owner','admin','member')` |

Indexes: `idx_tenant_members_user(user_id)`.

RLS: enabled and forced. Policy `tenant_members_isolation`: `USING (tenant_id = gmrag_current_tenant()) WITH CHECK (tenant_id = gmrag_current_tenant())`.

### platform_admins

Global table. No RLS policy in migrations.

| Column | Type | Null | Default | Constraints |
|---|---|---:|---|---|
| `user_id` | `UUID` | no | | primary key; FK `users(id) ON DELETE CASCADE` |
| `granted_at` | `TIMESTAMPTZ` | no | `now()` | |

### workspaces

Tenant-scoped.

| Column | Type | Null | Default | Constraints |
|---|---|---:|---|---|
| `id` | `UUID` | no | `gen_random_uuid()` | primary key |
| `tenant_id` | `UUID` | no | | FK `tenants(id) ON DELETE CASCADE`; unique with `slug` |
| `name` | `TEXT` | no | | |
| `slug` | `TEXT` | no | | unique with `tenant_id` |
| `created_by` | `UUID` | no | | FK `users(id)` |
| `created_at` | `TIMESTAMPTZ` | no | `now()` | |

Indexes: `idx_workspaces_tenant(tenant_id)`. Constraint: `UNIQUE (tenant_id, slug)`.

RLS: enabled and forced. Policy `workspaces_isolation`: `USING (tenant_id = gmrag_current_tenant()) WITH CHECK (tenant_id = gmrag_current_tenant())`.

### workspace_members

Tenant-scoped.

| Column | Type | Null | Default | Constraints |
|---|---|---:|---|---|
| `workspace_id` | `UUID` | no | | FK `workspaces(id) ON DELETE CASCADE`; primary key with `user_id` |
| `tenant_id` | `UUID` | no | | FK `tenants(id) ON DELETE CASCADE` |
| `user_id` | `UUID` | no | | FK `users(id) ON DELETE CASCADE`; primary key with `workspace_id` |
| `role` | `TEXT` | no | `'member'` | check `role IN ('owner','admin','member')` |
| `created_at` | `TIMESTAMPTZ` | no | `now()` | |

Indexes: `idx_workspace_members_user(user_id)`.

RLS: enabled and forced. Policy `workspace_members_isolation`: `USING (tenant_id = gmrag_current_tenant()) WITH CHECK (tenant_id = gmrag_current_tenant())`.

### documents

Tenant-scoped.

| Column | Type | Null | Default | Constraints |
|---|---|---:|---|---|
| `id` | `UUID` | no | `gen_random_uuid()` | primary key |
| `tenant_id` | `UUID` | no | | FK `tenants(id) ON DELETE CASCADE` |
| `workspace_id` | `UUID` | yes | | FK `workspaces(id) ON DELETE CASCADE` |
| `owner_id` | `UUID` | no | | FK `users(id)` |
| `title` | `TEXT` | no | | |
| `status` | `TEXT` | no | `'uploaded'` | check `status IN ('uploaded','processing','indexed','failed')` |
| `visibility` | `TEXT` | no | `'private'` | |
| `share_token` | `UUID` | yes | | |
| `mime_type` | `TEXT` | yes | | |
| `byte_size` | `BIGINT` | no | `0` | |
| `s3_key` | `TEXT` | yes | | |
| `created_at` | `TIMESTAMPTZ` | no | `now()` | |
| `updated_at` | `TIMESTAMPTZ` | no | `now()` | |

Indexes: `idx_documents_tenant(tenant_id)`, `idx_documents_workspace(workspace_id)`.

RLS: enabled and forced. Policy `documents_isolation`: `USING (tenant_id = gmrag_current_tenant()) WITH CHECK (tenant_id = gmrag_current_tenant())`.

### document_chunks

Tenant-scoped.

| Column | Type | Null | Default | Constraints |
|---|---|---:|---|---|
| `id` | `UUID` | no | `gen_random_uuid()` | primary key |
| `tenant_id` | `UUID` | no | | FK `tenants(id) ON DELETE CASCADE` |
| `document_id` | `UUID` | no | | FK `documents(id) ON DELETE CASCADE`; unique with `chunk_index` |
| `chunk_index` | `INT` | no | | unique with `document_id` |
| `content` | `TEXT` | no | | |
| `qdrant_point_id` | `UUID` | no | | |
| `token_count` | `INT` | yes | | |
| `created_at` | `TIMESTAMPTZ` | no | `now()` | |
| `page_start` | `INT` | yes | | added by page metadata migration |
| `page_end` | `INT` | yes | | added by page metadata migration |

Indexes: `idx_document_chunks_tenant(tenant_id)`, `idx_document_chunks_doc(document_id)`, `idx_document_chunks_point(qdrant_point_id)`. Constraint: `UNIQUE (document_id, chunk_index)`.

RLS: enabled and forced. Policy `document_chunks_isolation`: `USING (tenant_id = gmrag_current_tenant()) WITH CHECK (tenant_id = gmrag_current_tenant())`.

### graph_nodes

Tenant-scoped.

| Column | Type | Null | Default | Constraints |
|---|---|---:|---|---|
| `id` | `UUID` | no | `gen_random_uuid()` | primary key |
| `tenant_id` | `UUID` | no | | FK `tenants(id) ON DELETE CASCADE`; unique with `workspace_id`, `label`, `kind` |
| `kind` | `TEXT` | no | | unique with `tenant_id`, `workspace_id`, `label` |
| `label` | `TEXT` | no | | unique with `tenant_id`, `workspace_id`, `kind` |
| `properties` | `JSONB` | no | `'{}'::jsonb` | |
| `created_at` | `TIMESTAMPTZ` | no | `now()` | |
| `workspace_id` | `UUID` | yes | | FK `workspaces(id) ON DELETE CASCADE`; unique with `tenant_id`, `label`, `kind` |

Indexes: `idx_graph_nodes_tenant(tenant_id)`, `idx_graph_nodes_kind(kind)`, `idx_graph_nodes_workspace(workspace_id)`. Constraint: `graph_nodes_unique_workspace_label_kind UNIQUE (tenant_id, workspace_id, label, kind)`.

RLS: enabled and forced. Policy `graph_nodes_isolation`: `USING (tenant_id = gmrag_current_tenant()) WITH CHECK (tenant_id = gmrag_current_tenant())`.

### graph_edges

Tenant-scoped.

| Column | Type | Null | Default | Constraints |
|---|---|---:|---|---|
| `id` | `UUID` | no | `gen_random_uuid()` | primary key |
| `tenant_id` | `UUID` | no | | FK `tenants(id) ON DELETE CASCADE` |
| `src_node_id` | `UUID` | no | | FK `graph_nodes(id) ON DELETE CASCADE`; unique with `dst_node_id`, `kind` |
| `dst_node_id` | `UUID` | no | | FK `graph_nodes(id) ON DELETE CASCADE`; unique with `src_node_id`, `kind` |
| `kind` | `TEXT` | no | | unique with `src_node_id`, `dst_node_id` |
| `weight` | `REAL` | no | `1.0` | |
| `properties` | `JSONB` | no | `'{}'::jsonb` | |
| `created_at` | `TIMESTAMPTZ` | no | `now()` | |

Indexes: `idx_graph_edges_tenant(tenant_id)`, `idx_graph_edges_src(src_node_id)`, `idx_graph_edges_dst(dst_node_id)`. Constraint: `UNIQUE (src_node_id, dst_node_id, kind)`.

RLS: enabled and forced. Policy `graph_edges_isolation`: `USING (tenant_id = gmrag_current_tenant()) WITH CHECK (tenant_id = gmrag_current_tenant())`.

### graph_node_documents

Tenant-scoped provenance join table.

| Column | Type | Null | Default | Constraints |
|---|---|---:|---|---|
| `node_id` | `UUID` | no | | FK `graph_nodes(id) ON DELETE CASCADE`; primary key with `document_id` |
| `document_id` | `UUID` | no | | FK `documents(id) ON DELETE CASCADE`; primary key with `node_id` |
| `tenant_id` | `UUID` | no | | FK `tenants(id) ON DELETE CASCADE` |
| `created_at` | `TIMESTAMPTZ` | no | `now()` | |

Indexes: `idx_graph_node_documents_doc(document_id)`, `idx_graph_node_documents_node(node_id)`.

RLS: enabled and forced. Policy `graph_node_documents_isolation`: `USING (tenant_id = gmrag_current_tenant()) WITH CHECK (tenant_id = gmrag_current_tenant())`.

### chat_sessions

Tenant-scoped.

| Column | Type | Null | Default | Constraints |
|---|---|---:|---|---|
| `id` | `UUID` | no | `gen_random_uuid()` | primary key |
| `tenant_id` | `UUID` | no | | FK `tenants(id) ON DELETE CASCADE` |
| `workspace_id` | `UUID` | yes | | FK `workspaces(id) ON DELETE SET NULL` |
| `user_id` | `UUID` | no | | FK `users(id)` |
| `title` | `TEXT` | no | `''` | |
| `model` | `TEXT` | yes | | |
| `created_at` | `TIMESTAMPTZ` | no | `now()` | |
| `updated_at` | `TIMESTAMPTZ` | no | `now()` | |

Indexes: `idx_chat_sessions_tenant(tenant_id)`, `idx_chat_sessions_workspace(workspace_id)`.

RLS: enabled and forced. Policy `chat_sessions_isolation`: `USING (tenant_id = gmrag_current_tenant()) WITH CHECK (tenant_id = gmrag_current_tenant())`.

### chat_messages

Tenant-scoped.

| Column | Type | Null | Default | Constraints |
|---|---|---:|---|---|
| `id` | `UUID` | no | `gen_random_uuid()` | primary key |
| `tenant_id` | `UUID` | no | | FK `tenants(id) ON DELETE CASCADE` |
| `session_id` | `UUID` | no | | FK `chat_sessions(id) ON DELETE CASCADE` |
| `role` | `TEXT` | no | | check `role IN ('user','assistant','system')` |
| `content` | `TEXT` | no | | |
| `token_count` | `INT` | yes | | |
| `created_at` | `TIMESTAMPTZ` | no | `now()` | |

Indexes: `idx_chat_messages_tenant(tenant_id)`, `idx_chat_messages_session(session_id)`.

RLS: enabled and forced. Policy `chat_messages_isolation`: `USING (tenant_id = gmrag_current_tenant()) WITH CHECK (tenant_id = gmrag_current_tenant())`.

### invitations

Tenant-scoped.

| Column | Type | Null | Default | Constraints |
|---|---|---:|---|---|
| `id` | `UUID` | no | `gen_random_uuid()` | primary key |
| `tenant_id` | `UUID` | no | | FK `tenants(id) ON DELETE CASCADE` |
| `workspace_id` | `UUID` | yes | | FK `workspaces(id) ON DELETE CASCADE` |
| `email` | `TEXT` | no | | |
| `role` | `TEXT` | no | `'member'` | |
| `token` | `UUID` | no | `gen_random_uuid()` | |
| `status` | `TEXT` | no | `'pending'` | check `status IN ('pending','accepted','expired','revoked')` |
| `invited_by` | `UUID` | no | | FK `users(id)` |
| `expires_at` | `TIMESTAMPTZ` | yes | | |
| `created_at` | `TIMESTAMPTZ` | no | `now()` | |
| `accepted_at` | `TIMESTAMPTZ` | yes | | |

Indexes: `idx_invitations_tenant(tenant_id)`, `idx_invitations_token(token)`, `idx_invitations_email(email)`, `idx_invitations_workspace(workspace_id)`.

RLS: enabled and forced. Policy `invitations_isolation`: `USING (tenant_id = gmrag_current_tenant()) WITH CHECK (tenant_id = gmrag_current_tenant())`.

### tenant_quotas

Tenant-scoped.

| Column | Type | Null | Default | Constraints |
|---|---|---:|---|---|
| `tenant_id` | `UUID` | no | | primary key; FK `tenants(id) ON DELETE CASCADE` |
| `max_documents` | `INT` | no | `100` | |
| `max_workspaces` | `INT` | no | `10` | |
| `max_storage_bytes` | `BIGINT` | no | `10737418240` | |
| `max_members` | `INT` | no | `50` | |
| `updated_at` | `TIMESTAMPTZ` | no | `now()` | |

RLS: enabled and forced. Policy `tenant_quotas_isolation`: `USING (tenant_id = gmrag_current_tenant()) WITH CHECK (tenant_id = gmrag_current_tenant())`.

### usage_events

Tenant-scoped.

| Column | Type | Null | Default | Constraints |
|---|---|---:|---|---|
| `id` | `UUID` | no | `gen_random_uuid()` | primary key |
| `tenant_id` | `UUID` | no | | FK `tenants(id) ON DELETE CASCADE` |
| `metric` | `TEXT` | no | | |
| `delta` | `BIGINT` | no | `1` | |
| `metadata` | `JSONB` | yes | | |
| `created_at` | `TIMESTAMPTZ` | no | `now()` | |

Indexes: `idx_usage_events_tenant(tenant_id)`, `idx_usage_events_metric(metric)`, `idx_usage_events_created(created_at)`.

RLS: enabled and forced. Policy `usage_events_isolation`: `USING (tenant_id = gmrag_current_tenant()) WITH CHECK (tenant_id = gmrag_current_tenant())`.

### audit_log

Tenant-scoped.

| Column | Type | Null | Default | Constraints |
|---|---|---:|---|---|
| `id` | `UUID` | no | `gen_random_uuid()` | primary key |
| `tenant_id` | `UUID` | no | | FK `tenants(id) ON DELETE CASCADE` |
| `actor_id` | `UUID` | yes | | FK `users(id)` |
| `action` | `TEXT` | no | | |
| `resource_type` | `TEXT` | yes | | |
| `resource_id` | `UUID` | yes | | |
| `metadata` | `JSONB` | yes | | |
| `created_at` | `TIMESTAMPTZ` | no | `now()` | |

Indexes: `idx_audit_log_tenant(tenant_id)`, `idx_audit_log_actor(actor_id)`, `idx_audit_log_created(created_at)`.

RLS: enabled and forced. Policy `audit_log_isolation`: `USING (tenant_id = gmrag_current_tenant()) WITH CHECK (tenant_id = gmrag_current_tenant())`.

### ingest_jobs

Tenant-scoped.

| Column | Type | Null | Default | Constraints |
|---|---|---:|---|---|
| `id` | `UUID` | no | `gen_random_uuid()` | primary key |
| `tenant_id` | `UUID` | no | | FK `tenants(id) ON DELETE CASCADE` |
| `document_id` | `UUID` | no | | FK `documents(id) ON DELETE CASCADE` |
| `status` | `TEXT` | no | `'pending'` | check `status IN ('pending','processing','completed','failed')` |
| `attempts` | `INT` | no | `0` | |
| `last_error` | `TEXT` | yes | | |
| `created_at` | `TIMESTAMPTZ` | no | `now()` | |
| `updated_at` | `TIMESTAMPTZ` | no | `now()` | |
| `claimed_at` | `TIMESTAMPTZ` | yes | | added by sweeper migration |

Indexes: `idx_ingest_jobs_tenant(tenant_id)`, `idx_ingest_jobs_status(status)`, `idx_ingest_jobs_doc(document_id)`, partial `idx_ingest_jobs_claim(claimed_at) WHERE status = 'processing'`.

RLS: enabled and forced. Policy `ingest_jobs_isolation`: `USING (tenant_id = gmrag_current_tenant()) WITH CHECK (tenant_id = gmrag_current_tenant())`.

### ingest_outbox

Tenant-scoped transactional enqueue table.

| Column | Type | Null | Default | Constraints |
|---|---|---:|---|---|
| `id` | `UUID` | no | `gen_random_uuid()` | primary key |
| `tenant_id` | `UUID` | no | | FK `tenants(id) ON DELETE CASCADE` |
| `document_id` | `UUID` | no | | FK `documents(id) ON DELETE CASCADE` |
| `payload` | `JSONB` | no | | |
| `status` | `TEXT` | no | `'pending'` | check `status IN ('pending','dispatched')` |
| `created_at` | `TIMESTAMPTZ` | no | `now()` | |
| `dispatched_at` | `TIMESTAMPTZ` | yes | | |

Indexes: `idx_ingest_outbox_status_created(status, created_at)`, partial `idx_ingest_outbox_dispatched_at(dispatched_at) WHERE status = 'dispatched' AND dispatched_at IS NOT NULL`.

RLS: enabled and forced. Policy `ingest_outbox_isolation`: `USING (tenant_id = gmrag_current_tenant()) WITH CHECK (tenant_id = gmrag_current_tenant())`.

### tenant_llm_config

Tenant-scoped.

| Column | Type | Null | Default | Constraints |
|---|---|---:|---|---|
| `tenant_id` | `UUID` | no | | primary key; FK `tenants(id) ON DELETE CASCADE` |
| `provider` | `TEXT` | no | | |
| `api_key` | `TEXT` | yes | | legacy plaintext fallback |
| `model` | `TEXT` | no | | |
| `base_url` | `TEXT` | yes | | |
| `dimensions` | `INT` | no | `768` | |
| `enabled` | `BOOLEAN` | no | `true` | |
| `created_at` | `TIMESTAMPTZ` | no | `now()` | |
| `updated_at` | `TIMESTAMPTZ` | no | `now()` | |
| `llm_model` | `TEXT` | yes | | |
| `llm_base_url` | `TEXT` | yes | | |
| `api_key_ciphertext` | `BYTEA` | yes | | check paired with nonce |
| `api_key_nonce` | `BYTEA` | yes | | check paired with ciphertext |

Constraint: `tenant_llm_config_encrypted_key_pair` requires ciphertext and nonce to both be null or both be non-null.

RLS: enabled and forced. Policy `tenant_llm_config_isolation`: `USING (tenant_id = gmrag_current_tenant()) WITH CHECK (tenant_id = gmrag_current_tenant())`.

## Grants

All current tables above are granted to `gmrag_app` for `SELECT, INSERT, UPDATE, DELETE`, except `platform_admins` has `SELECT, INSERT, DELETE` and global `users`/`tenants` are also granted. `resource_acl` grants are historical because the table is dropped in the final migration.

## Rust vs Migration Drift

- Runtime ACL routes no longer query `resource_acl`; they use OpenFGA direct tuples. `resource_acl` references remain in migration history, `schema_acl.rs`, and `reconcile/backfill.rs` only. `reconcile/backfill.rs` first checks `to_regclass('public.resource_acl')`, so it tolerates the table being absent.
- I did not find a current runtime table reference in the opened route/worker files that lacks a migration-backed table. The most important removed table, `resource_acl`, is explicitly guarded or historical.
- Old docs referenced non-current columns/table names such as `relation_tuples`, `llm_settings`, `audit_logs`, `documents.size_bytes`, `graph_nodes.description`, `graph_nodes.qdrant_point_id`, `ingest_outbox.dispatched`, and `ingest_outbox.payload_json`; these are not present in the current migrations.
