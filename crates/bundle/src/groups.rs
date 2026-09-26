//! Shared storage groups, named by their publisher (IC-738).
//!
//! Two apps sharing data used to be an accident: reuse an app id and every
//! store derived from it was shared, with nobody having decided that. A
//! group makes sharing a deliberate act with a named owner and a named
//! membership:
//!
//! - the publisher ROOT signs a membership list: this group, these app ids,
//!   issued at this moment;
//! - an app may use the group only if the newest list known on this machine,
//!   from its own publisher root, names it;
//! - a newer list replaces an older one and an older one arriving later is
//!   refused, so removing an app from the list -- revocation -- sticks even
//!   when an old copy of the list turns up again inside an old bundle.
//!
//! The root signs, not a release key, for the same reason it signs
//! delegations: deciding who shares a publisher's data is a deliberate
//! publisher-level act, and a stolen release key must not be able to add an
//! app to a group. The list travels in the bundle's signature envelope, so a
//! recipient offline learns it from the app that carries it; how a newer
//! list reaches a machine that has only old bundles is the network feed
//! ADR-0016 gates, exactly as for revocations.
//!
//! This module decides; it holds no files. The caller keeps the newest
//! accepted list per (root, group) and asks [`accept`] whether an offered
//! one replaces it, and [`access`] whether an app is a member.

use serde::{Deserialize, Serialize};

use crate::signing::SigningKey;

/// Version of the membership encoding. Different from every other signed
/// record's schema, and the schema is the first signed field, so a
/// membership list can never be read as a delegation or a revocation list.
pub const GROUP_MEMBERSHIP_SCHEMA: &str = "krate.group.v1";

/// A publisher root's statement of which of its apps share a group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupMembership {
    pub schema: String,
    /// The publisher root that owns the group, lowercase hex.
    pub root: String,
    /// The group's name, as the manifest's `store.group:<name>` spells it.
    pub group: String,
    /// Unix seconds. Orders lists: a newer one replaces an older one.
    pub issued_at: u64,
    /// The app ids that may use the group. Nothing else may.
    pub members: Vec<String>,
}

impl GroupMembership {
    /// The exact bytes the root signs. Length-prefixed fields, members in
    /// sorted order so two lists naming the same apps in a different order
    /// are the same statement.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        fn field(out: &mut Vec<u8>, bytes: &[u8]) {
            out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
            out.extend_from_slice(bytes);
        }
        let mut members = self.members.clone();
        members.sort();
        members.dedup();
        let mut out = Vec::new();
        field(&mut out, self.schema.as_bytes());
        field(&mut out, self.root.as_bytes());
        field(&mut out, self.group.as_bytes());
        out.extend_from_slice(&self.issued_at.to_le_bytes());
        out.extend_from_slice(&(members.len() as u64).to_le_bytes());
        for member in &members {
            field(&mut out, member.as_bytes());
        }
        out
    }

    fn names(&self, app_id: &str) -> bool {
        self.members.iter().any(|member| member == app_id)
    }
}

/// A membership list with the root's signature over it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedGroupMembership {
    pub membership: GroupMembership,
    /// Lowercase hex Ed25519 signature by the root named in the list.
    pub signature: String,
}

/// Why a membership list is not accepted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GroupError {
    #[error("this group list was written by a newer Krate ({schema}); update to read it")]
    UnknownSchema { schema: String },
    #[error("the group list's signature does not verify against the publisher it names; it was altered or forged")]
    Forged,
    #[error("the group list names a publisher that is not a valid key")]
    BadRoot,
    #[error("the group list is for {found:?}, not {expected:?}")]
    WrongGroup { expected: String, found: String },
    #[error("the group list is signed by a different publisher than the app's")]
    WrongPublisher,
    #[error(
        "this group list (issued {offered}) is older than the one already known \
         (issued {known}); an older list cannot undo a newer one"
    )]
    Rollback { known: u64, offered: u64 },
    #[error("two different group lists claim the same moment ({issued_at}); neither is taken")]
    Conflict { issued_at: u64 },
}

impl SignedGroupMembership {
    /// Sign a list with the publisher root. `root` is set from the key, so
    /// a list can never name a root other than its signer.
    pub fn create(root: &SigningKey, mut membership: GroupMembership) -> SignedGroupMembership {
        membership.schema = GROUP_MEMBERSHIP_SCHEMA.to_string();
        membership.root = hex(&root.public_key());
        membership.members.sort();
        membership.members.dedup();
        let signature = root.sign_delegation_bytes(&membership.canonical_bytes());
        SignedGroupMembership {
            membership,
            signature: hex(&signature),
        }
    }

    /// Check the root's signature. Nothing in the list is read for any
    /// decision before this passes.
    pub fn verify(&self) -> Result<&GroupMembership, GroupError> {
        if self.membership.schema != GROUP_MEMBERSHIP_SCHEMA {
            return Err(GroupError::UnknownSchema {
                schema: self.membership.schema.clone(),
            });
        }
        let Some(root_key) = unhex(&self.membership.root) else {
            return Err(GroupError::BadRoot);
        };
        let Some(signature) = unhex(&self.signature) else {
            return Err(GroupError::Forged);
        };
        let root = ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, &root_key);
        root.verify(&self.membership.canonical_bytes(), &signature)
            .map_err(|_| GroupError::Forged)?;
        Ok(&self.membership)
    }
}

/// What to keep after an offered list is judged against the known one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Accepted {
    /// The offered list is newer (or the first seen): keep it.
    Replace,
    /// The offered list is the one already known.
    Unchanged,
}

/// Judge an offered list for `(root, group)` against the one already known.
///
/// Refused unless it verifies, names this group and this publisher, and is
/// not older than what is known. An older list is a rollback -- the way an
/// old bundle would re-admit a member the publisher has since removed -- and
/// is refused, not merged. Two different lists issued at the same second are
/// refused too: there is no honest way to choose between them.
pub fn accept(
    known: Option<&SignedGroupMembership>,
    offered: &SignedGroupMembership,
    root: &str,
    group: &str,
) -> Result<Accepted, GroupError> {
    let membership = offered.verify()?;
    if membership.group != group {
        return Err(GroupError::WrongGroup {
            expected: group.to_string(),
            found: membership.group.clone(),
        });
    }
    if membership.root != root {
        return Err(GroupError::WrongPublisher);
    }
    let Some(known) = known else {
        return Ok(Accepted::Replace);
    };
    let current = &known.membership;
    if membership.issued_at < current.issued_at {
        return Err(GroupError::Rollback {
            known: current.issued_at,
            offered: membership.issued_at,
        });
    }
    if membership.issued_at == current.issued_at {
        return if membership.canonical_bytes() == current.canonical_bytes() {
            Ok(Accepted::Unchanged)
        } else {
            Err(GroupError::Conflict {
                issued_at: membership.issued_at,
            })
        };
    }
    Ok(Accepted::Replace)
}

/// Whether an app may use a group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Access {
    /// The newest known list from the app's publisher names it.
    Member,
    /// A list is known and does not name this app -- never added, or removed.
    NotNamed,
    /// No list for this group from this publisher is known on this machine.
    NoList,
    /// The app is not signed by a publisher, so no list can name it.
    Unsigned,
}

impl Access {
    pub fn allowed(&self) -> bool {
        matches!(self, Access::Member)
    }
}

/// Decide whether `app_id`, signed by `publisher_root` (None when unsigned),
/// may use `group`, given the newest list known for that publisher's group.
///
/// `known` must be the list the caller stored for exactly
/// `(publisher_root, group)`; it is re-verified here rather than trusted,
/// so a store file edited on disk cannot grant membership.
pub fn access(
    publisher_root: Option<&str>,
    app_id: &str,
    group: &str,
    known: Option<&SignedGroupMembership>,
) -> Access {
    let Some(root) = publisher_root else {
        return Access::Unsigned;
    };
    let Some(known) = known else {
        return Access::NoList;
    };
    match known.verify() {
        Ok(list) if list.root == root && list.group == group => {
            if list.names(app_id) {
                Access::Member
            } else {
                Access::NotNamed
            }
        }
        // A stored list that does not verify, or belongs to another group
        // or publisher, is no list at all -- never a grant.
        _ => Access::NoList,
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> SigningKey {
        let doc = SigningKey::generate_pkcs8().expect("generate");
        SigningKey::from_pkcs8(&doc).expect("load")
    }

    fn list(root: &SigningKey, group: &str, at: u64, members: &[&str]) -> SignedGroupMembership {
        SignedGroupMembership::create(
            root,
            GroupMembership {
                schema: String::new(),
                root: String::new(),
                group: group.to_string(),
                issued_at: at,
                members: members.iter().map(|m| m.to_string()).collect(),
            },
        )
    }

    /// Test 1518, first half: a group grants only the apps its publisher
    /// named -- not a sibling app of the same publisher, not a stranger's
    /// app with a matching id, not an unsigned file.
    #[test]
    fn a_group_grants_only_the_apps_its_publisher_named() {
        let acme = key();
        let other = key();
        let root = hex(&acme.public_key());
        let members = list(
            &acme,
            "family-budget",
            100,
            &["com.acme.budget", "com.acme.reports"],
        );

        let ask =
            |root: Option<&str>, app: &str| access(root, app, "family-budget", Some(&members));
        assert_eq!(ask(Some(&root), "com.acme.budget"), Access::Member);
        assert_eq!(ask(Some(&root), "com.acme.reports"), Access::Member);
        assert_eq!(
            ask(Some(&root), "com.acme.game"),
            Access::NotNamed,
            "the same publisher's app is NOT a member unless it is named"
        );
        assert_eq!(
            ask(Some(&hex(&other.public_key())), "com.acme.budget"),
            Access::NoList,
            "another publisher's app with the same id is not a member of acme's group"
        );
        assert_eq!(ask(None, "com.acme.budget"), Access::Unsigned);
        assert_eq!(
            access(
                Some(&root),
                "com.acme.budget",
                "holiday-fund",
                Some(&members)
            ),
            Access::NoList,
            "a list for one group grants nothing in another"
        );
        assert_eq!(
            access(Some(&root), "com.acme.budget", "family-budget", None),
            Access::NoList
        );
    }

    /// Test 1518, second half: revocation. A newer list without the app
    /// removes it, and the older list turning up again -- inside an old
    /// bundle -- cannot put it back.
    #[test]
    fn removing_an_app_sticks_even_when_the_old_list_returns() {
        let acme = key();
        let root = hex(&acme.public_key());
        let first = list(
            &acme,
            "family-budget",
            100,
            &["com.acme.budget", "com.acme.reports"],
        );
        let later = list(&acme, "family-budget", 200, &["com.acme.budget"]);

        assert_eq!(
            accept(None, &first, &root, "family-budget"),
            Ok(Accepted::Replace)
        );
        assert_eq!(
            accept(Some(&first), &later, &root, "family-budget"),
            Ok(Accepted::Replace)
        );
        assert_eq!(
            access(
                Some(&root),
                "com.acme.reports",
                "family-budget",
                Some(&later)
            ),
            Access::NotNamed,
            "the removed app is out"
        );
        assert_eq!(
            accept(Some(&later), &first, &root, "family-budget"),
            Err(GroupError::Rollback {
                known: 200,
                offered: 100
            }),
            "the old list cannot undo the removal"
        );
        assert_eq!(
            accept(Some(&later), &later, &root, "family-budget"),
            Ok(Accepted::Unchanged)
        );
    }

    /// Unauthorized membership: a list nobody but the root could have
    /// written is the only kind that counts.
    #[test]
    fn a_list_the_publisher_did_not_sign_grants_nothing() {
        let acme = key();
        let intruder = key();
        let root = hex(&acme.public_key());

        // Signed by somebody else: refused on import, no list for access.
        let theirs = list(&intruder, "family-budget", 300, &["com.intruder.spy"]);
        assert_eq!(
            accept(None, &theirs, &root, "family-budget"),
            Err(GroupError::WrongPublisher)
        );
        assert_eq!(
            access(
                Some(&root),
                "com.intruder.spy",
                "family-budget",
                Some(&theirs)
            ),
            Access::NoList
        );

        // The publisher's own list, with a member added afterwards: the
        // signature no longer covers it.
        let mut edited = list(&acme, "family-budget", 100, &["com.acme.budget"]);
        edited
            .membership
            .members
            .push("com.intruder.spy".to_string());
        assert_eq!(
            accept(None, &edited, &root, "family-budget"),
            Err(GroupError::Forged)
        );
        assert_eq!(
            access(
                Some(&root),
                "com.intruder.spy",
                "family-budget",
                Some(&edited)
            ),
            Access::NoList,
            "a stored list edited on disk is re-verified and grants nothing"
        );

        // Renamed to another group after signing.
        let mut moved = list(&acme, "family-budget", 100, &["com.acme.budget"]);
        moved.membership.group = "holiday-fund".to_string();
        assert_eq!(
            accept(None, &moved, &root, "holiday-fund"),
            Err(GroupError::Forged)
        );

        // A list for another group offered as this one.
        let holiday = list(&acme, "holiday-fund", 100, &["com.acme.budget"]);
        assert!(matches!(
            accept(None, &holiday, &root, "family-budget"),
            Err(GroupError::WrongGroup { .. })
        ));
    }

    /// Two different lists claiming the same second cannot both be the
    /// newest, and neither is taken.
    #[test]
    fn two_lists_at_the_same_moment_are_a_conflict_not_a_choice() {
        let acme = key();
        let root = hex(&acme.public_key());
        let a = list(&acme, "family-budget", 100, &["com.acme.budget"]);
        let b = list(
            &acme,
            "family-budget",
            100,
            &["com.acme.budget", "com.acme.game"],
        );
        assert_eq!(
            accept(Some(&a), &b, &root, "family-budget"),
            Err(GroupError::Conflict { issued_at: 100 })
        );
        // The same members in another order are the same statement.
        let reordered = list(&acme, "family-budget", 100, &["com.acme.budget"]);
        assert_eq!(
            accept(Some(&a), &reordered, &root, "family-budget"),
            Ok(Accepted::Unchanged)
        );
    }

    /// The signed bytes are not another record's bytes.
    #[test]
    fn a_membership_list_cannot_be_read_as_another_signed_record() {
        let acme = key();
        let signed = list(&acme, "family-budget", 100, &["com.acme.budget"]);
        let bytes = signed.membership.canonical_bytes();
        assert_eq!(
            &bytes[8..8 + GROUP_MEMBERSHIP_SCHEMA.len()],
            GROUP_MEMBERSHIP_SCHEMA.as_bytes()
        );
        assert_ne!(
            GROUP_MEMBERSHIP_SCHEMA,
            crate::delegation::REVOCATION_LIST_SCHEMA
        );
    }
}
