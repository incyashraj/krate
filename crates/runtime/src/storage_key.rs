//! The name a signed app's data goes by on disk (IC-736, K-1172).
//!
//! An app's id is a string its author chose, so data keyed on it alone is
//! reachable by any other app that declares the same id. A signed app's data
//! is keyed on its id AND its verified publisher (the signing root), which
//! an impostor cannot supply. One rule, here, so a computer and a phone name
//! the same app's data the same way: the desktop's `StoragePrincipal` and
//! the Android and iPhone players both call [`verified_storage_key`].

/// Reduce a name to something safe to put in a path: letters, digits, `.`,
/// `-` and `_` survive, everything else (a separator, `@`, `+`) becomes `_`,
/// a run of dots cannot become a traversal, and an empty or all-dots name
/// still lands somewhere deterministic.
pub fn sanitize_storage_name(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let safe = safe.replace("..", "__");
    if safe.trim_matches(['.', '_'].as_slice()).is_empty() {
        "unnamed-app".to_string()
    } else {
        safe
    }
}

/// The directory-safe name a signed app's data goes by:
/// `<sanitized app id>@<first 16 characters of the publisher root>`.
///
/// The `@` never survives [`sanitize_storage_name`], so no app id -- signed
/// or not -- can name its way into another's publisher-keyed store. Sixteen
/// hex characters separate publishers without an unreadable 64-character
/// filename; a collision needs a deliberate 16-character preimage, and its
/// consequence would be two of ONE publisher's own apps sharing a store,
/// not a stranger reaching in.
///
/// The publisher is a verified signing key (hex), so sanitizing its prefix
/// changes nothing for a real one; it is there so that no string, however
/// it got here, can put a path separator into a file name.
pub fn verified_storage_key(app_id: &str, publisher: &str) -> String {
    let prefix = publisher.get(..16).unwrap_or(publisher);
    let prefix: String = prefix
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("{}@{prefix}", sanitize_storage_name(app_id))
}

/// Is `tag` the owner tag a Krate hub sends with an app (`x-krate-store`,
/// K-1159): `k` and sixteen lowercase hex characters, nothing else. Anything
/// else is not a tag and keys nothing, so a header can never put a path
/// separator, a dot or an `@` into a file name.
pub fn is_hub_owner_tag(tag: &str) -> bool {
    tag.len() == 17
        && tag.starts_with('k')
        && tag[1..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The directory-safe name an UNSIGNED app's data goes by when a Krate hub
/// vouched for who published it (K-1172): `<sanitized app id>~<tag>`, the
/// same `${id}~${tag}` the browser Player keys a tab's storage by (K-1159).
///
/// The tag is the hub's digest of the account that published the app, so
/// two accounts that both publish an unsigned `notes` reach two stores,
/// and an app opened from a file -- no hub, no tag -- keeps the id-keyed
/// name it always had. `None` when `tag` is not a hub tag.
///
/// The `~` never survives [`sanitize_storage_name`], so no app id can name
/// its way into a hub-keyed store, and it differs from a signed app's `@`
/// and a development identity's `+`.
pub fn hub_owner_storage_key(app_id: &str, tag: &str) -> Option<String> {
    is_hub_owner_tag(tag).then(|| format!("{}~{tag}", sanitize_storage_name(app_id)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signed_apps_key_is_its_id_and_its_publisher() {
        let publisher = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert_eq!(
            verified_storage_key("dev.krate.notes", publisher),
            "dev.krate.notes@0123456789abcdef"
        );
        assert_ne!(
            verified_storage_key("dev.krate.notes", publisher),
            verified_storage_key("dev.krate.notes", &"f".repeat(64)),
            "two publishers shipping one id never share a store"
        );
        assert_eq!(verified_storage_key("a", "abc"), "a@abc", "short key kept");
    }

    #[test]
    fn no_id_can_name_its_way_into_a_publishers_store() {
        let key = verified_storage_key("dev.krate.notes", &"a".repeat(64));
        assert_eq!(sanitize_storage_name(&key), key.replace('@', "_"));
        assert!(!sanitize_storage_name("x@aaaaaaaaaaaaaaaa").contains('@'));
        assert!(!verified_storage_key("../../etc", "../..//x").contains(['/', '\\']));
        assert_eq!(sanitize_storage_name(".."), "unnamed-app");
    }

    #[test]
    fn a_hub_owned_apps_key_is_its_id_and_the_hubs_tag() {
        assert_eq!(
            hub_owner_storage_key("dev.krate.notes", "k0123456789abcdef").as_deref(),
            Some("dev.krate.notes~k0123456789abcdef")
        );
        assert_ne!(
            hub_owner_storage_key("dev.krate.notes", "k0123456789abcdef"),
            hub_owner_storage_key("dev.krate.notes", "kfedcba9876543210"),
            "two hub accounts publishing one unsigned id never share a store"
        );
        for not_a_tag in [
            "",
            "k0123456789abcde",
            "k0123456789abcdef0",
            "K0123456789abcdef",
            "k0123456789ABCDEF",
            "k../../etc/passwd",
            "x0123456789abcdef",
        ] {
            assert_eq!(hub_owner_storage_key("a", not_a_tag), None, "{not_a_tag:?}");
        }
        assert!(!sanitize_storage_name("a~k0123456789abcdef").contains('~'));
    }
}
