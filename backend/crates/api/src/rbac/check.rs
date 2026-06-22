//! ReBAC `Check` API (Zanzibar §2.4.4, paper `docs/5068.pdf`).
//!
//! Answers "does `principal` have `relation` on `object`?" by recursively
//! evaluating the userset-rewrite tree from [`crate::rbac::model`] against
//! PostgreSQL. Every query runs on the caller-supplied [`PgConnection`], which
//! in production is the request's RLS-scoped transaction connection — so the
//! check shares the tenant-isolation context with every other query and a
//! grant from another tenant is simply invisible (returns `false`).
//!
//! Evaluation strategy (Zanzibar §3.2.3): leaf nodes are short-circuited — the
//! first leaf that resolves `true` ends the search. Recursion is depth-bounded
//! ([`MAX_DEPTH`]) to guarantee termination even if the namespace config were
//! mis-edited into a cycle.

use futures::future::BoxFuture;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::rbac::model::{
    rewrite_for, ObjectRef, ParentEdge, Principal, Relation, RewriteOp, NS_CHAT_SESSION,
    NS_DOCUMENT, NS_WORKSPACE,
};

/// Maximum userset-rewrite recursion depth (defensive bound).
const MAX_DEPTH: usize = 16;

/// Errors returned by the Check engine.
#[derive(Debug, thiserror::Error)]
pub enum CheckError {
    #[error("db error: {0}")]
    Db(#[from] sqlx::Error),

    #[error("rebac check recursion exceeded depth {0}")]
    DepthExceeded(usize),
}

/// `Check(object#relation@principal)` — the public entry point.
///
/// Returns `Ok(true)` iff the principal is a member of the effective userset
/// for `object#relation`.
pub async fn check_relation(
    conn: &mut PgConnection,
    object: &ObjectRef,
    relation: Relation,
    principal: Principal,
) -> Result<bool, CheckError> {
    check_inner(conn, object.clone(), relation, principal, 0).await
}

/// Recursive worker. Boxed because Rust async fns cannot recurse directly.
fn check_inner(
    conn: &mut PgConnection,
    object: ObjectRef,
    relation: Relation,
    principal: Principal,
    depth: usize,
) -> BoxFuture<'_, Result<bool, CheckError>> {
    Box::pin(async move {
        if depth > MAX_DEPTH {
            return Err(CheckError::DepthExceeded(MAX_DEPTH));
        }

        for op in rewrite_for(&object.namespace, relation) {
            match op {
                RewriteOp::This => {
                    if eval_this(conn, &object, relation, principal).await? {
                        return Ok(true);
                    }
                }
                RewriteOp::ComputedUserset(other) => {
                    if check_inner(conn, object.clone(), other, principal, depth + 1).await? {
                        return Ok(true);
                    }
                }
                RewriteOp::TupleToUserset { tupleset, computed } => {
                    if let Some(parent) = resolve_parent(conn, &object, tupleset).await? {
                        if check_inner(conn, parent, computed, principal, depth + 1).await? {
                            return Ok(true);
                        }
                    }
                }
            }
        }
        Ok(false)
    })
}

/// Resolve the `_this` leaf: subjects materialised directly for
/// `object#relation` (own-column owners/members, public share, and stored
/// `resource_acl` grants).
async fn eval_this(
    conn: &mut PgConnection,
    object: &ObjectRef,
    relation: Relation,
    principal: Principal,
) -> Result<bool, CheckError> {
    match (object.namespace.as_str(), relation) {
        // Ownership is the resource's own owner column.
        (NS_DOCUMENT, Relation::Owner) => {
            owner_is(conn, "documents", "owner_id", object.id, principal).await
        }
        (NS_CHAT_SESSION, Relation::Owner) => {
            owner_is(conn, "chat_sessions", "user_id", object.id, principal).await
        }

        // Workspace membership lives in workspace_members.
        (NS_WORKSPACE, Relation::Member) => workspace_member(conn, object.id, principal).await,

        // Document viewer also honours the legacy public `shared` visibility.
        (NS_DOCUMENT, Relation::Viewer) => {
            if matches!(principal, Principal::User(_)) && document_is_shared(conn, object.id).await?
            {
                return Ok(true);
            }
            grant_tuple(conn, NS_DOCUMENT, object.id, Relation::Viewer, principal).await
        }

        // Remaining grantable object relations are stored resource_acl tuples.
        (NS_DOCUMENT, Relation::Editor) => {
            grant_tuple(conn, NS_DOCUMENT, object.id, Relation::Editor, principal).await
        }
        (NS_CHAT_SESSION, Relation::Editor) => {
            grant_tuple(conn, NS_CHAT_SESSION, object.id, Relation::Editor, principal).await
        }
        (NS_CHAT_SESSION, Relation::Viewer) => {
            grant_tuple(conn, NS_CHAT_SESSION, object.id, Relation::Viewer, principal).await
        }

        // Anything else has no direct materialisation.
        _ => Ok(false),
    }
}

/// `owner_is`: the resource row's own owner column equals the (user) principal.
///
/// `table`/`column` are compile-time constants (never request input), so the
/// inlined identifiers are injection-safe.
async fn owner_is(
    conn: &mut PgConnection,
    table: &'static str,
    column: &'static str,
    id: Uuid,
    principal: Principal,
) -> Result<bool, CheckError> {
    let user_id = match principal {
        Principal::User(u) => u,
        // Only a concrete user can *be* an owner; a group cannot.
        Principal::Workspace(_) => return Ok(false),
    };
    let sql = format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE id = $1 AND {column} = $2)");
    let exists: bool = sqlx::query_scalar(&sql)
        .bind(id)
        .bind(user_id)
        .fetch_one(conn)
        .await?;
    Ok(exists)
}

/// Direct membership in `workspace_members`.
async fn workspace_member(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    principal: Principal,
) -> Result<bool, CheckError> {
    let user_id = match principal {
        Principal::User(u) => u,
        Principal::Workspace(_) => return Ok(false),
    };
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM workspace_members
         WHERE workspace_id = $1 AND user_id = $2)",
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_one(conn)
    .await?;
    Ok(exists)
}

/// A document is publicly readable.
async fn document_is_shared(conn: &mut PgConnection, id: Uuid) -> Result<bool, CheckError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM documents WHERE id = $1 AND visibility = 'shared')",
    )
    .bind(id)
    .fetch_one(conn)
    .await?;
    Ok(exists)
}

/// A stored `resource_acl` grant of exactly `relation` for the principal.
///
/// For a user principal this matches both a direct `user` grant and any
/// `workspace` (group) grant whose membership includes the user.
async fn grant_tuple(
    conn: &mut PgConnection,
    resource_type: &str,
    resource_id: Uuid,
    relation: Relation,
    principal: Principal,
) -> Result<bool, CheckError> {
    match principal {
        Principal::User(user_id) => {
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                   SELECT 1 FROM resource_acl ra
                   WHERE ra.resource_type = $1
                     AND ra.resource_id = $2
                     AND ra.permission = $3
                     AND (
                       (ra.principal_type = 'user' AND ra.principal_id = $4)
                       OR (ra.principal_type = 'workspace' AND EXISTS (
                             SELECT 1 FROM workspace_members wm
                             WHERE wm.workspace_id = ra.principal_id
                               AND wm.user_id = $4))
                     )
                 )",
            )
            .bind(resource_type)
            .bind(resource_id)
            .bind(relation.as_str())
            .bind(user_id)
            .fetch_one(conn)
            .await?;
            Ok(exists)
        }
        Principal::Workspace(ws_id) => {
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                   SELECT 1 FROM resource_acl
                   WHERE resource_type = $1 AND resource_id = $2 AND permission = $3
                     AND principal_type = 'workspace' AND principal_id = $4
                 )",
            )
            .bind(resource_type)
            .bind(resource_id)
            .bind(relation.as_str())
            .bind(ws_id)
            .fetch_one(conn)
            .await?;
            Ok(exists)
        }
    }
}

/// Resolve the object's parent for `tuple_to_userset` (its owning workspace).
async fn resolve_parent(
    conn: &mut PgConnection,
    object: &ObjectRef,
    edge: ParentEdge,
) -> Result<Option<ObjectRef>, CheckError> {
    let ParentEdge::Workspace = edge;
    let table = match object.namespace.as_str() {
        NS_DOCUMENT => "documents",
        NS_CHAT_SESSION => "chat_sessions",
        _ => return Ok(None),
    };
    let sql = format!("SELECT workspace_id FROM {table} WHERE id = $1");
    let ws: Option<Uuid> = sqlx::query_scalar(&sql)
        .bind(object.id)
        .fetch_optional(conn)
        .await?
        .flatten();
    Ok(ws.map(|id| ObjectRef::new(NS_WORKSPACE, id)))
}
