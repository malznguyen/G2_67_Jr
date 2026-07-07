//! Phase 3 — OpenFGA reconciler integration tests.
//!
//! Verifies:
//! - correct categorization of `missing_in_openfga` and `orphaned_in_openfga`;
//! - a legitimate dynamic ACL grant on a LIVE document is NOT flagged orphaned
//!   (resource-existence check, not set-difference — see module docs);
//! - `auto_fix = true` writes missing tuples and deletes orphaned ones;
//! - `auto_fix = false` makes NO write/delete call even when drift is present.

use std::collections::HashSet;
use std::sync::Mutex;

use async_trait::async_trait;
use gmrag_api::authz::{
    AuthorizationService, AuthzError, CheckRequest, CheckResult, Consistency, RelationshipTuple,
};
use gmrag_api::reconcile::run_openfga_reconcile;
use sqlx::PgPool;
use uuid::Uuid;

/// Recording OpenFGA backend: `read_all` returns a seeded live set; every
/// `write_relationships` call is recorded so tests assert auto-fix gating.
struct RecordingAuthz {
    live: Mutex<HashSet<RelationshipTuple>>,
    writes: Mutex<Vec<RelationshipTuple>>,
    deletes: Mutex<Vec<RelationshipTuple>>,
}

impl RecordingAuthz {
    fn new(live: HashSet<RelationshipTuple>) -> Self {
        Self {
            live: Mutex::new(live),
            writes: Mutex::new(Vec::new()),
            deletes: Mutex::new(Vec::new()),
        }
    }
    fn writes(&self) -> Vec<RelationshipTuple> {
        self.writes.lock().unwrap().clone()
    }
    fn deletes(&self) -> Vec<RelationshipTuple> {
        self.deletes.lock().unwrap().clone()
    }
}

#[async_trait]
impl AuthorizationService for RecordingAuthz {
    async fn check(&self, _request: CheckRequest) -> Result<bool, AuthzError> {
        Ok(true)
    }
    async fn batch_check(
        &self,
        requests: Vec<CheckRequest>,
    ) -> Result<Vec<CheckResult>, AuthzError> {
        Ok(requests
            .into_iter()
            .map(|r| CheckResult {
                request: r,
                allowed: true,
            })
            .collect())
    }
    async fn list_objects(
        &self,
        _user: &str,
        _relation: &str,
        _object_type: &str,
        _consistency: Consistency,
    ) -> Result<Vec<String>, AuthzError> {
        Ok(Vec::new())
    }
    async fn read_direct_relationships(
        &self,
        object: &str,
    ) -> Result<Vec<RelationshipTuple>, AuthzError> {
        Ok(self
            .live
            .lock()
            .unwrap()
            .iter()
            .filter(|t| t.object == object)
            .cloned()
            .collect())
    }
    async fn read_all_direct_relationships(&self) -> Result<Vec<RelationshipTuple>, AuthzError> {
        Ok(self.live.lock().unwrap().iter().cloned().collect())
    }
    async fn write_relationships(
        &self,
        writes: Vec<RelationshipTuple>,
        deletes: Vec<RelationshipTuple>,
    ) -> Result<(), AuthzError> {
        let mut live = self.live.lock().unwrap();
        for t in &deletes {
            live.remove(t);
        }
        for t in &writes {
            live.insert(t.clone());
        }
        self.writes.lock().unwrap().extend(writes);
        self.deletes.lock().unwrap().extend(deletes);
        Ok(())
    }
    async fn delete_all_direct_relationships_for_object(
        &self,
        object: &str,
    ) -> Result<(), AuthzError> {
        self.live.lock().unwrap().retain(|t| t.object != object);
        Ok(())
    }
    async fn health(&self) -> Result<(), AuthzError> {
        Ok(())
    }
}

async fn insert_tenant(pool: &PgPool, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, name) VALUES ($1, $2)")
        .bind(id)
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn insert_user(pool: &PgPool, email: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, email, name) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(email)
        .bind(email)
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn insert_tenant_member(pool: &PgPool, tenant: Uuid, user: Uuid, role: &str) {
    sqlx::query("INSERT INTO tenant_members (tenant_id, user_id, role) VALUES ($1, $2, $3)")
        .bind(tenant)
        .bind(user)
        .bind(role)
        .execute(pool)
        .await
        .unwrap();
}

async fn insert_document(pool: &PgPool, tenant: Uuid, owner: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO documents (id, tenant_id, owner_id, title, status, visibility, s3_key)
         VALUES ($1, $2, $3, 'd', 'indexed', 'private', 'k')",
    )
    .bind(id)
    .bind(tenant)
    .bind(owner)
    .execute(pool)
    .await
    .unwrap();
    id
}

fn tenant_tuple(tenant: Uuid, user: Uuid, role: &str) -> RelationshipTuple {
    RelationshipTuple::new(format!("user:{user}"), role, format!("tenant:{tenant}"))
}
fn doc_owner_tuple(owner: Uuid, doc: Uuid) -> RelationshipTuple {
    RelationshipTuple::new(format!("user:{owner}"), "owner", format!("document:{doc}"))
}
// Matches authz::document_tenant_tuple: user=tenant:{tenant}, relation=tenant.
fn doc_tenant_tuple(tenant: Uuid, doc: Uuid) -> RelationshipTuple {
    RelationshipTuple::new(
        format!("tenant:{tenant}"),
        "tenant",
        format!("document:{doc}"),
    )
}
// A dynamic ACL grant (viewer on a live doc) — lives ONLY in OpenFGA.
fn doc_editor_tuple(user: Uuid, doc: Uuid) -> RelationshipTuple {
    RelationshipTuple::new(format!("user:{user}"), "viewer", format!("document:{doc}"))
}
// An orphaned tuple: references a document id that has NO Postgres row. Uses
// the real document_tenant shape so the orphan check sees a well-formed tuple
// whose object (the ghost document) no longer exists.
fn orphan_doc_tuple(tenant: Uuid, ghost_doc: Uuid) -> RelationshipTuple {
    RelationshipTuple::new(
        format!("tenant:{tenant}"),
        "tenant",
        format!("document:{ghost_doc}"),
    )
}

#[sqlx::test(migrations = "../../migrations")]
async fn openfga_reconcile_categorizes_and_preserves_dynamic_grants(pool: PgPool) {
    let tenant = insert_tenant(&pool, "rec-t").await;
    let owner = insert_user(&pool, "owner@rec.test").await;
    insert_tenant_member(&pool, tenant, owner, "owner").await;
    let doc = insert_document(&pool, tenant, owner).await;

    let _tenant_t = tenant_tuple(tenant, owner, "owner");
    let doc_owner_t = doc_owner_tuple(owner, doc);
    let doc_tenant_t = doc_tenant_tuple(tenant, doc);
    let dynamic_grant = doc_editor_tuple(owner, doc);
    let ghost = Uuid::new_v4();
    let orphan_t = orphan_doc_tuple(tenant, ghost);

    // Live set: drop the tenant tuple (→ missing), add a dynamic grant on a live
    // doc (must NOT be orphaned), and an orphan tuple on a ghost doc.
    let mut live = HashSet::new();
    live.insert(doc_owner_t.clone());
    live.insert(doc_tenant_t.clone());
    live.insert(dynamic_grant.clone());
    live.insert(orphan_t.clone());
    let authz = RecordingAuthz::new(live);

    let report = run_openfga_reconcile(&pool, &authz, false)
        .await
        .expect("reconcile");

    assert_eq!(
        report.missing_in_openfga.count, 1,
        "one structural tuple missing"
    );
    assert!(
        report
            .missing_in_openfga
            .sample
            .iter()
            .any(|s| s.contains(&format!("tenant:{tenant}"))),
        "missing tuple should reference the tenant"
    );
    assert_eq!(
        report.orphaned_in_openfga.count, 1,
        "one orphaned tuple (ghost doc)"
    );
    assert!(
        report
            .orphaned_in_openfga
            .sample
            .iter()
            .any(|s| s.contains(&format!("document:{ghost}"))),
        "orphaned tuple should reference the ghost document"
    );
    // The dynamic grant on a LIVE document must NOT be flagged orphaned.
    assert!(
        !report
            .orphaned_in_openfga
            .sample
            .iter()
            .any(|s| s.contains(&format!("document:{doc}"))),
        "dynamic grant on a live document must not be orphaned"
    );
    assert!(!report.auto_fix_ran);
    assert_eq!(report.written, 0);
    assert_eq!(report.deleted, 0);
    assert!(authz.writes().is_empty(), "dry-run must not write");
    assert!(authz.deletes().is_empty(), "dry-run must not delete");
}

#[sqlx::test(migrations = "../../migrations")]
async fn openfga_reconcile_auto_fix_writes_missing_and_deletes_orphaned(pool: PgPool) {
    let tenant = insert_tenant(&pool, "rec-fix").await;
    let owner = insert_user(&pool, "owner@rec-fix.test").await;
    insert_tenant_member(&pool, tenant, owner, "owner").await;
    let doc = insert_document(&pool, tenant, owner).await;

    let tenant_t = tenant_tuple(tenant, owner, "owner");
    let doc_owner_t = doc_owner_tuple(owner, doc);
    let doc_tenant_t = doc_tenant_tuple(tenant, doc);
    let ghost = Uuid::new_v4();
    let orphan_t = orphan_doc_tuple(tenant, ghost);

    let mut live = HashSet::new();
    live.insert(doc_owner_t.clone());
    live.insert(doc_tenant_t.clone());
    live.insert(orphan_t.clone());
    let authz = RecordingAuthz::new(live);

    let report = run_openfga_reconcile(&pool, &authz, true)
        .await
        .expect("reconcile auto-fix");

    assert!(report.auto_fix_ran);
    assert_eq!(report.written, 1, "missing tenant tuple written");
    assert_eq!(report.deleted, 1, "orphaned tuple deleted");

    let writes = authz.writes();
    assert!(
        writes.iter().any(|t| t == &tenant_t),
        "missing tuple written: {writes:?}"
    );
    let deletes = authz.deletes();
    assert!(
        deletes.iter().any(|t| t == &orphan_t),
        "orphaned tuple deleted: {deletes:?}"
    );

    let live_after = authz.live.lock().unwrap().clone();
    assert!(live_after.contains(&tenant_t), "written tuple now present");
    assert!(
        live_after.contains(&doc_owner_t),
        "untouched tuple preserved"
    );
    assert!(!live_after.contains(&orphan_t), "orphaned tuple removed");
}

#[sqlx::test(migrations = "../../migrations")]
async fn openfga_reconcile_dry_run_never_writes_or_deletes(pool: PgPool) {
    // Critical default-OFF invariant: drift present, auto_fix=false → zero
    // backend mutations.
    let tenant = insert_tenant(&pool, "rec-gate").await;
    let owner = insert_user(&pool, "owner@rec-gate.test").await;
    insert_tenant_member(&pool, tenant, owner, "owner").await;
    let doc = insert_document(&pool, tenant, owner).await;
    let ghost = Uuid::new_v4();

    let mut live = HashSet::new();
    live.insert(doc_owner_tuple(owner, doc));
    live.insert(orphan_doc_tuple(tenant, ghost));
    let authz = RecordingAuthz::new(live);

    let report = run_openfga_reconcile(&pool, &authz, false)
        .await
        .expect("reconcile");
    assert!(report.missing_in_openfga.count >= 1);
    assert_eq!(report.orphaned_in_openfga.count, 1);
    assert!(!report.auto_fix_ran);
    assert_eq!(report.written, 0);
    assert_eq!(report.deleted, 0);
    assert!(authz.writes().is_empty());
    assert!(authz.deletes().is_empty());
    assert!(authz
        .live
        .lock()
        .unwrap()
        .contains(&orphan_doc_tuple(tenant, ghost)));
}
