-- =========================================================
-- T26: Database seed script for local development.
--
-- Inserts mock data for 2 tenants (Acme, Globex) with users,
-- workspaces, documents, chunks, chat history, quotas, audit log,
-- and an ingest job. Run AFTER migrations are applied.
--
-- Usage:
--   docker cp infra/postgres/seed.sql gmrag-postgres16:/tmp/seed.sql
--   docker exec gmrag-postgres16 psql -U gmrag -d gmrag -f /tmp/seed.sql
--
-- This script runs as the superuser (gmrag), which bypasses RLS.
-- At runtime, the app pool (gmrag_app) will only see rows matching
-- the active tenant context (app.tenant_id).
--
-- Idempotent: uses ON CONFLICT DO NOTHING so it can be re-run safely.
-- All UUIDs use hex-valid characters only (a-f, 0-9).
-- =========================================================

BEGIN;

-- ---------- Tenants ----------
INSERT INTO tenants (id, name) VALUES
    ('a1000000-0000-0000-0000-000000000001', 'Acme Corp'),
    ('a1000000-0000-0000-0000-000000000002', 'Globex Inc')
ON CONFLICT (id) DO NOTHING;

-- ---------- Users ----------
INSERT INTO users (id, email, name) VALUES
    ('b1000000-0000-0000-0000-000000000001', 'alice@acme.com', 'Alice Chen'),
    ('b1000000-0000-0000-0000-000000000002', 'bob@acme.com', 'Bob Smith'),
    ('b1000000-0000-0000-0000-000000000003', 'carol@globex.com', 'Carol Wong')
ON CONFLICT (id) DO NOTHING;

-- ---------- Tenant Members ----------
-- Alice is owner of Acme, Bob is member of Acme + Globex, Carol is owner of Globex.
INSERT INTO tenant_members (tenant_id, user_id, role) VALUES
    ('a1000000-0000-0000-0000-000000000001', 'b1000000-0000-0000-0000-000000000001', 'owner'),
    ('a1000000-0000-0000-0000-000000000001', 'b1000000-0000-0000-0000-000000000002', 'member'),
    ('a1000000-0000-0000-0000-000000000002', 'b1000000-0000-0000-0000-000000000002', 'member'),
    ('a1000000-0000-0000-0000-000000000002', 'b1000000-0000-0000-0000-000000000003', 'owner')
ON CONFLICT (tenant_id, user_id) DO NOTHING;

-- ---------- Workspaces ----------
INSERT INTO workspaces (id, tenant_id, name, slug, created_by) VALUES
    ('c1000000-0000-0000-0000-000000000001', 'a1000000-0000-0000-0000-000000000001', 'Engineering', 'eng', 'b1000000-0000-0000-0000-000000000001'),
    ('c1000000-0000-0000-0000-000000000002', 'a1000000-0000-0000-0000-000000000002', 'Research', 'research', 'b1000000-0000-0000-0000-000000000003')
ON CONFLICT (id) DO NOTHING;

-- ---------- Workspace Members ----------
INSERT INTO workspace_members (workspace_id, tenant_id, user_id, role) VALUES
    ('c1000000-0000-0000-0000-000000000001', 'a1000000-0000-0000-0000-000000000001', 'b1000000-0000-0000-0000-000000000001', 'admin'),
    ('c1000000-0000-0000-0000-000000000001', 'a1000000-0000-0000-0000-000000000001', 'b1000000-0000-0000-0000-000000000002', 'member'),
    ('c1000000-0000-0000-0000-000000000002', 'a1000000-0000-0000-0000-000000000002', 'b1000000-0000-0000-0000-000000000003', 'admin')
ON CONFLICT (workspace_id, user_id) DO NOTHING;

-- ---------- Documents ----------
INSERT INTO documents (id, tenant_id, workspace_id, owner_id, title, status, mime_type, byte_size, s3_key) VALUES
    ('d1000000-0000-0000-0000-000000000001', 'a1000000-0000-0000-0000-000000000001', 'c1000000-0000-0000-0000-000000000001', 'b1000000-0000-0000-0000-000000000001', 'Architecture Guide', 'indexed', 'application/pdf', 245678, 'acme/eng/architecture-guide.pdf'),
    ('d1000000-0000-0000-0000-000000000002', 'a1000000-0000-0000-0000-000000000001', 'c1000000-0000-0000-0000-000000000001', 'b1000000-0000-0000-0000-000000000002', 'API Reference', 'indexed', 'text/markdown', 45120, 'acme/eng/api-reference.md'),
    ('d1000000-0000-0000-0000-000000000003', 'a1000000-0000-0000-0000-000000000002', 'c1000000-0000-0000-0000-000000000002', 'b1000000-0000-0000-0000-000000000003', 'Research Notes', 'processing', 'text/plain', 8900, 'globex/research/notes.txt')
ON CONFLICT (id) DO NOTHING;

-- ---------- Document Chunks ----------
-- qdrant_point_id references hypothetical Qdrant points.
INSERT INTO document_chunks (id, tenant_id, document_id, chunk_index, content, qdrant_point_id, token_count) VALUES
    ('e1000000-0000-0000-0000-000000000001', 'a1000000-0000-0000-0000-000000000001', 'd1000000-0000-0000-0000-000000000001', 0, 'The system uses a microservices architecture with...', 'a2000000-0000-0000-0000-000000000011', 128),
    ('e1000000-0000-0000-0000-000000000002', 'a1000000-0000-0000-0000-000000000001', 'd1000000-0000-0000-0000-000000000001', 1, 'Authentication is handled via Keycloak OIDC...', 'a2000000-0000-0000-0000-000000000012', 96),
    ('e1000000-0000-0000-0000-000000000003', 'a1000000-0000-0000-0000-000000000001', 'd1000000-0000-0000-0000-000000000002', 0, 'GET /users/me returns the authenticated user profile...', 'a2000000-0000-0000-0000-000000000013', 64),
    ('e1000000-0000-0000-0000-000000000004', 'a1000000-0000-0000-0000-000000000001', 'd1000000-0000-0000-0000-000000000002', 1, 'POST /documents uploads a file to S3 and creates...', 'a2000000-0000-0000-0000-000000000014', 80),
    ('e1000000-0000-0000-0000-000000000005', 'a1000000-0000-0000-0000-000000000002', 'd1000000-0000-0000-0000-000000000003', 0, 'Initial research findings indicate that...', 'a2000000-0000-0000-0000-000000000015', 112)
ON CONFLICT (id) DO NOTHING;

-- ---------- Chat Sessions ----------
INSERT INTO chat_sessions (id, tenant_id, workspace_id, user_id, title, model) VALUES
    ('f1000000-0000-0000-0000-000000000001', 'a1000000-0000-0000-0000-000000000001', 'c1000000-0000-0000-0000-000000000001', 'b1000000-0000-0000-0000-000000000001', 'Architecture Q&A', 'deepseek-v4-flash')
ON CONFLICT (id) DO NOTHING;

-- ---------- Chat Messages ----------
INSERT INTO chat_messages (id, tenant_id, session_id, role, content, token_count) VALUES
    ('f2000000-0000-0000-0000-000000000001', 'a1000000-0000-0000-0000-000000000001', 'f1000000-0000-0000-0000-000000000001', 'user', 'What architecture does the system use?', 8),
    ('f2000000-0000-0000-0000-000000000002', 'a1000000-0000-0000-0000-000000000001', 'f1000000-0000-0000-0000-000000000001', 'assistant', 'The system uses a microservices architecture with...', 45)
ON CONFLICT (id) DO NOTHING;

-- ---------- Tenant Quotas ----------
INSERT INTO tenant_quotas (tenant_id, max_documents, max_workspaces, max_storage_bytes, max_members) VALUES
    ('a1000000-0000-0000-0000-000000000001', 500, 20, 53687091200, 100),
    ('a1000000-0000-0000-0000-000000000002', 200, 10, 10737418240, 50)
ON CONFLICT (tenant_id) DO NOTHING;

-- ---------- Audit Log ----------
INSERT INTO audit_log (id, tenant_id, actor_id, action, resource_type, resource_id, metadata) VALUES
    ('b2000000-0000-0000-0000-000000000001', 'a1000000-0000-0000-0000-000000000001', 'b1000000-0000-0000-0000-000000000001', 'document.upload', 'document', 'd1000000-0000-0000-0000-000000000001', '{"source": "web", "ip": "127.0.0.1"}'::jsonb)
ON CONFLICT (id) DO NOTHING;

-- ---------- Ingest Jobs ----------
INSERT INTO ingest_jobs (id, tenant_id, document_id, status, attempts) VALUES
    ('c2000000-0000-0000-0000-000000000001', 'a1000000-0000-0000-0000-000000000002', 'd1000000-0000-0000-0000-000000000003', 'processing', 0)
ON CONFLICT (id) DO NOTHING;

COMMIT;

-- ---------- Summary ----------
SELECT 'tenants' as table_name, COUNT(*) as cnt FROM tenants
UNION ALL SELECT 'users', COUNT(*) FROM users
UNION ALL SELECT 'tenant_members', COUNT(*) FROM tenant_members
UNION ALL SELECT 'workspaces', COUNT(*) FROM workspaces
UNION ALL SELECT 'workspace_members', COUNT(*) FROM workspace_members
UNION ALL SELECT 'documents', COUNT(*) FROM documents
UNION ALL SELECT 'document_chunks', COUNT(*) FROM document_chunks
UNION ALL SELECT 'chat_sessions', COUNT(*) FROM chat_sessions
UNION ALL SELECT 'chat_messages', COUNT(*) FROM chat_messages
UNION ALL SELECT 'tenant_quotas', COUNT(*) FROM tenant_quotas
UNION ALL SELECT 'audit_log', COUNT(*) FROM audit_log
UNION ALL SELECT 'ingest_jobs', COUNT(*) FROM ingest_jobs
ORDER BY table_name;
