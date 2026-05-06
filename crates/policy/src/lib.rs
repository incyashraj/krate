//! Phase 2 UCap session policy.
//!
//! This crate decides whether a capability requested by an app is available in
//! the current run session. It is intentionally session-scoped. Persistent
//! grants and revocation are later-phase work.

use std::{collections::BTreeSet, str::FromStr};

use layer36_adapter_common::path::LogicalPath;
use layer36_manifest::{default_granted_capabilities, Capability, Manifest, ManifestError};
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

    const MANIFEST: &str = r#"
        [app]
        id = "com.example.notes"
        name = "Notes"
        version = "1.0.0"
        entry = "notes.wasm"
        world = "layer36:app/cli@0.1.0"

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
        let fs_read = "fs.read:./notes/today.txt"
            .parse()
            .expect("parse capability");

        assert!(policy.allows(&stdout));
        assert!(!policy.allows(&fs_read));
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
