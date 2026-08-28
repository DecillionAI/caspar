//! Store access permissions — what a member (a person or a machine) may do
//! inside a store.
//!
//! Membership and permissions are two different facts and live in two
//! different links:
//!
//!   * `hasaccess::<userId>::<storeId>` = `"true"` — membership. The guard
//!     (`Guard::is_in_store`) checks this to decide whether the caller may
//!     address the store at all.
//!   * `onaccess::<storeId>::<memberId>` = a permission set — what the member
//!     may then do. This is the reverse index the node already kept, now
//!     carrying the grant instead of a bare `"true"`.
//!
//! The set is stored as a comma-separated list of flag names (`read,signal`)
//! so it stays greppable in the store and extensible without a migration.
//!
//! **Absent or unparseable means no permissions.** There is no implicit
//! default: a member whose grant was never written cannot read or signal, and
//! finds out immediately. That is deliberate — an implicit "full access" default
//! would silently hand a viewer the ability to post the first time a grant
//! failed to write.

use serde::{Deserialize, Serialize};

/// Read the store's persisted signals (`stores/history`).
pub const PERM_READ: &str = "read";
/// Send a signal into the store — the flag a viewer does NOT get, so a viewer
/// follows a space without being able to post into it.
pub const PERM_SIGNAL: &str = "signal";
/// Administer the store's membership and other members' permissions.
pub const PERM_MANAGE: &str = "manage";

/// One member's permissions within one store.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorePermissions {
    #[serde(default)]
    pub read: bool,
    #[serde(default)]
    pub signal: bool,
    #[serde(default)]
    pub manage: bool,
}

impl StorePermissions {
    /// The grant an owner/administrator holds.
    pub fn owner() -> Self {
        StorePermissions { read: true, signal: true, manage: true }
    }

    /// The grant an ordinary participant holds: reads the history, posts into
    /// the store, administers nothing.
    pub fn member() -> Self {
        StorePermissions { read: true, signal: true, manage: false }
    }

    /// The grant a viewer holds: follows the store, cannot post.
    pub fn viewer() -> Self {
        StorePermissions { read: true, signal: false, manage: false }
    }

    /// True when no flag is set — the state an absent or unparseable grant
    /// decodes to.
    pub fn is_empty(&self) -> bool {
        !self.read && !self.signal && !self.manage
    }

    /// Parse a stored link value. Unknown flag names are ignored (so a newer
    /// node's extra flag does not void an older node's read of the same grant),
    /// but nothing is inferred: a value naming no known flag yields an empty
    /// set, which denies.
    pub fn parse(raw: &str) -> Self {
        let mut out = StorePermissions::default();
        for part in raw.split(',') {
            match part.trim() {
                PERM_READ => out.read = true,
                PERM_SIGNAL => out.signal = true,
                PERM_MANAGE => out.manage = true,
                _ => {}
            }
        }
        out
    }

    /// Encode for storage as the `onaccess::` link value.
    pub fn encode(&self) -> String {
        let mut parts: Vec<&str> = Vec::with_capacity(3);
        if self.read {
            parts.push(PERM_READ);
        }
        if self.signal {
            parts.push(PERM_SIGNAL);
        }
        if self.manage {
            parts.push(PERM_MANAGE);
        }
        parts.join(",")
    }

    /// Build from an explicit list of flag names, as a caller supplies them
    /// over the wire or through a host call.
    pub fn from_list(flags: &[String]) -> Self {
        StorePermissions::parse(&flags.join(","))
    }
}

/// The `onaccess::<storeId>::<memberId>` link key.
pub fn access_link_key(store_id: &str, member_id: &str) -> String {
    format!("onaccess::{}::{}", store_id, member_id)
}

/// Read a member's permissions out of a transaction.
pub fn read_permissions(
    trx: &dyn crate::models::transaction::ITrx,
    store_id: &str,
    member_id: &str,
) -> StorePermissions {
    StorePermissions::parse(&trx.get_link(&access_link_key(store_id, member_id)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_the_link_value() {
        for perms in [
            StorePermissions::owner(),
            StorePermissions::member(),
            StorePermissions::viewer(),
        ] {
            assert_eq!(StorePermissions::parse(&perms.encode()), perms);
        }
    }

    #[test]
    fn viewer_cannot_signal() {
        let v = StorePermissions::viewer();
        assert!(v.read);
        assert!(!v.signal);
        assert_eq!(v.encode(), "read");
    }

    #[test]
    fn absent_grant_denies_everything() {
        let none = StorePermissions::parse("");
        assert!(none.is_empty());
        assert!(!none.read && !none.signal && !none.manage);
    }

    #[test]
    fn a_legacy_true_value_grants_nothing() {
        // Pre-permission nodes stored a bare "true". It names no flag, so it
        // denies rather than silently meaning "everything".
        assert!(StorePermissions::parse("true").is_empty());
    }

    #[test]
    fn unknown_flags_are_ignored_not_fatal() {
        let p = StorePermissions::parse("read,teleport,signal");
        assert!(p.read && p.signal && !p.manage);
    }

    #[test]
    fn from_list_matches_parse() {
        let p = StorePermissions::from_list(&["read".into(), "signal".into()]);
        assert_eq!(p, StorePermissions::member());
    }
}
