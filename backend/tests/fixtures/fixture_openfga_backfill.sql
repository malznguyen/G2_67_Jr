-- Controlled fixture for OpenFGA backfill validation.
-- Targets the schema as of migration 20260624020000 (resource_acl still present).
-- Wipes prior fixture rows so the file is rerunnable.

BEGIN;

DELETE FROM resource_acl;
DELETE FROM graph_node_documents;
DELETE FROM graph_nodes;
DELETE FROM graph_edges;
DELETE FROM document_chunks;
DELETE FROM chat_messages;
DELETE FROM chat_sessions;
DELETE FROM documents;
DELETE FROM workspace_members;
DELETE FROM workspaces;
DELETE FROM tenant_members;
DELETE FROM tenants;

-- ---------- Tenants & membership ----------
INSERT INTO tenants (id, name) VALUES
  ('10000000-0000-0000-0000-000000000001', 'Tenant A'),
  ('10000000-0000-0000-0000-000000000002', 'Tenant B')
ON CONFLICT (id) DO NOTHING;

INSERT INTO users (id, email, name) VALUES
  ('00000000-0000-0000-0000-000000000001', 'owner-a@example.com',  'TenantA Owner'),
  ('00000000-0000-0000-0000-000000000002', 'admin-a@example.com',  'TenantA Admin'),
  ('00000000-0000-0000-0000-000000000003', 'member-a@example.com', 'TenantA Member'),
  ('00000000-0000-0000-0000-000000000004', 'wsowner@example.com',  'WS Owner'),
  ('00000000-0000-0000-0000-000000000005', 'wsadmin@example.com',  'WS Admin'),
  ('00000000-0000-0000-0000-000000000006', 'wsmember@example.com', 'WS Member'),
  ('00000000-0000-0000-0000-000000000007', 'doceditor@example.com','Doc Editor'),
  ('00000000-0000-0000-0000-000000000008', 'docviewer@example.com','Doc Viewer'),
  ('00000000-0000-0000-0000-000000000011', 'owner-b@example.com',  'TenantB Owner'),
  ('00000000-0000-0000-0000-000000000012', 'member-b@example.com', 'TenantB Member')
ON CONFLICT (id) DO NOTHING;

INSERT INTO tenant_members (tenant_id, user_id, role) VALUES
  ('10000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000001', 'owner'),
  ('10000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000002', 'admin'),
  ('10000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000003', 'member'),
  ('10000000-0000-0000-0000-000000000002', '00000000-0000-0000-0000-000000000011', 'owner'),
  ('10000000-0000-0000-0000-000000000002', '00000000-0000-0000-0000-000000000012', 'member')
ON CONFLICT DO NOTHING;

-- ---------- Workspace ----------
INSERT INTO workspaces (id, tenant_id, name, slug, created_by) VALUES
  ('20000000-0000-0000-0000-000000000001', '10000000-0000-0000-0000-000000000001', 'Eng', 'eng', '00000000-0000-0000-0000-000000000004')
ON CONFLICT (id) DO NOTHING;

INSERT INTO workspace_members (workspace_id, tenant_id, user_id, role) VALUES
  ('20000000-0000-0000-0000-000000000001', '10000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000004', 'owner'),
  ('20000000-0000-0000-0000-000000000001', '10000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000005', 'admin'),
  ('20000000-0000-0000-0000-000000000001', '10000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000006', 'member')
ON CONFLICT DO NOTHING;

-- ---------- Documents (owner + shared + private + cross-tenant) ----------
INSERT INTO documents (id, tenant_id, workspace_id, owner_id, title, status, visibility, s3_key) VALUES
  ('30000000-0000-0000-0000-000000000001', '10000000-0000-0000-0000-000000000001', '20000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000004', 'owned.pdf',   'indexed', 'private', 'k1'),
  ('30000000-0000-0000-0000-000000000002', '10000000-0000-0000-0000-000000000001', '20000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000004', 'shared.pdf', 'indexed', 'shared',  'k2'),
  ('30000000-0000-0000-0000-000000000003', '10000000-0000-0000-0000-000000000001', '20000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000004', 'granted.pdf','indexed', 'private', 'k3'),
  ('30000000-0000-0000-0000-000000000005', '10000000-0000-0000-0000-000000000002', NULL, '00000000-0000-0000-0000-000000000011', 'tenantb.pdf', 'indexed', 'private', 'k5')
ON CONFLICT (id) DO NOTHING;

-- ---------- Chat sessions ----------
INSERT INTO chat_sessions (id, tenant_id, workspace_id, user_id) VALUES
  ('40000000-0000-0000-0000-000000000001', '10000000-0000-0000-0000-000000000001', '20000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000004'),
  ('40000000-0000-0000-0000-000000000002', '10000000-0000-0000-0000-000000000001', NULL, '00000000-0000-0000-0000-000000000006')
ON CONFLICT (id) DO NOTHING;

-- ---------- Legacy resource_acl rows (direct + workspace group + duplicate-equivalent) ----------
-- Direct user viewer grant on doc 003.
INSERT INTO resource_acl (tenant_id, resource_type, resource_id, principal_type, principal_id, permission) VALUES
  ('10000000-0000-0000-0000-000000000001', 'document', '30000000-0000-0000-0000-000000000003', 'user',      '00000000-0000-0000-0000-000000000008', 'viewer'),
  -- Direct user editor grant on doc 003.
  ('10000000-0000-0000-0000-000000000001', 'document', '30000000-0000-0000-0000-000000000003', 'user',      '00000000-0000-0000-0000-000000000007', 'editor'),
  -- Workspace-group viewer grant on chat session 001.
  ('10000000-0000-0000-0000-000000000001', 'chat_session', '40000000-0000-0000-0000-000000000001', 'workspace', '20000000-0000-0000-0000-000000000001', 'viewer'),
  -- Direct user viewer grant on chat session 002 (null-workspace session).
  ('10000000-0000-0000-0000-000000000001', 'chat_session', '40000000-0000-0000-0000-000000000002', 'user',      '00000000-0000-0000-0000-000000000003', 'viewer'),
  -- Duplicate-equivalent row (same logical tuple, different insert id) —
  -- UNIQUE constraint blocks true duplicates on the 5-tuple; this row
  -- intentionally duplicates the *logical* OpenFGA tuple that document_tuples
  -- also derives from the documents.owner_id column. It exercises dedupe().
  ('10000000-0000-0000-0000-000000000001', 'document', '30000000-0000-0000-0000-000000000001', 'user',      '00000000-0000-0000-0000-000000000004', 'owner')
ON CONFLICT DO NOTHING;

COMMIT;
