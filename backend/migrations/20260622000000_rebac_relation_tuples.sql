-- =========================================================
-- T64: ReBAC relation-tuple semantics over resource_acl.
-- Zanzibar-style relationship-based access control (paper docs/5068.pdf).
--
-- resource_acl already stores polymorphic tuples
--   (resource_type, resource_id, principal_type, principal_id, permission).
-- Reinterpreted as a Zanzibar relation tuple:
--   object   = (resource_type : resource_id)
--   relation = permission            ('owner' | 'editor' | 'viewer')
--   user     = (principal_type : principal_id)  where principal_type is the
--              subject namespace: 'user' (a direct subject) or 'workspace'
--              (a userset / group — every member of the workspace).
--
-- The "owner" relation for documents/chat_sessions is the row's own
-- owner column (documents.owner_id / chat_sessions.user_id) and the "member"
-- relation for workspaces lives in workspace_members; those are NOT stored
-- here. resource_acl holds only the explicit share grants.
--
-- This migration constrains the relation + subject vocabularies and adds a
-- covering index for the Check hot path. RLS on resource_acl was already
-- applied in T25 (rls_apply_all).
-- =========================================================

-- Default relation for a new grant is the least-privileged 'viewer'.
ALTER TABLE resource_acl ALTER COLUMN permission SET DEFAULT 'viewer';

-- Constrain the relation vocabulary to the MVP namespace config.
ALTER TABLE resource_acl
    ADD CONSTRAINT resource_acl_relation_chk
    CHECK (permission IN ('owner', 'editor', 'viewer'));

-- Constrain subject namespaces: a direct user, or a workspace userset (group).
ALTER TABLE resource_acl
    ADD CONSTRAINT resource_acl_principal_type_chk
    CHECK (principal_type IN ('user', 'workspace'));

-- Covering index for the Check evaluation hot path:
--   WHERE resource_type = $1 AND resource_id = $2 AND permission = ANY($3)
CREATE INDEX idx_resource_acl_check
    ON resource_acl (resource_type, resource_id, permission);
