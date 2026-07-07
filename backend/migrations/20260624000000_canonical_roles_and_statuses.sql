-- =========================================================
-- Phase 0 (TASK-P0-01 + TASK-P0-02): canonical role and
-- status vocabularies as named CHECK constraints.
--
-- Forward-only. Normalizes known legacy values BEFORE adding
-- the CHECK constraints so existing rows satisfy the closed
-- vocabularies. Unknown / unsupported values raise an
-- exception (the migration aborts) rather than being silently
-- coerced — corrupt data must be visible, not hidden.
--
-- Constraint names are stable and descriptive so future
-- migrations can reference them by name if needed.
-- =========================================================

-- ---------- TASK-P0-01: membership roles ----------
-- tenant_members.role ∈ {owner, admin, member}
-- workspace_members.role ∈ {owner, admin, member}
--
-- Known legacy values for these columns are exactly
-- {owner, admin, member} (seed.sql used 'admin'/'member'/'owner';
-- code default is 'member'). No remapping is required — every
-- existing value is already canonical. We still guard against
-- unknown values so the migration fails loudly on corrupt data.

DO $$
DECLARE
    bad_tenant int;
    bad_ws int;
BEGIN
    SELECT COUNT(*) INTO bad_tenant
    FROM tenant_members
    WHERE role IS NOT NULL
      AND role NOT IN ('owner', 'admin', 'member');

    IF bad_tenant > 0 THEN
        RAISE EXCEPTION
          'tenant_members contains % row(s) with non-canonical role; '
          'manual remediation required before Phase 0 CHECK can be applied',
          bad_tenant;
    END IF;

    SELECT COUNT(*) INTO bad_ws
    FROM workspace_members
    WHERE role IS NOT NULL
      AND role NOT IN ('owner', 'admin', 'member');

    IF bad_ws > 0 THEN
        RAISE EXCEPTION
          'workspace_members contains % row(s) with non-canonical role; '
          'manual remediation required before Phase 0 CHECK can be applied',
          bad_ws;
    END IF;
END
$$;

ALTER TABLE tenant_members
    ADD CONSTRAINT tenant_members_role_chk
    CHECK (role IN ('owner', 'admin', 'member'));

ALTER TABLE workspace_members
    ADD CONSTRAINT workspace_members_role_chk
    CHECK (role IN ('owner', 'admin', 'member'));

-- ---------- TASK-P0-02: status / chat-role vocabularies ----------
-- documents.status      ∈ {uploaded, processing, indexed, failed}
-- ingest_jobs.status    ∈ {pending, processing, completed, failed}
-- ingest_outbox.status  ∈ {pending, dispatched}
-- invitations.status    ∈ {pending, accepted, expired, revoked}
-- chat_messages.role    ∈ {user, assistant, system}
--
-- Normalization: documents.status='ready' → 'indexed' (the only
-- known legacy drift, from seed.sql). All other known values are
-- already canonical. Unknown values raise an exception per table.

-- documents: normalize 'ready' → 'indexed', then guard.
UPDATE documents SET status = 'indexed', updated_at = now()
WHERE status = 'ready';

DO $$
DECLARE
    bad int;
BEGIN
    SELECT COUNT(*) INTO bad
    FROM documents
    WHERE status NOT IN ('uploaded', 'processing', 'indexed', 'failed');

    IF bad > 0 THEN
        RAISE EXCEPTION
          'documents contains % row(s) with non-canonical status; '
          'manual remediation required before Phase 0 CHECK can be applied',
          bad;
    END IF;
END
$$;

ALTER TABLE documents
    ADD CONSTRAINT documents_status_chk
    CHECK (status IN ('uploaded', 'processing', 'indexed', 'failed'));

-- ingest_jobs: guard, then CHECK.
DO $$
DECLARE
    bad int;
BEGIN
    SELECT COUNT(*) INTO bad
    FROM ingest_jobs
    WHERE status NOT IN ('pending', 'processing', 'completed', 'failed');

    IF bad > 0 THEN
        RAISE EXCEPTION
          'ingest_jobs contains % row(s) with non-canonical status; '
          'manual remediation required before Phase 0 CHECK can be applied',
          bad;
    END IF;
END
$$;

ALTER TABLE ingest_jobs
    ADD CONSTRAINT ingest_jobs_status_chk
    CHECK (status IN ('pending', 'processing', 'completed', 'failed'));

-- ingest_outbox: guard, then CHECK.
DO $$
DECLARE
    bad int;
BEGIN
    SELECT COUNT(*) INTO bad
    FROM ingest_outbox
    WHERE status NOT IN ('pending', 'dispatched');

    IF bad > 0 THEN
        RAISE EXCEPTION
          'ingest_outbox contains % row(s) with non-canonical status; '
          'manual remediation required before Phase 0 CHECK can be applied',
          bad;
    END IF;
END
$$;

ALTER TABLE ingest_outbox
    ADD CONSTRAINT ingest_outbox_status_chk
    CHECK (status IN ('pending', 'dispatched'));

-- invitations: guard, then CHECK.
DO $$
DECLARE
    bad int;
BEGIN
    SELECT COUNT(*) INTO bad
    FROM invitations
    WHERE status NOT IN ('pending', 'accepted', 'expired', 'revoked');

    IF bad > 0 THEN
        RAISE EXCEPTION
          'invitations contains % row(s) with non-canonical status; '
          'manual remediation required before Phase 0 CHECK can be applied',
          bad;
    END IF;
END
$$;

ALTER TABLE invitations
    ADD CONSTRAINT invitations_status_chk
    CHECK (status IN ('pending', 'accepted', 'expired', 'revoked'));

-- chat_messages.role: guard, then CHECK.
DO $$
DECLARE
    bad int;
BEGIN
    SELECT COUNT(*) INTO bad
    FROM chat_messages
    WHERE role NOT IN ('user', 'assistant', 'system');

    IF bad > 0 THEN
        RAISE EXCEPTION
          'chat_messages contains % row(s) with non-canonical role; '
          'manual remediation required before Phase 0 CHECK can be applied',
          bad;
    END IF;
END
$$;

ALTER TABLE chat_messages
    ADD CONSTRAINT chat_messages_role_chk
    CHECK (role IN ('user', 'assistant', 'system'));
