//! Phase 2 UCap session policy.
//!
//! This crate decides whether a capability requested by an app is available in
//! the current run session. It is intentionally session-scoped. Persistent
//! grants and revocation are later-phase work.

use std::{collections::BTreeSet, str::FromStr};

use krate_adapter_common::path::LogicalPath;
use krate_manifest::{default_granted_capabilities, Capability, Manifest, ManifestError};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPolicy {
    grants: BTreeSet<Capability>,
}

impl SessionPolicy {
    pub fn from_grants(grants: impl IntoIterator<Item = Capability>) -> Self {
        let mut resolved = default_granted_capabilities();
        resolved.extend(grants);
        Self { grants: resolved }
    }

    pub fn allow_all_declared(manifest: &Manifest) -> Result<Self> {
        Ok(Self::from_grants(manifest.declared_capabilities()?))
    }

    pub fn from_cli_grants(grants: &[String]) -> Result<Self> {
        let parsed = grants
            .iter()
            .map(|grant| Capability::from_str(grant))
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(Self::from_grants(parsed))
    }

    pub fn grants(&self) -> &BTreeSet<Capability> {
        &self.grants
    }

    pub fn check(&self, required: &Capability) -> Result<()> {
        if self.allows(required) {
            Ok(())
        } else {
            Err(PolicyError::Denied {
                cap: required.to_string(),
            })
        }
    }

    pub fn allows(&self, required: &Capability) -> bool {
        self.grants
            .iter()
            .any(|grant| capability_allows(grant, required))
    }

    pub fn missing_required_for_manifest(&self, manifest: &Manifest) -> Result<Vec<Capability>> {
        Ok(manifest
            .required_capabilities()?
            .into_iter()
            .filter(|cap| !self.allows(cap))
            .collect())
    }
}

impl Default for SessionPolicy {
    fn default() -> Self {
        Self::from_grants([])
    }
}

pub fn resolve_session_policy(
    manifest: Option<&Manifest>,
    cli_grants: &[String],
    auto_grant: bool,
) -> Result<SessionPolicy> {
    match (manifest, auto_grant) {
        (Some(manifest), true) => SessionPolicy::allow_all_declared(manifest),
        _ => SessionPolicy::from_cli_grants(cli_grants),
    }
}

fn capability_allows(grant: &Capability, required: &Capability) -> bool {
    if grant.module() != required.module() || grant.action() != required.action() {
        return false;
    }

    match (grant.resource(), required.resource()) {
        (None, None) => true,
        (Some(grant_resource), Some(required_resource)) => {
            let Some(grant_resource) = normalize_resource(grant.module(), grant_resource) else {
                return false;
            };
            let Some(required_resource) = normalize_resource(required.module(), required_resource)
            else {
                return false;
            };
            if grant.module() == "net" {
                return net_resource_pattern_matches(&grant_resource, &required_resource);
            }
            // `fs.list:images/**` must cover listing `images` itself. The glob
            // matches paths *inside* the folder, so the one operation the
            // person obviously meant to allow -- reading the folder's contents
            // -- was refused, and an image viewer reported an empty library
            // with a granted folder full of pictures. Listing is the only
            // action widened this way: `fs.remove:images/**` covering removal
            // of the folder itself would delete more than was granted.
            if grant.module() == "fs" && grant.action() == "list" {
                if let Some(prefix) = grant_resource.strip_suffix("/**") {
                    if required_resource == prefix {
                        return true;
                    }
                }
            }
            resource_pattern_matches(&grant_resource, &required_resource)
        }
        _ => false,
    }
}

fn normalize_resource(module: &str, resource: &str) -> Option<String> {
    if module == "fs" {
        return LogicalPath::parse(resource)
            .ok()
            .map(|path| path.as_str().to_string());
    }
    if module == "net" {
        return normalize_net_resource(resource);
    }
    Some(resource.to_string())
}

fn normalize_net_resource(resource: &str) -> Option<String> {
    let (host, port) = resource.split_once(':')?;
    if host.is_empty() || port.is_empty() {
        return None;
    }
    let host = host.to_ascii_lowercase();
    if port == "*" {
        return Some(format!("{host}:*"));
    }

    let port = port.parse::<u16>().ok()?;
    if port == 0 {
        return None;
    }

    Some(format!("{host}:{port}"))
}

fn net_resource_pattern_matches(pattern: &str, value: &str) -> bool {
    let Some((pattern_host, pattern_port)) = split_net_resource(pattern) else {
        return false;
    };
    let Some((value_host, value_port)) = split_net_resource(value) else {
        return false;
    };

    if pattern_port != "*" && pattern_port != value_port {
        return false;
    }

    if pattern_host == "*" {
        return true;
    }

    if let Some(suffix) = pattern_host.strip_prefix("*.") {
        if value_host == suffix {
            return false;
        }
        let Some(prefix) = value_host.strip_suffix(suffix) else {
            return false;
        };
        let Some(prefix) = prefix.strip_suffix('.') else {
            return false;
        };
        return !prefix.is_empty() && !prefix.contains('.');
    }

    pattern_host == value_host
}

fn split_net_resource(resource: &str) -> Option<(&str, &str)> {
    let (host, port) = resource.split_once(':')?;
    if host.is_empty() || port.is_empty() {
        return None;
    }
    Some((host, port))
}

fn resource_pattern_matches(pattern: &str, value: &str) -> bool {
    wildcard_match(
        &pattern.chars().collect::<Vec<_>>(),
        &value.chars().collect::<Vec<_>>(),
    )
}

fn wildcard_match(pattern: &[char], value: &[char]) -> bool {
    let mut memo = BTreeSet::new();
    wildcard_match_from(pattern, value, 0, 0, &mut memo)
}

fn wildcard_match_from(
    pattern: &[char],
    value: &[char],
    p: usize,
    v: usize,
    failed: &mut BTreeSet<(usize, usize)>,
) -> bool {
    if failed.contains(&(p, v)) {
        return false;
    }

    let matched = if p == pattern.len() {
        v == value.len()
    } else if pattern[p] == '*' {
        let is_double_star = p + 1 < pattern.len() && pattern[p + 1] == '*';
        let next_p = if is_double_star { p + 2 } else { p + 1 };

        wildcard_match_from(pattern, value, next_p, v, failed)
            || (v < value.len()
                && (is_double_star || value[v] != '/')
                && wildcard_match_from(pattern, value, p, v + 1, failed))
    } else {
        v < value.len()
            && pattern[p] == value[v]
            && wildcard_match_from(pattern, value, p + 1, v + 1, failed)
    };

    if !matched {
        failed.insert((p, v));
    }

    matched
}

#[derive(Debug, Error)]
pub enum PolicyError {
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error("capability `{cap}` was not granted")]
    Denied { cap: String },
}

pub type Result<T> = std::result::Result<T, PolicyError>;

#[cfg(test)]
mod tests {
    use super::*;

    /// K-086: the default-granted dialog wildcard silently covered the
    /// privileged dialogs, so promoting file-open/file-save to explicit
    /// asks changed the wall and changed nothing at the policy layer. The
    /// defaults now grant exactly the harmless pair; everything that moves
    /// data must be declared and shown to a person.
    #[test]
    fn default_dialog_grants_cover_message_boxes_and_nothing_privileged() {
        let policy = SessionPolicy::from_grants(vec![]);
        for req in ["ui.dialog:message", "ui.dialog:confirm"] {
            let cap: Capability = req.parse().expect("cap");
            assert!(policy.allows(&cap), "default must allow {req}");
        }
        for req in [
            "ui.dialog:open-folder",
            "ui.dialog:file-open",
            "ui.dialog:file-save",
        ] {
            let cap: Capability = req.parse().expect("cap");
            assert!(!policy.allows(&cap), "default must NOT allow {req}");
        }
        // A DECLARED wildcard is an explicit ask for all dialogs and does
        // cover them -- the person saw it on the wall.
        let starred = SessionPolicy::from_grants(vec!["ui.dialog:*".parse().expect("cap")]);
        let folder: Capability = "ui.dialog:open-folder".parse().expect("cap");
        assert!(
            starred.allows(&folder),
            "a declared wildcard covers the dialogs"
        );
    }

    #[test]
    fn a_list_glob_covers_the_folder_itself_and_a_remove_glob_does_not() {
        // `fs.list:images/**` grants "see what is in images". Listing the
        // folder is that exact operation, and refusing it made an image viewer
        // report an empty library with a granted folder full of pictures. But
        // `fs.remove:images/**` grants deleting things *inside* the folder --
        // widening it to the folder itself would delete more than was granted.
        let list_grant = Capability::from_str("fs.list:images/**").expect("grant");
        let list_folder = Capability::from_str("fs.list:images").expect("required");
        assert!(capability_allows(&list_grant, &list_folder));

        let list_inside = Capability::from_str("fs.list:images/holiday").expect("required");
        assert!(capability_allows(&list_grant, &list_inside));

        let remove_grant = Capability::from_str("fs.remove:images/**").expect("grant");
        let remove_folder = Capability::from_str("fs.remove:images").expect("required");
        assert!(
            !capability_allows(&remove_grant, &remove_folder),
            "a remove glob must not extend to the folder itself"
        );

        // And an unrelated folder stays refused.
        let other = Capability::from_str("fs.list:secrets").expect("required");
        assert!(!capability_allows(&list_grant, &other));
    }

    const MANIFEST: &str = r#"
        [app]
        id = "com.example.notes"
        name = "Notes"
        version = "1.0.0"
        entry = "notes.wasm"
        world = "krate:app/cli@0.1.0"

        [[capabilities]]
        cap = "io.stdout"
        rationale = "Print output"
        required = true

        [[capabilities]]
        cap = "fs.read:./notes/**"
        rationale = "Read notes"
        required = true

        [[capabilities]]
        cap = "net.connect:api.example.com:443"
        rationale = "Sync notes"
        required = false
    "#;

    #[test]
    fn default_policy_allows_default_grants() {
        let policy = SessionPolicy::default();
        let stdout = "io.stdout".parse().expect("parse capability");
        let ui_window = "ui.window:create".parse().expect("parse capability");
        let gfx_basic = "gfx.gpu:basic".parse().expect("parse capability");
        let fs_read = "fs.read:./notes/today.txt"
            .parse()
            .expect("parse capability");

        assert!(policy.allows(&stdout));
        assert!(policy.allows(&ui_window));
        assert!(policy.allows(&gfx_basic));
        assert!(!policy.allows(&fs_read));
    }

    #[test]
    fn phase3_sensitive_caps_are_not_default_granted() {
        let policy = SessionPolicy::default();
        let clipboard_read = "ui.clipboard:read".parse().expect("parse capability");
        let audio_capture = "audio.capture".parse().expect("parse capability");

        assert!(!policy.allows(&clipboard_read));
        assert!(!policy.allows(&audio_capture));
    }

    #[test]
    fn explicit_grant_allows_matching_resource() {
        let grant = "fs.read:./notes/**".parse().expect("parse grant");
        let policy = SessionPolicy::from_grants([grant]);
        let required = "fs.read:./notes/today.txt".parse().expect("parse required");

        assert!(policy.allows(&required));
    }

    #[test]
    fn fs_resource_matching_uses_shared_path_normalization() {
        let grant = "fs.read:./notes/**".parse().expect("parse grant");
        let policy = SessionPolicy::from_grants([grant]);
        let required = "fs.read:notes\\today.txt".parse().expect("parse required");

        assert!(policy.allows(&required));
    }

    #[test]
    fn fs_resource_matching_rejects_parent_traversal() {
        let grant = "fs.read:./notes/**".parse().expect("parse grant");
        let policy = SessionPolicy::from_grants([grant]);
        let err = "fs.read:./notes/../secret.txt"
            .parse::<Capability>()
            .expect_err("parent traversal should fail during capability parsing");

        assert!(
            matches!(err, ManifestError::InvalidCapability { .. }),
            "unexpected parse error: {err:?}"
        );
        let required = "fs.read:./notes/today.txt".parse().expect("parse required");
        assert!(policy.allows(&required));
    }

    #[test]
    fn explicit_grant_does_not_cross_actions() {
        let grant = "fs.read:./notes/**".parse().expect("parse grant");
        let policy = SessionPolicy::from_grants([grant]);
        let required = "fs.write:./notes/today.txt"
            .parse()
            .expect("parse required");

        assert!(!policy.allows(&required));
    }

    #[test]
    fn auto_grant_allows_required_manifest_caps() {
        let manifest = Manifest::parse(MANIFEST).expect("parse manifest");
        let policy = resolve_session_policy(Some(&manifest), &[], true).expect("policy");
        let missing = policy
            .missing_required_for_manifest(&manifest)
            .expect("missing caps");

        assert!(missing.is_empty());
    }

    #[test]
    fn reports_missing_required_manifest_caps() {
        let manifest = Manifest::parse(MANIFEST).expect("parse manifest");
        let policy = SessionPolicy::default();
        let missing = policy
            .missing_required_for_manifest(&manifest)
            .expect("missing caps");

        assert_eq!(
            missing.iter().map(ToString::to_string).collect::<Vec<_>>(),
            ["fs.read:notes/**"]
        );
    }

    #[test]
    fn wildcard_supports_middle_and_suffix_matches() {
        assert!(resource_pattern_matches("./notes/**", "./notes/a/b.txt"));
        assert!(!resource_pattern_matches(
            "./notes/*.txt",
            "./notes/a/b.txt"
        ));
    }

    #[test]
    fn net_resource_matching_normalizes_host_case() {
        let grant = "net.connect:API.Example.com:443"
            .parse()
            .expect("parse grant");
        let policy = SessionPolicy::from_grants([grant]);
        let required = "net.connect:api.example.com:443"
            .parse()
            .expect("parse required");

        assert!(policy.allows(&required));
    }

    #[test]
    fn net_resource_matching_normalizes_numeric_ports() {
        let grant = "net.connect:api.example.com:0443"
            .parse()
            .expect("parse grant");
        let policy = SessionPolicy::from_grants([grant]);
        let required = "net.connect:api.example.com:443"
            .parse()
            .expect("parse required");

        assert!(policy.allows(&required));
    }

    #[test]
    fn net_resource_matching_leftmost_wildcard_is_single_label_only() {
        let grant = "net.connect:*.example.com:443"
            .parse()
            .expect("parse grant");
        let policy = SessionPolicy::from_grants([grant]);
        let one_label = "net.connect:api.example.com:443"
            .parse()
            .expect("parse required");
        let two_labels = "net.connect:deep.api.example.com:443"
            .parse()
            .expect("parse required");
        let apex = "net.connect:example.com:443"
            .parse()
            .expect("parse required");

        assert!(policy.allows(&one_label));
        assert!(!policy.allows(&two_labels));
        assert!(!policy.allows(&apex));
    }

    #[test]
    fn net_resource_matching_global_wildcard_matches_any_host() {
        let grant = "net.connect:*:443".parse().expect("parse grant");
        let policy = SessionPolicy::from_grants([grant]);
        let required = "net.connect:api.example.com:443"
            .parse()
            .expect("parse required");

        assert!(policy.allows(&required));
    }
}
