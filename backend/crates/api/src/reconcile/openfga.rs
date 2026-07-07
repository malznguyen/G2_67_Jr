//! Phase 3 — OpenFGA drift reconciler.
//!
//! Postgres is the source of truth. This compares the expected structural
//! tuple set (derived from Postgres via [`super::backfill::collect_tuples`])
//! against the live tuple set in OpenFGA, reports drift, and — only when
//! `auto_fix` is explicitly enabled — repairs it.
//!
//! ## Why orphan detection is NOT a set-difference
//!
//! Dynamic ACL grants (editor/viewer on a document or chat_session, created
//! via the `/acl` API) are stored ONLY in OpenFGA — there is no Postgres
//! grants table (`acl.rs` writes the tuple + an `audit_log` row, nothing
//! else). So `expected` (the structural tuple set derived from membership /
//! resource rows) is a SUBSET of what a healthy OpenFGA store contains. A
//! naive `orphaned = live − expected` would flag EVERY legitimate dynamic
//! grant as orphaned and, with auto-fix enabled, delete them — a catastrophic
//! data-loss bug.
//!
//! Instead, orphan detection is **resource-existence-based**: a live tuple is
//! orphaned only when the Postgres entity it references (its `object`, or the
//! `user` / userset principal) no longer exists. This catches the real
//! failure mode this phase targets — a deleted document/workspace/tenant
//! whose OpenFGA cleanup failed mid-write — while preserving every live
//! dynamic grant on a live resource. Malformed tuples (unparseable
//! `type:uuid`) are reported separately and never auto-deleted, since their
//! referenced entity cannot be verified.
//!
//! Auto-fix default is OFF. When `auto_fix = false`, this function makes NO
//! `write_relationships` / `delete_*` call to OpenFGA — verified by test.

use std::collections::HashSet;

use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::authz::{
    AuthorizationService, RelationshipTuple, TYPE_CHAT_SESSION, TYPE_DOCUMENT, TYPE_TENANT,
    TYPE_USER, TYPE_WORKSPACE,
};

use super::backfill::collect_tuples;

/// Bounded sample size per drift category (keeps logs/binary output bounded;
/// the full set is still counted).
pub const SAMPLE_LIMIT: usize = 50;
/// Write/delete batch size — matches the OpenFGA client's internal chunking.
const WRITE_BATCH: usize = 100;

#[derive(Debug, Clone, Serialize)]
pub struct CategoryReport {
    pub count: usize,
    pub sample: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OpenFgaReport {
    pub missing_in_openfga: CategoryReport,
    pub orphaned_in_openfga: CategoryReport,
    pub malformed: CategoryReport,
    pub auto_fix_ran: bool,
    pub written: usize,
    pub deleted: usize,
}

fn empty_category() -> CategoryReport {
    CategoryReport {
        count: 0,
        sample: Vec::new(),
    }
}

fn tuple_key(t: &RelationshipTuple) -> String {
    format!("{} {} {}", t.user, t.relation, t.object)
}

fn push_sample(cat: &mut CategoryReport, key: String) {
    cat.count += 1;
    if cat.sample.len() < SAMPLE_LIMIT {
        cat.sample.push(key);
    }
}

/// Parse `type:uuid` or `type:uuid#relation` into `(type, uuid)`.
fn parse_typed_uuid(s: &str) -> Option<(&str, Uuid)> {
    let (ty, rest) = s.split_once(':')?;
    let raw = rest.split_once('#').map(|(id, _)| id).unwrap_or(rest);
    let id = Uuid::parse_str(raw).ok()?;
    Some((ty, id))
}

/// Live Postgres entity id sets used for the resource-existence orphan check.
struct LiveEntities {
    users: HashSet<Uuid>,
    tenants: HashSet<Uuid>,
    workspaces: HashSet<Uuid>,
    documents: HashSet<Uuid>,
    chat_sessions: HashSet<Uuid>,
}

impl LiveEntities {
    /// `true` iff the entity referenced by `type:uuid` (optionally with a
    /// `#relation` suffix) still exists in Postgres. Malformed refs → `false`.
    fn exists(&self, typed: &str) -> bool {
        let Some((ty, id)) = parse_typed_uuid(typed) else {
            return false;
        };
        match ty {
            TYPE_USER => self.users.contains(&id),
            TYPE_TENANT => self.tenants.contains(&id),
            TYPE_WORKSPACE => self.workspaces.contains(&id),
            TYPE_DOCUMENT => self.documents.contains(&id),
            TYPE_CHAT_SESSION => self.chat_sessions.contains(&id),
            _ => false,
        }
    }
}

async fn load_live_entities(pool: &PgPool) -> anyhow::Result<LiveEntities> {
    let users = sqlx::query_scalar("SELECT id FROM users")
        .fetch_all(pool)
        .await?;
    let tenants = sqlx::query_scalar("SELECT id FROM tenants")
        .fetch_all(pool)
        .await?;
    let workspaces = sqlx::query_scalar("SELECT id FROM workspaces")
        .fetch_all(pool)
        .await?;
    let documents = sqlx::query_scalar("SELECT id FROM documents")
        .fetch_all(pool)
        .await?;
    let chat_sessions = sqlx::query_scalar("SELECT id FROM chat_sessions")
        .fetch_all(pool)
        .await?;
    Ok(LiveEntities {
        users: users.into_iter().collect(),
        tenants: tenants.into_iter().collect(),
        workspaces: workspaces.into_iter().collect(),
        documents: documents.into_iter().collect(),
        chat_sessions: chat_sessions.into_iter().collect(),
    })
}

/// Run one OpenFGA reconciliation pass.
///
/// `auto_fix = false` (the default) → report-only: NO writes, NO deletes,
/// ever. `auto_fix = true` → write missing tuples and delete orphaned tuples,
/// logging each write/delete individually with before/after state.
pub async fn run_openfga_reconcile(
    pool: &PgPool,
    authz: &dyn AuthorizationService,
    auto_fix: bool,
) -> anyhow::Result<OpenFgaReport> {
    // 1. Expected structural tuples from Postgres (source of truth).
    let (expected_vec, _counts) = collect_tuples(pool).await?;
    let expected: HashSet<RelationshipTuple> = expected_vec.into_iter().collect();

    // 2. Live tuples currently in OpenFGA.
    let live_vec = authz
        .read_all_direct_relationships()
        .await
        .map_err(|e| anyhow::anyhow!("read_all_direct_relationships: {e}"))?;
    let live: HashSet<RelationshipTuple> = live_vec.into_iter().collect();

    // 3. Missing = expected structural tuples not present in OpenFGA.
    let mut missing_tuples: Vec<RelationshipTuple> = Vec::new();
    for t in &expected {
        if !live.contains(t) {
            missing_tuples.push(t.clone());
        }
    }

    // 4. Orphaned = live tuples whose referenced Postgres entity no longer
    //    exists (resource-existence check — NOT set-difference; see module
    //    docs). Malformed tuples are reported separately, never auto-deleted.
    let entities = load_live_entities(pool).await?;
    let mut orphaned_tuples: Vec<RelationshipTuple> = Vec::new();
    let mut malformed_tuples: Vec<RelationshipTuple> = Vec::new();
    for t in &live {
        // A structural expected tuple is, by construction, live-backed.
        if expected.contains(t) {
            continue;
        }
        let obj_ok = parse_typed_uuid(&t.object).is_some();
        let user_ok = parse_typed_uuid(&t.user).is_some();
        if !obj_ok || !user_ok {
            malformed_tuples.push(t.clone());
            continue;
        }
        if !entities.exists(&t.object) || !entities.exists(&t.user) {
            orphaned_tuples.push(t.clone());
        }
    }

    let mut missing = empty_category();
    for t in &missing_tuples {
        push_sample(&mut missing, tuple_key(t));
    }
    let mut orphaned = empty_category();
    for t in &orphaned_tuples {
        push_sample(&mut orphaned, tuple_key(t));
    }
    let mut malformed = empty_category();
    for t in &malformed_tuples {
        push_sample(&mut malformed, tuple_key(t));
    }

    tracing::info!(
        missing = missing.count,
        orphaned = orphaned.count,
        malformed = malformed.count,
        auto_fix,
        "openfga reconcile: drift report"
    );

    let mut report = OpenFgaReport {
        missing_in_openfga: missing,
        orphaned_in_openfga: orphaned,
        malformed,
        auto_fix_ran: false,
        written: 0,
        deleted: 0,
    };

    // 5. Repair — ONLY when auto_fix is true. When false, return now: no
    //    write/delete call is ever made to OpenFGA.
    if !auto_fix {
        return Ok(report);
    }
    report.auto_fix_ran = true;

    // Write missing tuples (the OpenFGA client chunks internally too; we
    // chunk here only so each write is logged individually).
    for t in &missing_tuples {
        tracing::info!(
            before = "absent", after = "present", key = %tuple_key(t),
            "openfga reconcile: WRITE missing tuple"
        );
    }
    for chunk in missing_tuples.chunks(WRITE_BATCH) {
        authz
            .write_relationships(chunk.to_vec(), Vec::new())
            .await
            .map_err(|e| anyhow::anyhow!("write missing tuples: {e}"))?;
    }
    report.written = missing_tuples.len();

    // Delete orphaned tuples.
    for t in &orphaned_tuples {
        tracing::info!(
            before = "present", after = "absent", key = %tuple_key(t),
            "openfga reconcile: DELETE orphaned tuple"
        );
    }
    for chunk in orphaned_tuples.chunks(WRITE_BATCH) {
        authz
            .write_relationships(Vec::new(), chunk.to_vec())
            .await
            .map_err(|e| anyhow::anyhow!("delete orphaned tuples: {e}"))?;
    }
    report.deleted = orphaned_tuples.len();

    tracing::info!(
        written = report.written,
        deleted = report.deleted,
        "openfga reconcile: auto-fix complete"
    );
    Ok(report)
}
