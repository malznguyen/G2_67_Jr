//! OpenFGA tuple derivation, shared by the `openfga_backfill` ops binary and
//! the Phase 3 drift reconciler.
//!
//! This module was extracted verbatim from `api/src/bin/openfga_backfill.rs`
//! so both the one-shot backfill and the periodic reconciler derive the
//! "expected structural tuple set" from the same Postgres reads — never two
//! independent implementations. Postgres remains the source of truth; the
//! derived set is what OpenFGA *should* contain for the live membership /
//! resource rows.
//!
//! The functions here read the cross-tenant admin view of the tables
//! (tenant_members, workspaces, workspace_members, documents, chat_sessions,
//! and the legacy `resource_acl` if it still exists), so they must be called
//! on the admin pool (bypasses RLS) — the same sanctioned exception as the
//! outbox relay and the job sweeper.

use std::collections::HashSet;

use anyhow::Context as _;
use sqlx::PgPool;
use tracing::warn;
use uuid::Uuid;

use crate::authz::{
    chat_owner_tuple, chat_session_obj, chat_tenant_tuple, chat_workspace_tuple, document_obj,
    document_owner_tuple, document_shared_tuple, document_tenant_tuple, document_workspace_tuple,
    tenant_obj, tuple, user_obj, workspace_member_userset, workspace_role_tuple,
    workspace_tenant_tuple, RelationshipTuple, REL_ADMIN, REL_EDITOR, REL_MEMBER, REL_OWNER,
    REL_VIEWER, TYPE_CHAT_SESSION, TYPE_DOCUMENT, TYPE_USER, TYPE_WORKSPACE,
};

/// Derive the full expected structural tuple set from current Postgres state.
///
/// Returns `(tuples, per-source counts)` where `tuples` is deduplicated. The
/// reconciler treats this as the set of tuples OpenFGA *should* contain for
/// membership / resource provenance (tenant↔user, workspace↔tenant/member,
/// document↔tenant/owner/workspace/shared, chat_session↔tenant/owner/
/// workspace). Dynamic ACL grants (editor/viewer on a live document or
/// chat_session) are NOT in this set — they live only in OpenFGA — so the
/// reconciler detects missing structural tuples via set-difference but
/// detects orphaned tuples via *resource existence*, NOT set-difference (see
/// `openfga::run_openfga_reconcile`).
pub async fn collect_tuples(
    pool: &PgPool,
) -> anyhow::Result<(Vec<RelationshipTuple>, Vec<(&'static str, usize)>)> {
    let mut counts: Vec<(&'static str, usize)> = Vec::new();

    let tenant = tenant_member_tuples(pool).await?;
    counts.push(("tenant_member", tenant.len()));
    let workspace = workspace_tuples(pool).await?;
    counts.push(("workspace", workspace.len()));
    let document = document_tuples(pool).await?;
    counts.push(("document", document.len()));
    let chat = chat_session_tuples(pool).await?;
    counts.push(("chat_session", chat.len()));
    let old_acl = old_resource_acl_tuples(pool).await?;
    counts.push(("resource_acl", old_acl.len()));

    let mut tuples = Vec::new();
    tuples.extend(tenant);
    tuples.extend(workspace);
    tuples.extend(document);
    tuples.extend(chat);
    tuples.extend(old_acl);
    let pre_dedupe = tuples.len();
    let tuples = dedupe(tuples);
    counts.push(("pre_dedupe", pre_dedupe));
    counts.push(("deduped_total", tuples.len()));
    Ok((tuples, counts))
}

fn dedupe(tuples: Vec<RelationshipTuple>) -> Vec<RelationshipTuple> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for tuple in tuples {
        if seen.insert(tuple.clone()) {
            out.push(tuple);
        }
    }
    out
}

async fn tenant_member_tuples(pool: &PgPool) -> anyhow::Result<Vec<RelationshipTuple>> {
    let rows: Vec<(Uuid, Uuid, String)> =
        sqlx::query_as("SELECT tenant_id, user_id, role FROM tenant_members")
            .fetch_all(pool)
            .await
            .context("reading tenant_members")?;

    let mut tuples = Vec::new();
    for (tenant_id, user_id, role) in rows {
        if let Some(relation) = role_relation(&role) {
            tuples.push(tuple(user_obj(user_id), relation, tenant_obj(tenant_id)));
        } else {
            warn!(%tenant_id, %user_id, role, "skipping unknown tenant role");
        }
    }
    Ok(tuples)
}

async fn workspace_tuples(pool: &PgPool) -> anyhow::Result<Vec<RelationshipTuple>> {
    let workspaces: Vec<(Uuid, Uuid)> = sqlx::query_as("SELECT id, tenant_id FROM workspaces")
        .fetch_all(pool)
        .await
        .context("reading workspaces")?;
    let members: Vec<(Uuid, Uuid, String)> =
        sqlx::query_as("SELECT workspace_id, user_id, role FROM workspace_members")
            .fetch_all(pool)
            .await
            .context("reading workspace_members")?;

    let mut tuples = Vec::new();
    for (workspace_id, tenant_id) in workspaces {
        tuples.push(workspace_tenant_tuple(tenant_id, workspace_id));
    }
    for (workspace_id, user_id, role) in members {
        if let Some(relation) = role_relation(&role) {
            tuples.push(workspace_role_tuple(user_id, relation, workspace_id));
        } else {
            warn!(%workspace_id, %user_id, role, "skipping unknown workspace role");
        }
    }
    Ok(tuples)
}

async fn document_tuples(pool: &PgPool) -> anyhow::Result<Vec<RelationshipTuple>> {
    let rows: Vec<(Uuid, Uuid, Option<Uuid>, Uuid, String)> =
        sqlx::query_as("SELECT id, tenant_id, workspace_id, owner_id, visibility FROM documents")
            .fetch_all(pool)
            .await
            .context("reading documents")?;

    let mut tuples = Vec::new();
    for (document_id, tenant_id, workspace_id, owner_id, visibility) in rows {
        tuples.push(document_tenant_tuple(tenant_id, document_id));
        tuples.push(document_owner_tuple(owner_id, document_id));
        if let Some(workspace_id) = workspace_id {
            tuples.push(document_workspace_tuple(workspace_id, document_id));
        }
        if visibility == "shared" {
            tuples.push(document_shared_tuple(tenant_id, document_id));
        }
    }
    Ok(tuples)
}

async fn chat_session_tuples(pool: &PgPool) -> anyhow::Result<Vec<RelationshipTuple>> {
    let rows: Vec<(Uuid, Uuid, Option<Uuid>, Uuid)> =
        sqlx::query_as("SELECT id, tenant_id, workspace_id, user_id FROM chat_sessions")
            .fetch_all(pool)
            .await
            .context("reading chat_sessions")?;

    let mut tuples = Vec::new();
    for (session_id, tenant_id, workspace_id, owner_id) in rows {
        tuples.push(chat_tenant_tuple(tenant_id, session_id));
        tuples.push(chat_owner_tuple(owner_id, session_id));
        if let Some(workspace_id) = workspace_id {
            tuples.push(chat_workspace_tuple(workspace_id, session_id));
        }
    }
    Ok(tuples)
}

async fn old_resource_acl_tuples(pool: &PgPool) -> anyhow::Result<Vec<RelationshipTuple>> {
    let exists: bool = sqlx::query_scalar("SELECT to_regclass('public.resource_acl') IS NOT NULL")
        .fetch_one(pool)
        .await
        .context("checking resource_acl existence")?;
    if !exists {
        tracing::info!("resource_acl table is absent; skipping old grant import");
        return Ok(Vec::new());
    }

    let rows: Vec<(String, Uuid, String, Uuid, String)> = sqlx::query_as(
        "SELECT resource_type, resource_id, principal_type, principal_id, permission FROM resource_acl",
    )
    .fetch_all(pool)
    .await
    .context("reading resource_acl")?;

    let mut tuples = Vec::new();
    for (resource_type, resource_id, principal_type, principal_id, permission) in rows {
        let Some(object) = resource_object(&resource_type, resource_id) else {
            warn!(resource_type, %resource_id, "skipping unsupported ACL resource type");
            continue;
        };
        let Some(user) = principal_user(&principal_type, principal_id) else {
            warn!(principal_type, %principal_id, "skipping unsupported ACL principal type");
            continue;
        };
        if !matches!(permission.as_str(), REL_OWNER | REL_EDITOR | REL_VIEWER) {
            warn!(permission, "skipping unsupported ACL relation");
            continue;
        }
        if permission == REL_OWNER && !user.starts_with(&format!("{TYPE_USER}:")) {
            warn!(
                principal_type,
                %principal_id,
                "skipping non-user ACL owner grant; OpenFGA owner accepts direct users only"
            );
            continue;
        }
        tuples.push(tuple(user, permission, object));
    }
    Ok(tuples)
}

fn role_relation(role: &str) -> Option<&'static str> {
    match role {
        REL_OWNER => Some(REL_OWNER),
        REL_ADMIN => Some(REL_ADMIN),
        REL_MEMBER => Some(REL_MEMBER),
        _ => None,
    }
}

fn resource_object(resource_type: &str, resource_id: Uuid) -> Option<String> {
    match resource_type {
        TYPE_DOCUMENT => Some(document_obj(resource_id)),
        TYPE_CHAT_SESSION => Some(chat_session_obj(resource_id)),
        _ => None,
    }
}

fn principal_user(principal_type: &str, principal_id: Uuid) -> Option<String> {
    match principal_type {
        TYPE_USER => Some(user_obj(principal_id)),
        TYPE_WORKSPACE => Some(workspace_member_userset(principal_id)),
        _ => None,
    }
}
