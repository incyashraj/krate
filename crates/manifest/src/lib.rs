//! Phase 2 sidecar manifest parser.
//!
//! A Layer36 Phase 2 app is still a plain `.wasm` component, but it may sit
//! next to a `manifest.toml` that declares identity, entry world, and requested
//! capabilities.

use std::{
    collections::BTreeSet,
    fmt,
    path::{Path, PathBuf},
    str::FromStr,
};

use layer36_adapter_common::path::LogicalPath;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const PHASE2_CLI_WORLD: &str = "layer36:app/cli@0.1.0";

const PHASE2_CAPABILITY_SPECS: &[CapabilitySpec] = &[
    CapabilitySpec::resource_free("io", "stdin", true),
    CapabilitySpec::resource_free("io", "stdout", true),
    CapabilitySpec::resource_free("io", "stderr", true),
    CapabilitySpec::resource_free("io", "args", true),
    CapabilitySpec::resource_free("io", "log", true),
    CapabilitySpec::resource_scoped("fs", "read", "<path-glob>"),
    CapabilitySpec::resource_scoped("fs", "write", "<path-glob>"),
    CapabilitySpec::resource_scoped("fs", "list", "<path-glob>"),
    CapabilitySpec::resource_scoped("fs", "remove", "<path-glob>"),
    CapabilitySpec::resource_scoped("fs", "mkdir", "<path-glob>"),
    CapabilitySpec::resource_scoped("net", "connect", "<host>:<port>"),
    CapabilitySpec::resource_free("time", "clock", true),
    CapabilitySpec::resource_free("time", "monotonic", true),
    CapabilitySpec::resource_free("time", "sleep", true),
    CapabilitySpec::resource_free("locale", "info", true),
    CapabilitySpec::resource_free("locale", "format", true),
];

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub app: App,
    #[serde(default)]
    pub capabilities: Vec<CapabilityRequest>,
}

impl Manifest {
    pub fn parse(input: &str) -> Result<Self> {
        let manifest: Self =
            toml::from_str(input).map_err(|err| ManifestError::Toml(err.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn parse_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let input = std::fs::read_to_string(path).map_err(|source| ManifestError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(&input)
    }

    pub fn declared_capabilities(&self) -> Result<Vec<Capability>> {
        self.capabilities
            .iter()
            .map(|request| request.cap.parse())
            .collect()
    }

    pub fn required_capabilities(&self) -> Result<Vec<Capability>> {
        self.capabilities
            .iter()
            .filter(|request| request.required)
            .map(|request| request.cap.parse())
            .collect()
    }

    pub fn to_toml_pretty(&self) -> Result<String> {
        self.validate()?;
        toml::to_string_pretty(self).map_err(|err| ManifestError::Toml(err.to_string()))
    }

    fn validate(&self) -> Result<()> {
        validate_app_id(&self.app.id)?;
        validate_required("app.name", &self.app.name)?;
        validate_required("app.version", &self.app.version)?;
        validate_required_path("app.entry", &self.app.entry)?;

        if self.app.world != PHASE2_CLI_WORLD {
            return Err(ManifestError::UnsupportedWorld {
                world: self.app.world.clone(),
            });
        }

        let mut seen = BTreeSet::new();
        for request in &self.capabilities {
            let cap: Capability = request.cap.parse()?;
            validate_required("capability.rationale", &request.rationale)?;
            if !seen.insert(cap.to_string()) {
                return Err(ManifestError::DuplicateCapability {
                    cap: request.cap.clone(),
                });
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct App {
    pub id: String,
    pub name: String,
    pub version: String,
    pub entry: PathBuf,
    pub world: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityRequest {
    pub cap: String,
    pub rationale: String,
    pub required: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilitySpec {
    module: &'static str,
    action: &'static str,
    resource: Option<&'static str>,
    default_granted: bool,
}

impl CapabilitySpec {
    const fn resource_free(
        module: &'static str,
        action: &'static str,
        default_granted: bool,
    ) -> Self {
        Self {
            module,
            action,
            resource: None,
            default_granted,
        }
    }

    const fn resource_scoped(
        module: &'static str,
        action: &'static str,
        resource: &'static str,
    ) -> Self {
        Self {
            module,
            action,
            resource: Some(resource),
            default_granted: false,
        }
    }

    pub fn module(&self) -> &'static str {
        self.module
    }

    pub fn action(&self) -> &'static str {
        self.action
    }

    pub fn resource(&self) -> Option<&'static str> {
        self.resource
    }

    pub fn name(&self) -> String {
        format!("{}.{}", self.module, self.action)
    }

    pub fn display_pattern(&self) -> String {
        match self.resource {
            Some(resource) => format!("{}:{resource}", self.name()),
            None => self.name(),
        }
    }

    pub fn default_granted(&self) -> bool {
        self.default_granted
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Capability {
    module: String,
    action: String,
    resource: Option<String>,
}

impl Capability {
    pub fn new(module: &str, action: &str, resource: Option<&str>) -> Result<Self> {
        validate_ident("capability module", module)?;
        validate_ident("capability action", action)?;

        let cap_name = format!("{module}.{action}");
        let resource_required = capability_resource_required(module, action).ok_or_else(|| {
            ManifestError::InvalidCapability {
                cap: cap_name.clone(),
                reason: "unknown Phase 2 capability".to_string(),
            }
        })?;
        let resource_was_present = resource.is_some();
        let resource = resource
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);

        match (resource_required, resource.as_ref(), resource_was_present) {
            (true, None, _) => {
                return Err(ManifestError::InvalidCapability {
                    cap: cap_name,
                    reason: "this capability requires a resource after `:`".to_string(),
                });
            }
            (false, Some(_), _) | (false, None, true) => {
                return Err(ManifestError::InvalidCapability {
                    cap: cap_name,
                    reason: "this capability does not take a resource".to_string(),
                });
            }
            _ => {}
        }

        if let Some(resource) = resource.as_deref() {
            validate_capability_resource(module, action, resource).map_err(|reason| {
                ManifestError::InvalidCapability {
                    cap: format!("{cap_name}:{resource}"),
                    reason,
                }
            })?;
        }

        Ok(Self {
            module: module.to_owned(),
            action: action.to_owned(),
            resource,
        })
    }

    pub fn module(&self) -> &str {
        &self.module
    }

    pub fn action(&self) -> &str {
        &self.action
    }

    pub fn resource(&self) -> Option<&str> {
        self.resource.as_deref()
    }

    pub fn is_default_granted(&self) -> bool {
        default_granted_capabilities().contains(self)
    }
}

impl FromStr for Capability {
    type Err = ManifestError;

    fn from_str(input: &str) -> Result<Self> {
        let input = input.trim();
        let (module, rest) =
            input
                .split_once('.')
                .ok_or_else(|| ManifestError::InvalidCapability {
                    cap: input.to_owned(),
                    reason: "expected <module>.<action>[:resource]".to_string(),
                })?;
        let (action, resource) = match rest.split_once(':') {
            Some((action, resource)) => (action, Some(resource)),
            None => (rest, None),
        };

        Self::new(module, action, resource).map_err(|err| match err {
            ManifestError::InvalidIdentifier { field, reason } => {
                ManifestError::InvalidCapability {
                    cap: input.to_owned(),
                    reason: format!("{field}: {reason}"),
                }
            }
            other => other,
        })
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.module, self.action)?;
        if let Some(resource) = &self.resource {
            write!(f, ":{resource}")?;
        }
        Ok(())
    }
}

pub fn default_granted_capabilities() -> BTreeSet<Capability> {
    PHASE2_CAPABILITY_SPECS
        .iter()
        .filter(|spec| spec.default_granted())
        .map(|spec| {
            Capability::new(spec.module(), spec.action(), None)
                .expect("default capability specs are valid")
        })
        .collect()
}

pub fn supported_capability_specs() -> &'static [CapabilitySpec] {
    PHASE2_CAPABILITY_SPECS
}

#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("failed to read manifest {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse manifest TOML: {0}")]
    Toml(String),
    #[error("missing required field `{0}`")]
    MissingField(&'static str),
    #[error("invalid app id `{id}`: {reason}")]
    InvalidAppId { id: String, reason: String },
    #[error("unsupported app world `{world}`")]
    UnsupportedWorld { world: String },
    #[error("invalid {field}: {reason}")]
    InvalidIdentifier { field: &'static str, reason: String },
    #[error("invalid capability `{cap}`: {reason}")]
    InvalidCapability { cap: String, reason: String },
    #[error("duplicate capability `{cap}`")]
    DuplicateCapability { cap: String },
}

pub type Result<T> = std::result::Result<T, ManifestError>;

fn validate_required(field: &'static str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        Err(ManifestError::MissingField(field))
    } else {
        Ok(())
    }
}

fn validate_required_path(field: &'static str, value: &Path) -> Result<()> {
    if value.as_os_str().is_empty() {
        Err(ManifestError::MissingField(field))
    } else {
        Ok(())
    }
}

fn validate_app_id(id: &str) -> Result<()> {
    validate_required("app.id", id)?;

    let parts = id.split('.').collect::<Vec<_>>();
    if parts.len() < 2 {
        return Err(ManifestError::InvalidAppId {
            id: id.to_owned(),
            reason: "use reverse-DNS form, for example com.example.app".to_string(),
        });
    }

    for part in parts {
        if part.is_empty()
            || !part
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
        {
            return Err(ManifestError::InvalidAppId {
                id: id.to_owned(),
                reason: "segments may only contain ASCII letters, numbers, hyphen, or underscore"
                    .to_string(),
            });
        }
    }

    Ok(())
}

fn validate_ident(field: &'static str, value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(ManifestError::InvalidIdentifier {
            field,
            reason: "value is empty".to_string(),
        });
    }

    if !value
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        return Err(ManifestError::InvalidIdentifier {
            field,
            reason: "use lowercase ASCII letters, numbers, or hyphen".to_string(),
        });
    }

    Ok(())
}

fn capability_resource_required(module: &str, action: &str) -> Option<bool> {
    supported_capability_specs()
        .iter()
        .find(|spec| spec.module == module && spec.action == action)
        .map(|spec| spec.resource.is_some())
}

fn validate_capability_resource(
    module: &str,
    action: &str,
    resource: &str,
) -> std::result::Result<(), String> {
    if module == "fs" {
        LogicalPath::parse(resource)
            .map(|_| ())
            .map_err(|err| format!("invalid filesystem resource pattern: {err}"))?;
        return Ok(());
    }

    if module == "net" && action == "connect" {
        validate_connect_resource(resource)?;
    }

    Ok(())
}

fn validate_connect_resource(resource: &str) -> std::result::Result<(), String> {
    if resource
        .chars()
        .any(|ch| ch.is_ascii_whitespace() || ch == '\0' || ch.is_control())
    {
        return Err("network endpoint cannot contain whitespace or control characters".to_string());
    }
    if resource.starts_with('[') || resource.contains("]:") {
        return Err("IPv6 bracketed endpoint forms are not supported in this phase".to_string());
    }
    if resource.matches(':').count() != 1 {
        return Err("expected <host>:<port> endpoint form".to_string());
    }

    let (host, port) = resource
        .split_once(':')
        .ok_or_else(|| "expected <host>:<port> endpoint form".to_string())?;
    if host.is_empty() {
        return Err("network endpoint host cannot be empty".to_string());
    }
    validate_connect_host_pattern(host)?;

    if port == "*" {
        return Ok(());
    }
    let value = port.parse::<u16>().map_err(|_| {
        "network endpoint port must be `*` or a numeric port in 1..65535".to_string()
    })?;
    if value == 0 {
        return Err("network endpoint port must be in 1..65535".to_string());
    }

    Ok(())
}

fn validate_connect_host_pattern(host: &str) -> std::result::Result<(), String> {
    if host.starts_with('.') || host.ends_with('.') || host.contains("..") {
        return Err("network endpoint host has invalid dot placement".to_string());
    }
    if !host
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '*'))
    {
        return Err("network endpoint host contains unsupported characters".to_string());
    }

    let labels = host.split('.').collect::<Vec<_>>();
    let wildcard_labels = labels.iter().filter(|label| **label == "*").count();

    if labels
        .iter()
        .any(|label| label.contains('*') && *label != "*")
    {
        return Err("network endpoint host wildcard must be a full `*` label".to_string());
    }
    if wildcard_labels > 1 {
        return Err("network endpoint host can contain at most one wildcard label".to_string());
    }
    if wildcard_labels == 1 && labels.first().copied() != Some("*") {
        return Err("network endpoint host wildcard must be the left-most label".to_string());
    }

    let numeric_like = host.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'.'));
    if numeric_like && !is_valid_ipv4_host(host) {
        return Err("network endpoint host is not a valid IPv4 address".to_string());
    }

    for label in labels {
        if label.is_empty() || label.len() > 63 {
            return Err("network endpoint host label is empty or too long".to_string());
        }
        if label == "*" {
            continue;
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err("network endpoint host label cannot start or end with `-`".to_string());
        }
    }

    Ok(())
}

fn is_valid_ipv4_host(host: &str) -> bool {
    let mut octets = [0u8; 4];
    let mut parts = host.split('.');
    for octet in &mut octets {
        let Some(part) = parts.next() else {
            return false;
        };
        if part.is_empty() {
            return false;
        }
        let Ok(value) = part.parse::<u8>() else {
            return false;
        };
        *octet = value;
    }
    if parts.next().is_some() {
        return false;
    }

    let ip = std::net::Ipv4Addr::new(octets[0], octets[1], octets[2], octets[3]);
    if ip.is_unspecified() || ip.is_multicast() || ip == std::net::Ipv4Addr::BROADCAST {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
        [app]
        id = "com.example.hello"
        name = "Hello"
        version = "1.0.0"
        entry = "hello.wasm"
        world = "layer36:app/cli@0.1.0"

        [[capabilities]]
        cap = "fs.read:~/Documents/notes/**"
        rationale = "Read saved notes"
        required = true

        [[capabilities]]
        cap = "net.connect:api.example.com:443"
        rationale = "Sync to cloud"
        required = false
    "#;

    #[test]
    fn parses_phase_2_manifest_schema() {
        let manifest = Manifest::parse(EXAMPLE).expect("parse manifest");

        assert_eq!(manifest.app.id, "com.example.hello");
        assert_eq!(manifest.app.entry, PathBuf::from("hello.wasm"));
        assert_eq!(manifest.capabilities.len(), 2);

        let caps = manifest
            .declared_capabilities()
            .expect("declared capabilities");
        assert_eq!(caps[0].module(), "fs");
        assert_eq!(caps[0].action(), "read");
        assert_eq!(caps[0].resource(), Some("~/Documents/notes/**"));
    }

    #[test]
    fn rejects_unsupported_worlds() {
        let input = EXAMPLE.replace(PHASE2_CLI_WORLD, "layer36:app/gui@0.1.0");
        let err = Manifest::parse(&input).expect_err("reject unsupported world");

        assert!(matches!(err, ManifestError::UnsupportedWorld { .. }));
    }

    #[test]
    fn rejects_duplicate_capabilities() {
        let input = format!(
            "{EXAMPLE}\n[[capabilities]]\ncap = \"fs.read:~/Documents/notes/**\"\nrationale = \"again\"\nrequired = true\n"
        );
        let err = Manifest::parse(&input).expect_err("reject duplicate capability");

        assert!(matches!(err, ManifestError::DuplicateCapability { .. }));
    }

    #[test]
    fn parses_capability_parts() {
        let cap: Capability = "net.connect:api.example.com:443"
            .parse()
            .expect("parse cap");

        assert_eq!(cap.module(), "net");
        assert_eq!(cap.action(), "connect");
        assert_eq!(cap.resource(), Some("api.example.com:443"));
        assert_eq!(cap.to_string(), "net.connect:api.example.com:443");
    }

    #[test]
    fn rejects_unknown_capability_names() {
        let err = "net.listen:127.0.0.1:8080"
            .parse::<Capability>()
            .expect_err("reject unknown cap");

        assert!(matches!(err, ManifestError::InvalidCapability { .. }));
    }

    #[test]
    fn rejects_missing_required_resource() {
        let err = "fs.read"
            .parse::<Capability>()
            .expect_err("reject missing resource");

        assert!(matches!(err, ManifestError::InvalidCapability { .. }));
    }

    #[test]
    fn rejects_resource_on_resource_free_capability() {
        let err = "io.stdout:terminal"
            .parse::<Capability>()
            .expect_err("reject extra resource");

        assert!(matches!(err, ManifestError::InvalidCapability { .. }));
    }

    #[test]
    fn validates_fs_resource_patterns_with_shared_path_rules() {
        let valid = "fs.read:./notes/**"
            .parse::<Capability>()
            .expect("valid fs path glob");
        assert_eq!(valid.resource(), Some("./notes/**"));

        let err = "fs.read:../secret.txt"
            .parse::<Capability>()
            .expect_err("reject parent traversal in fs resource");
        assert!(matches!(err, ManifestError::InvalidCapability { .. }));

        let err = "fs.read:C:/secret.txt"
            .parse::<Capability>()
            .expect_err("reject unsupported prefix in fs resource");
        assert!(matches!(err, ManifestError::InvalidCapability { .. }));
    }

    #[test]
    fn validates_net_connect_resource_shape() {
        "net.connect:127.0.0.1:443"
            .parse::<Capability>()
            .expect("accept host and numeric port");
        "net.connect:api-1.example.com:443"
            .parse::<Capability>()
            .expect("accept domain labels with hyphen");
        "net.connect:*.example.com:*"
            .parse::<Capability>()
            .expect("accept wildcard host and wildcard port");
        "net.connect:*:443"
            .parse::<Capability>()
            .expect("accept wildcard host");

        for input in [
            "net.connect::443",
            "net.connect:example.com",
            "net.connect:example.com:0",
            "net.connect:example.com:70000",
            "net.connect:example.com:not-a-port",
            "net.connect:exa mple.com:443",
            "net.connect:[::1]:80",
            "net.connect:one:two:three",
            "net.connect:.example.com:443",
            "net.connect:example.com.:443",
            "net.connect:example..com:443",
            "net.connect:-example.com:443",
            "net.connect:example-.com:443",
            "net.connect:256.1.1.1:443",
            "net.connect:1.2.3:443",
            "net.connect:1.2.3.4.5:443",
            "net.connect:0.0.0.0:443",
            "net.connect:255.255.255.255:443",
            "net.connect:239.1.2.3:443",
            "net.connect:api.*.example.com:443",
            "net.connect:*.*.example.com:443",
            "net.connect:exa*mple.com:443",
            "net.connect:api*.example.com:443",
        ] {
            let err = input
                .parse::<Capability>()
                .expect_err("invalid endpoint should be rejected");
            assert!(
                matches!(err, ManifestError::InvalidCapability { .. }),
                "unexpected error type for `{input}`: {err:?}"
            );
        }
    }

    #[test]
    fn tracks_default_grants() {
        let stdin: Capability = "io.stdin".parse().expect("parse stdin cap");
        let stdout: Capability = "io.stdout".parse().expect("parse stdout cap");
        let fs_read: Capability = "fs.read:./data/**".parse().expect("parse fs cap");

        assert!(stdin.is_default_granted());
        assert!(stdout.is_default_granted());
        assert!(!fs_read.is_default_granted());
    }

    #[test]
    fn exposes_canonical_phase_2_capability_specs() {
        let specs = supported_capability_specs();

        assert!(specs
            .iter()
            .any(|spec| spec.display_pattern() == "io.args" && spec.default_granted()));
        assert!(specs.iter().any(|spec| {
            spec.display_pattern() == "fs.read:<path-glob>" && !spec.default_granted()
        }));
        assert!(specs.iter().any(|spec| {
            spec.display_pattern() == "net.connect:<host>:<port>" && !spec.default_granted()
        }));
    }

    #[test]
    fn renders_manifest_template_as_valid_toml() {
        let manifest = Manifest {
            app: App {
                id: "com.example.hello".to_string(),
                name: "Hello".to_string(),
                version: "0.1.0".to_string(),
                entry: PathBuf::from("hello.wasm"),
                world: PHASE2_CLI_WORLD.to_string(),
            },
            capabilities: vec![CapabilityRequest {
                cap: "io.stdout".to_string(),
                rationale: "Print output".to_string(),
                required: true,
            }],
        };

        let rendered = manifest.to_toml_pretty().expect("render manifest");
        assert!(rendered.contains("[app]"));
        assert!(rendered.contains("[[capabilities]]"));
        assert_eq!(
            Manifest::parse(&rendered).expect("parse rendered"),
            manifest
        );
    }
}
