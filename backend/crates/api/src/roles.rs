//! Canonical membership role vocabularies (Phase 0, TASK-P0-01).
//!
//! These enums are the single source of truth for the lowercase role strings
//! stored in `tenant_members.role` and `workspace_members.role`. They:
//!
//! - deserialize only the canonical lowercase values (rejecting anything else
//!   with a serde error, which the handlers map to HTTP 400 before any DB
//!   insert),
//! - serialize back to the canonical lowercase strings,
//! - expose an OpenAPI enum via [`utoipa::ToSchema`].
//!
//! They are intentionally distinct from OpenFGA resource relations
//! (`owner` / `editor` / `viewer` / `member`),
//! which describe ACL tuples on resources — not tenant/workspace membership.
//! `owner`/`member` overlap by name but live in different vocabularies; do
//! not mix them.
//!
//! Phase 0 does NOT change authorization semantics: `admin` is a valid stored
//! role but introduces no new powers here. Existing tenant `owner`-only guards
//! remain `owner`-only until Phase 1.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// `tenant_members.role` vocabulary: `owner` > `admin` > `member`.
///
/// `owner` is the highest role. `admin` is a valid stored role but Phase 0
/// grants it no new authorization powers. `member` is the default role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum TenantMemberRole {
    Owner,
    Admin,
    Member,
}

impl TenantMemberRole {
    /// Canonical lowercase storage string.
    pub fn as_str(self) -> &'static str {
        match self {
            TenantMemberRole::Owner => "owner",
            TenantMemberRole::Admin => "admin",
            TenantMemberRole::Member => "member",
        }
    }

    /// Parse a raw string into a [`TenantMemberRole`], rejecting unknown
    /// values. Trimmed before matching.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "owner" => Some(Self::Owner),
            "admin" => Some(Self::Admin),
            "member" => Some(Self::Member),
            _ => None,
        }
    }
}

impl std::fmt::Display for TenantMemberRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `workspace_members.role` vocabulary: `owner` > `admin` > `member`.
///
/// Same semantics as [`TenantMemberRole`] but for workspace membership.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceMemberRole {
    Owner,
    Admin,
    Member,
}

impl WorkspaceMemberRole {
    /// Canonical lowercase storage string.
    pub fn as_str(self) -> &'static str {
        match self {
            WorkspaceMemberRole::Owner => "owner",
            WorkspaceMemberRole::Admin => "admin",
            WorkspaceMemberRole::Member => "member",
        }
    }

    /// Parse a raw string into a [`WorkspaceMemberRole`], rejecting unknown
    /// values. Trimmed before matching.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "owner" => Some(Self::Owner),
            "admin" => Some(Self::Admin),
            "member" => Some(Self::Member),
            _ => None,
        }
    }
}

impl std::fmt::Display for WorkspaceMemberRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tenant_role_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&TenantMemberRole::Owner).unwrap(),
            "\"owner\""
        );
        assert_eq!(
            serde_json::to_string(&TenantMemberRole::Admin).unwrap(),
            "\"admin\""
        );
        assert_eq!(
            serde_json::to_string(&TenantMemberRole::Member).unwrap(),
            "\"member\""
        );
    }

    #[test]
    fn tenant_role_deserializes_only_canonical() {
        for s in ["owner", "admin", "member"] {
            let r: TenantMemberRole = serde_json::from_str(&format!("\"{s}\"")).unwrap();
            assert_eq!(r.as_str(), s);
        }
    }

    #[test]
    fn tenant_role_rejects_unknown_and_case_variants() {
        for bad in ["\"viewer\"", "\"OWNER\"", "\"editor\"", "\"\"", "\"root\""] {
            let r: Result<TenantMemberRole, _> = serde_json::from_str(bad);
            assert!(r.is_err(), "TenantMemberRole must reject {bad}");
        }
    }

    #[test]
    fn workspace_role_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&WorkspaceMemberRole::Owner).unwrap(),
            "\"owner\""
        );
        assert_eq!(
            serde_json::to_string(&WorkspaceMemberRole::Admin).unwrap(),
            "\"admin\""
        );
        assert_eq!(
            serde_json::to_string(&WorkspaceMemberRole::Member).unwrap(),
            "\"member\""
        );
    }

    #[test]
    fn workspace_role_rejects_non_member() {
        assert!(serde_json::from_str::<WorkspaceMemberRole>("\"editor\"").is_err());
        assert!(serde_json::from_str::<WorkspaceMemberRole>("\"viewer\"").is_err());
        assert!(serde_json::from_str::<WorkspaceMemberRole>("\"Owner\"").is_err());
    }

    #[test]
    fn parse_trims_input() {
        assert_eq!(
            TenantMemberRole::parse("  owner "),
            Some(TenantMemberRole::Owner)
        );
        assert_eq!(
            WorkspaceMemberRole::parse("\tmember\n"),
            Some(WorkspaceMemberRole::Member)
        );
        assert_eq!(TenantMemberRole::parse("viewer"), None);
        assert_eq!(WorkspaceMemberRole::parse(""), None);
    }

    #[test]
    fn display_matches_as_str() {
        assert_eq!(TenantMemberRole::Admin.to_string(), "admin");
        assert_eq!(WorkspaceMemberRole::Member.to_string(), "member");
    }
}
