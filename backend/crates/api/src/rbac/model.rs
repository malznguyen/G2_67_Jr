//! ReBAC relation model + namespace configuration (Zanzibar-style).
//!
//! This module is the pure, storage-agnostic *policy* layer: it declares the
//! object namespaces, the relation vocabulary, and the userset-rewrite rules
//! (Zanzibar §2.3.1 — paper `docs/5068.pdf`). It performs no I/O; the
//! storage adapters that resolve `_this` / `tuple_to_userset` leaves against
//! PostgreSQL live in [`crate::rbac::check`].
//!
//! MVP scope:
//! - namespaces: `document`, `chat_session`, `workspace`
//! - relations: `owner`, `editor`, `viewer`, `member`
//! - rewrites: concentric `viewer ⊇ editor ⊇ owner` (`computed_userset`) plus
//!   inheritance of `viewer` from the parent workspace's `member`
//!   (`tuple_to_userset`).

use uuid::Uuid;

/// Object namespace: a tenant-scoped resource that owns ACL tuples.
pub const NS_DOCUMENT: &str = "document";
pub const NS_CHAT_SESSION: &str = "chat_session";
pub const NS_WORKSPACE: &str = "workspace";

/// Relations understood by the engine (Zanzibar relation names).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Relation {
    Owner,
    Editor,
    Viewer,
    /// Group membership (only meaningful on the `workspace` namespace).
    Member,
}

impl Relation {
    pub fn as_str(self) -> &'static str {
        match self {
            Relation::Owner => "owner",
            Relation::Editor => "editor",
            Relation::Viewer => "viewer",
            Relation::Member => "member",
        }
    }

    pub fn parse(s: &str) -> Option<Relation> {
        match s {
            "owner" => Some(Relation::Owner),
            "editor" => Some(Relation::Editor),
            "viewer" => Some(Relation::Viewer),
            "member" => Some(Relation::Member),
            _ => None,
        }
    }

    /// Relations that may be granted directly via `resource_acl` tuples.
    /// `member` is derived from `workspace_members`, never granted here.
    pub fn is_grantable(self) -> bool {
        matches!(self, Relation::Owner | Relation::Editor | Relation::Viewer)
    }
}

/// A reference to an object: `namespace:id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectRef {
    pub namespace: String,
    pub id: Uuid,
}

impl ObjectRef {
    pub fn new(namespace: impl Into<String>, id: Uuid) -> Self {
        Self {
            namespace: namespace.into(),
            id,
        }
    }
}

/// A subject (Zanzibar "user"): a direct user, or a workspace userset (group)
/// whose members all inherit the grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Principal {
    User(Uuid),
    Workspace(Uuid),
}

impl Principal {
    pub fn type_str(self) -> &'static str {
        match self {
            Principal::User(_) => "user",
            Principal::Workspace(_) => "workspace",
        }
    }

    pub fn id(self) -> Uuid {
        match self {
            Principal::User(id) | Principal::Workspace(id) => id,
        }
    }

    pub fn from_parts(principal_type: &str, id: Uuid) -> Option<Principal> {
        match principal_type {
            "user" => Some(Principal::User(id)),
            "workspace" => Some(Principal::Workspace(id)),
            _ => None,
        }
    }
}

/// A parent edge followed by `tuple_to_userset` (the object's "parent" object).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParentEdge {
    /// The owning workspace of a document / chat session.
    Workspace,
}

/// A single leaf of a userset-rewrite expression (Zanzibar §2.3.1).
///
/// The effective userset for `(namespace, relation)` is the **union** of the
/// ops returned by [`rewrite_for`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RewriteOp {
    /// `_this`: subjects from stored tuples for this `object#relation`
    /// (plus, for relations backed by a column, the row's own owner/member).
    This,
    /// `computed_userset`: also include subjects of another relation on the
    /// **same** object (concentric relations).
    ComputedUserset(Relation),
    /// `tuple_to_userset`: follow the object's parent edge, then include
    /// subjects of `computed` on that parent object.
    TupleToUserset { tupleset: ParentEdge, computed: Relation },
}

/// The userset-rewrite rule for a `(namespace, relation)` pair.
///
/// Returns the union of [`RewriteOp`]s. An empty vector means the relation is
/// not defined for that namespace (so it can never be satisfied).
pub fn rewrite_for(namespace: &str, relation: Relation) -> Vec<RewriteOp> {
    match (namespace, relation) {
        // ── document / chat_session share the same object-sharing config ──
        (NS_DOCUMENT | NS_CHAT_SESSION, Relation::Owner) => vec![RewriteOp::This],
        (NS_DOCUMENT | NS_CHAT_SESSION, Relation::Editor) => {
            vec![RewriteOp::This, RewriteOp::ComputedUserset(Relation::Owner)]
        }
        (NS_DOCUMENT | NS_CHAT_SESSION, Relation::Viewer) => vec![
            RewriteOp::This,
            // editor ⊇ owner, so viewer transitively contains both.
            RewriteOp::ComputedUserset(Relation::Editor),
            // Inherit viewer from the parent workspace's members.
            RewriteOp::TupleToUserset {
                tupleset: ParentEdge::Workspace,
                computed: Relation::Member,
            },
        ],

        // ── workspace ──
        (NS_WORKSPACE, Relation::Member) => vec![RewriteOp::This],

        // Undefined combinations have no rule → never satisfied.
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relation_round_trips_through_str() {
        for r in [
            Relation::Owner,
            Relation::Editor,
            Relation::Viewer,
            Relation::Member,
        ] {
            assert_eq!(Relation::parse(r.as_str()), Some(r));
        }
        assert_eq!(Relation::parse("nope"), None);
    }

    #[test]
    fn only_owner_editor_viewer_are_grantable() {
        assert!(Relation::Owner.is_grantable());
        assert!(Relation::Editor.is_grantable());
        assert!(Relation::Viewer.is_grantable());
        assert!(!Relation::Member.is_grantable(), "member is derived, not granted");
    }

    #[test]
    fn principal_parts_round_trip() {
        let id = Uuid::new_v4();
        let u = Principal::from_parts("user", id).unwrap();
        assert_eq!(u, Principal::User(id));
        assert_eq!(u.type_str(), "user");
        assert_eq!(u.id(), id);

        let w = Principal::from_parts("workspace", id).unwrap();
        assert_eq!(w, Principal::Workspace(id));
        assert_eq!(w.type_str(), "workspace");

        assert_eq!(Principal::from_parts("robot", id), None);
    }

    #[test]
    fn document_owner_is_direct_only() {
        assert_eq!(rewrite_for(NS_DOCUMENT, Relation::Owner), vec![RewriteOp::This]);
    }

    #[test]
    fn document_editor_is_concentric_with_owner() {
        let ops = rewrite_for(NS_DOCUMENT, Relation::Editor);
        assert!(ops.contains(&RewriteOp::This));
        assert!(ops.contains(&RewriteOp::ComputedUserset(Relation::Owner)));
    }

    #[test]
    fn document_viewer_includes_editor_and_workspace_inheritance() {
        let ops = rewrite_for(NS_DOCUMENT, Relation::Viewer);
        assert!(ops.contains(&RewriteOp::This));
        assert!(ops.contains(&RewriteOp::ComputedUserset(Relation::Editor)));
        assert!(ops.contains(&RewriteOp::TupleToUserset {
            tupleset: ParentEdge::Workspace,
            computed: Relation::Member,
        }));
    }

    #[test]
    fn chat_session_mirrors_document_sharing() {
        assert_eq!(
            rewrite_for(NS_CHAT_SESSION, Relation::Viewer),
            rewrite_for(NS_DOCUMENT, Relation::Viewer)
        );
    }

    #[test]
    fn workspace_member_is_direct_only() {
        assert_eq!(rewrite_for(NS_WORKSPACE, Relation::Member), vec![RewriteOp::This]);
    }

    #[test]
    fn undefined_namespace_relation_has_no_rule() {
        assert!(rewrite_for(NS_WORKSPACE, Relation::Viewer).is_empty());
        assert!(rewrite_for(NS_DOCUMENT, Relation::Member).is_empty());
        assert!(rewrite_for("unknown_ns", Relation::Viewer).is_empty());
    }

    /// Concentric expansion must terminate: walking `computed_userset` edges
    /// from `viewer` reaches `owner` without cycling.
    #[test]
    fn concentric_expansion_terminates_and_reaches_owner() {
        fn collect(ns: &str, rel: Relation, depth: usize, acc: &mut Vec<Relation>) {
            assert!(depth < 16, "rewrite expansion must be bounded (no cycles)");
            for op in rewrite_for(ns, rel) {
                if let RewriteOp::ComputedUserset(next) = op {
                    acc.push(next);
                    collect(ns, next, depth + 1, acc);
                }
            }
        }
        let mut reached = Vec::new();
        collect(NS_DOCUMENT, Relation::Viewer, 0, &mut reached);
        assert!(reached.contains(&Relation::Owner));
        assert!(reached.contains(&Relation::Editor));
    }
}
