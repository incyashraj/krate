//! Krate sidecar manifest parser.
//!
//! A Krate app is still a plain `.wasm` component, but it may sit next to a
//! `manifest.toml` that declares identity, entry world, and requested
//! capabilities.

use std::{
    collections::BTreeSet,
    fmt,
    path::{Path, PathBuf},
    str::FromStr,
};

use krate_adapter_common::path::LogicalPath;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const PHASE2_CLI_WORLD: &str = "krate:app/cli@0.1.0";
pub const PHASE3_GUI_WORLD: &str = "krate:app/gui@0.2.0";

/// Older world names this runtime still hosts (IC-016).
///
/// The compatibility window, written down. Krate's promise is that a file
/// somebody was sent keeps opening, and an app is not broken merely because
/// its contract has a lower number than today's.
///
/// Hosting an older world costs nothing at the linker: a component imports
/// only the interfaces it actually uses, so a world that has since GROWN
/// still satisfies it. Measured rather than assumed -- adding an interface
/// to the world and rebuilding left all 16 shipped apps running. What was
/// stopping them was this list, which did not exist: an app declaring
/// `krate:app/gui@0.1.0` was refused at manifest validation, before the
/// linker it would have satisfied was ever consulted.
///
/// An entry leaves this list only when its contract can no longer be
/// honoured -- an interface removed, or a signature changed in a way no
/// adaptation covers. That is a deliberate, announced act, and the refusal
/// says so rather than reading as a defect.
const SUPPORTED_OLDER_WORLDS: &[(&str, AppWorld)] = &[
    // The GUI world before the Phase 3 interfaces were added. Its apps
    // import a strict subset of what gui@0.2.0 provides.
    ("krate:app/gui@0.1.0", AppWorld::Phase3Gui),
];

/// World names this runtime deliberately no longer hosts, and why.
///
/// Named rather than merely absent, so the refusal can say what happened
/// instead of "unsupported", which reads as a bug in the file. A person
/// whose app stopped opening deserves the reason and the move.
const RETIRED_WORLDS: &[(&str, &str)] = &[(
    "krate:phase1/host@0.0.1",
    "Phase 1 was the prototype interface; every app has been rebuilt since. \
     Open it with `krate` and choose \"Make a change\" to rebuild it against \
     the current one.",
)];
const MAX_NET_CONNECT_HOST_BYTES: usize = 253;

const KRATE_CAPABILITY_SPECS: &[CapabilitySpec] = &[
    CapabilitySpec::resource_free(CapabilityPhase::Phase2, "io", "stdin", true),
    CapabilitySpec::resource_free(CapabilityPhase::Phase2, "io", "stdout", true),
    CapabilitySpec::resource_free(CapabilityPhase::Phase2, "io", "stderr", true),
    CapabilitySpec::resource_free(CapabilityPhase::Phase2, "io", "args", true),
    CapabilitySpec::resource_free(CapabilityPhase::Phase2, "io", "log", true),
    CapabilitySpec::resource_scoped(CapabilityPhase::Phase2, "fs", "read", "<path-glob>", false),
    CapabilitySpec::resource_scoped(CapabilityPhase::Phase2, "fs", "write", "<path-glob>", false),
    CapabilitySpec::resource_scoped(CapabilityPhase::Phase2, "fs", "list", "<path-glob>", false),
    // The app's own key-value store. Resource-free because there is no path to
    // scope: the app cannot name a location, so the grant is simply "may this
    // app remember things" rather than "may this app read this folder".
    CapabilitySpec::resource_free(CapabilityPhase::Phase2, "store", "kv", false),
    // The app's own database. Resource-free for the same reason as kv: the app
    // names tables, never a file, so the grant is "may this app keep a
    // database" rather than access to a location.
    CapabilitySpec::resource_free(CapabilityPhase::Phase2, "store", "sql", false),
    // Sign-in tokens and keys the app keeps for itself, encrypted at rest.
    CapabilitySpec::resource_free(CapabilityPhase::Phase2, "store", "secret", false),
    // A key-value bucket shared between the machines that hold its invite
    // code, synced through krate.tech. Resource-free: the app names keys,
    // never a location. Never default-granted -- data leaving the machine is
    // exactly what a person must be asked about, and the consent wording
    // says who can see it (anyone with the code).
    CapabilitySpec::resource_free(CapabilityPhase::Phase2, "store", "shared", false),
    // Random bytes from the OS. Resource-free because there is nothing to
    // scope -- entropy has no location and reveals nothing about the machine.
    //
    // Not default-granted even so. It costs an app one line to declare, and a
    // person reading the permission list learns that this app draws random
    // numbers, which is worth knowing for anything that generates keys or ids.
    CapabilitySpec::resource_free(CapabilityPhase::Phase2, "random", "bytes", false),
    CapabilitySpec::resource_scoped(
        CapabilityPhase::Phase2,
        "fs",
        "remove",
        "<path-glob>",
        false,
    ),
    CapabilitySpec::resource_scoped(CapabilityPhase::Phase2, "fs", "mkdir", "<path-glob>", false),
    CapabilitySpec::resource_scoped(
        CapabilityPhase::Phase2,
        "net",
        "connect",
        "<host>:<port>",
        false,
    ),
    CapabilitySpec::resource_free(CapabilityPhase::Phase2, "time", "clock", true),
    CapabilitySpec::resource_free(CapabilityPhase::Phase2, "time", "monotonic", true),
    CapabilitySpec::resource_free(CapabilityPhase::Phase2, "time", "sleep", true),
    CapabilitySpec::resource_free(CapabilityPhase::Phase2, "locale", "info", true),
    CapabilitySpec::resource_free(CapabilityPhase::Phase2, "locale", "format", true),
    CapabilitySpec::resource_scoped(CapabilityPhase::Phase3, "ui", "window", "create", true),
    CapabilitySpec::resource_scoped(CapabilityPhase::Phase3, "ui", "clipboard", "read", false),
    CapabilitySpec::resource_scoped(CapabilityPhase::Phase3, "ui", "clipboard", "write", false),
    CapabilitySpec::resource_scoped(CapabilityPhase::Phase3, "ui", "menu", "system", false),
    // Handing a link to the person's browser. Not default-granted: opening a
    // URL is an outward action, and an app that does it unasked is spam.
    CapabilitySpec::resource_free(CapabilityPhase::Phase3, "ui", "open-url", false),
    // Desktop notifications, for the same reason.
    CapabilitySpec::resource_free(CapabilityPhase::Phase3, "ui", "notify", false),
    CapabilitySpec::resource_scoped(
        CapabilityPhase::Phase3,
        "ui",
        "dropzone",
        "<mime-type>",
        false,
    ),
    // Message and confirm boxes show text and take a click -- no data
    // moves -- so they are granted to every app. The WILDCARD is not: a
    // default-granted `ui.dialog:*` silently covered file-open, file-save
    // and open-folder at the policy layer, which made the "explicit ask"
    // promotion of the file dialogs hollow -- every app had them through
    // the star without declaring anything (K-086). `*` stays declarable,
    // as an explicit ask meaning "all dialogs", and walls as one.
    CapabilitySpec::resource_scoped(CapabilityPhase::Phase3, "ui", "dialog", "message", true),
    CapabilitySpec::resource_scoped(CapabilityPhase::Phase3, "ui", "dialog", "confirm", true),
    CapabilitySpec::resource_scoped(CapabilityPhase::Phase3, "ui", "dialog", "*", false),
    // The file dialogs are explicit asks. They were default-granted while
    // unimplemented ("a request that does not work would put a line in front
    // of a person that means nothing"), with a note that said to promote them
    // the day dialogs landed on all three systems. That day came and the note
    // sat stale -- rfd serves the picker on macOS, Windows and Linux
    // (choose_file_on_host) -- until an outside reviewer quoted the comment
    // back as proof the permission list names things that do not work.
    // Choosing a file is a real decision, so it is asked for.
    CapabilitySpec::resource_scoped(CapabilityPhase::Phase3, "ui", "dialog", "file-open", false),
    CapabilitySpec::resource_scoped(CapabilityPhase::Phase3, "ui", "dialog", "file-save", false),
    // The folder pick is the biggest dialog grant -- a subtree, not one
    // file -- so it is an explicit ask like the file dialogs. It is also the
    // designed answer to K-075: apps that work on "a folder of yours" ask
    // for this instead of an fs scope, and the person's pick draws the
    // boundary at run time.
    CapabilitySpec::resource_scoped(
        CapabilityPhase::Phase3,
        "ui",
        "dialog",
        "open-folder",
        false,
    ),
    // Drawing with the GPU. Default-granted. When this was written canvas2d
    // refused every call; it now carries 16 working functions and shipped
    // games sit on it, so the name gates real behaviour. The stale version of
    // this comment ("refuse every call") was quoted in the same review as
    // proof the list cannot be trusted -- comments about capability reality
    // rot faster than any other kind, which is why interface-parity.md is
    // generated instead. See docs/book/src/reference/interface-parity.md.
    CapabilitySpec::resource_scoped(CapabilityPhase::Phase3, "gfx", "gpu", "basic", true),
    // GPU compute, which is a general-purpose processor an app can keep busy,
    // so it is asked for rather than granted. Also not implemented yet.
    CapabilitySpec::resource_scoped(CapabilityPhase::Phase3, "gfx", "gpu", "compute", false),
    // Playing sound OUT through the speakers is default-granted, like opening a
    // window: it is the app doing its obvious job, not reaching for the
    // person's data. It was classified as sensitive as the microphone, which
    // meant a music player opened silent -- the audio was never granted and,
    // being optional, was never even asked about. Output is not input.
    CapabilitySpec::resource_free(CapabilityPhase::Phase3, "audio", "playback", true),
    // Capturing sound FROM the microphone stays opt-in: that is the person's
    // room, and recording it is exactly the kind of thing consent exists for.
    CapabilitySpec::resource_free(CapabilityPhase::Phase3, "audio", "capture", false),
    // The camera is the most sensitive door Krate opens: it is the person's
    // face and their room, and unlike a file pick there is no moment where
    // they choose what it sees. Opt-in, always asked for, and never granted
    // by default -- and the host asks the operating system too, so the
    // machine's own indicator light is the second, unfakeable signal.
    CapabilitySpec::resource_free(CapabilityPhase::Phase3, "camera", "capture", false),
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
        validate_display_text("app.name", &self.app.name)?;
        validate_required("app.version", &self.app.version)?;
        validate_required_path("app.entry", &self.app.entry)?;

        self.app_world()?;

        let mut seen = BTreeSet::new();
        for request in &self.capabilities {
            let cap: Capability = request.cap.parse()?;
            validate_required("capability.rationale", &request.rationale)?;
            validate_display_text("capability.rationale", &request.rationale)?;
            if !seen.insert(cap.to_string()) {
                return Err(ManifestError::DuplicateCapability {
                    cap: request.cap.clone(),
                });
            }
        }

        Ok(())
    }

    pub fn app_world(&self) -> Result<AppWorld> {
        AppWorld::from_world_name(&self.app.world)
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
pub enum AppWorld {
    Phase2Cli,
    Phase3Gui,
}

impl AppWorld {
    pub fn from_world_name(world: &str) -> Result<Self> {
        match world {
            PHASE2_CLI_WORLD => return Ok(Self::Phase2Cli),
            PHASE3_GUI_WORLD => return Ok(Self::Phase3Gui),
            _ => {}
        }
        // Inside the compatibility window: an older contract this runtime
        // still honours, hosted by the world that superseded it.
        if let Some((_, hosted_by)) = SUPPORTED_OLDER_WORLDS
            .iter()
            .find(|(name, _)| *name == world)
        {
            return Ok(*hosted_by);
        }
        Err(ManifestError::UnsupportedWorld {
            world: world.to_string(),
        })
    }

    /// Why a world is not hosted, in words a person can act on.
    ///
    /// A retired contract and a name Krate has never seen are different
    /// situations with different moves, and "unsupported app world" for both
    /// tells somebody nothing about which they have.
    pub fn explain_unsupported(world: &str) -> String {
        if let Some((_, reason)) = RETIRED_WORLDS.iter().find(|(name, _)| *name == world) {
            return format!("`{world}` is no longer supported. {reason}");
        }
        format!(
            "`{world}` is not an app interface this copy of Krate knows. If \
             somebody sent you this app recently, it may need a newer Krate: \
             https://krate.tech/open"
        )
    }

    /// Every world name this runtime accepts, current and older.
    pub fn supported_world_names() -> Vec<&'static str> {
        let mut names = vec![PHASE2_CLI_WORLD, PHASE3_GUI_WORLD];
        names.extend(SUPPORTED_OLDER_WORLDS.iter().map(|(name, _)| *name));
        names
    }

    pub fn world_name(self) -> &'static str {
        match self {
            Self::Phase2Cli => PHASE2_CLI_WORLD,
            Self::Phase3Gui => PHASE3_GUI_WORLD,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityPhase {
    Phase2,
    Phase3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilitySpec {
    phase: CapabilityPhase,
    module: &'static str,
    action: &'static str,
    resource: Option<&'static str>,
    default_granted: bool,
}

impl CapabilitySpec {
    const fn resource_free(
        phase: CapabilityPhase,
        module: &'static str,
        action: &'static str,
        default_granted: bool,
    ) -> Self {
        Self {
            phase,
            module,
            action,
            resource: None,
            default_granted,
        }
    }

    const fn resource_scoped(
        phase: CapabilityPhase,
        module: &'static str,
        action: &'static str,
        resource: &'static str,
        default_granted: bool,
    ) -> Self {
        Self {
            phase,
            module,
            action,
            resource: Some(resource),
            default_granted,
        }
    }

    pub fn phase(&self) -> CapabilityPhase {
        self.phase
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

    fn default_capability(&self) -> Result<Capability> {
        Capability::new(self.module(), self.action(), self.resource())
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
                reason: "unknown Krate capability".to_string(),
            }
        })?;
        let resource_was_present = resource.is_some();
        let mut resource = resource
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

        if let Some(current) = resource.as_deref() {
            resource = Some(
                canonicalize_capability_resource(module, action, current).ok_or_else(|| {
                    ManifestError::InvalidCapability {
                        cap: format!("{cap_name}:{current}"),
                        reason: "failed to canonicalize capability resource".to_string(),
                    }
                })?,
            );
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
    KRATE_CAPABILITY_SPECS
        .iter()
        .filter(|spec| spec.default_granted())
        .map(|spec| {
            spec.default_capability()
                .expect("default capability specs are valid")
        })
        .collect()
}

pub fn supported_capability_specs() -> &'static [CapabilitySpec] {
    KRATE_CAPABILITY_SPECS
}

pub fn supported_phase2_capability_specs() -> impl Iterator<Item = &'static CapabilitySpec> {
    KRATE_CAPABILITY_SPECS
        .iter()
        .filter(|spec| spec.phase() == CapabilityPhase::Phase2)
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
    // Carries the explanation rather than only the name: a retired contract
    // and a name Krate has never seen are different situations with
    // different moves, and one message for both reads as a defect in the
    // person's file (IC-016).
    #[error("{}", AppWorld::explain_unsupported(world))]
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

/// A field the permission wall shows to a person, checked for characters that
/// could rewrite the wall rather than appear in it.
///
/// The mobile players hand the wall its request list as text, and the wall
/// finds the boundaries by splitting on U+001E and U+001C. Nothing stopped a
/// manifest string from containing them, so an app name of
/// `Notes<U+001E>net.connect<U+001C>Sync your notes<U+001C>1` displayed a
/// network request the app had never declared, under a name that still read
/// as "Notes". A crafted rationale was worse: it could flip its own capability
/// from required to optional, and required rows are the ones a person is not
/// allowed to switch off.
///
/// The players no longer pass structure as text, so this is the second lock
/// rather than the only one -- a bundle carrying these characters in a
/// displayed field does not open at all, on any platform, whatever the caller
/// does with the strings afterwards.
///
/// Only C0/C1 controls and the Unicode separators are refused. Ordinary text
/// in any language passes untouched: this must never become a filter on what
/// an app may call itself.
fn validate_display_text(field: &'static str, value: &str) -> Result<()> {
    for ch in value.chars() {
        let forbidden = match ch {
            // Tab, newline and carriage return are controls a person might
            // reasonably type; they are still refused, because a name that
            // spans lines misrepresents itself in a one-line row.
            '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}' => true,
            // The Unicode line/paragraph separators, which split text in the
            // same way without being C0 controls.
            '\u{2028}' | '\u{2029}' => true,
            // Bidirectional overrides: these reorder what is drawn without
            // changing what is parsed, so a capability can be made to read as
            // a different one.
            '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => true,
            _ => false,
        };
        if forbidden {
            return Err(ManifestError::InvalidIdentifier {
                field,
                reason: format!(
                    "contains U+{:04X}, a control or text-direction character. \
                     These do not show up as themselves on the permission \
                     screen -- they change what it says.",
                    ch as u32
                ),
            });
        }
    }
    Ok(())
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
        // `~` is not a path Krate can honour, and accepting it produced the
        // worst kind of failure: a manifest saying `fs.read:~/Pictures/**`
        // packed fine, and the app then reported "not found" for a folder the
        // person could see in their file manager. Every app path resolves
        // inside the sandbox root, so a home directory is not reachable by
        // design -- `~` would have to be either a lie or a hole in that.
        //
        // Refused here, where the fix is one line in a manifest, rather than
        // silently at run time.
        // `$HOME/x` and `%USERPROFILE%\\x` are the same mistake wearing a
        // different hat: a shell would expand them, Krate does not, and the
        // grant ends up naming a folder literally called `$HOME`.
        if resource.starts_with('$') || resource.starts_with('%') {
            return Err(format!(
                "filesystem resource `{resource}` looks like a shell variable, which Krate \
                 does not expand: it would match a folder literally named that. Use a path \
                 relative to the app's sandbox instead."
            ));
        }
        if resource.starts_with('~') {
            return Err(format!(
                "filesystem resource `{resource}` starts with `~`, which Krate does not \
                 expand: every app path resolves inside the sandbox the app was given, \
                 so a home directory is not reachable. Use a path relative to that \
                 sandbox instead -- `images/**` rather than `~/Pictures/**` -- and let \
                 the person choose files outside it with `ui.dialog:file-open`."
            ));
        }
        LogicalPath::parse(resource)
            .map(|_| ())
            .map_err(|err| format!("invalid filesystem resource pattern: {err}"))?;
        return Ok(());
    }

    if module == "net" && action == "connect" {
        validate_connect_resource(resource)?;
        return Ok(());
    }

    // `store.group` is not declarable yet -- see the test on
    // validate_group_name. The dispatch stays out until the capability
    // exists, so nothing suggests a grant that does nothing.

    if module == "ui" {
        validate_ui_resource(action, resource)?;
        return Ok(());
    }

    if module == "gfx" && action == "gpu" {
        validate_one_of(resource, &["basic", "compute"], "GPU resource")?;
    }

    Ok(())
}

fn canonicalize_capability_resource(module: &str, action: &str, resource: &str) -> Option<String> {
    if module == "fs" {
        return LogicalPath::parse(resource)
            .ok()
            .map(|path| path.as_str().to_string());
    }
    if module == "net" && action == "connect" {
        return canonicalize_connect_resource(resource);
    }
    if module == "ui" || module == "gfx" {
        return Some(resource.to_ascii_lowercase());
    }
    Some(resource.to_string())
}

fn validate_ui_resource(action: &str, resource: &str) -> std::result::Result<(), String> {
    match action {
        "window" => validate_one_of(resource, &["create"], "window resource"),
        "clipboard" => validate_one_of(resource, &["read", "write", "*"], "clipboard resource"),
        "menu" => validate_one_of(resource, &["system"], "menu resource"),
        "dialog" => validate_one_of(
            resource,
            // Kept in step with the spec table above by the cross-check test:
            // this list rotting behind a new spec entry is exactly what
            // blocked open-folder for one build.
            &[
                "message",
                "confirm",
                "file-open",
                "file-save",
                "open-folder",
                "*",
            ],
            "dialog resource",
        ),
        "dropzone" => validate_mime_resource(resource),
        _ => Ok(()),
    }
}

fn validate_one_of(
    resource: &str,
    accepted: &[&str],
    label: &str,
) -> std::result::Result<(), String> {
    let normalized = resource.to_ascii_lowercase();
    if accepted.contains(&normalized.as_str()) {
        Ok(())
    } else {
        Err(format!("{label} must be one of {}", accepted.join(", ")))
    }
}

fn validate_mime_resource(resource: &str) -> std::result::Result<(), String> {
    if resource == "*" || resource == "*/*" {
        return Ok(());
    }
    let Some((top, sub)) = resource.split_once('/') else {
        return Err("MIME resource must use <type>/<subtype>".to_string());
    };
    for part in [top, sub] {
        if part.is_empty()
            || !part
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '+' | '.' | '*'))
        {
            return Err("MIME resource contains unsupported characters".to_string());
        }
    }
    Ok(())
}

fn canonicalize_connect_resource(resource: &str) -> Option<String> {
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

/// A shared-group name, which becomes a directory component (IC-738).
///
/// Held to a narrow shape rather than sanitised, because sanitising is how
/// two different names quietly become one store: `family budget` and
/// `family/budget` both flattening to `family_budget` would put two groups
/// that were never meant to meet in the same bucket. A name Krate cannot
/// represent exactly is refused instead.
///
/// It is also shown to a person on the permission wall, so it must read as
/// itself: no control characters, no leading or trailing punctuation.
#[cfg_attr(not(test), allow(dead_code))]
fn validate_group_name(name: &str) -> std::result::Result<(), String> {
    if name.is_empty() {
        return Err("a shared group needs a name".to_string());
    }
    if name.len() > 64 {
        return Err(format!(
            "shared group name is {} characters; 64 is the limit, so it fits \
             on the permission screen a person reads",
            name.len()
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(format!(
            "shared group name `{name}` may use only lowercase letters, \
             digits and hyphens. Anything else has to be flattened to sit in \
             a path, and two names flattening to one would silently share a \
             store that was never meant to be shared."
        ));
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err(format!(
            "shared group name `{name}` cannot start or end with a hyphen"
        ));
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
    if host.len() > MAX_NET_CONNECT_HOST_BYTES {
        return Err("network endpoint host is too long".to_string());
    }
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
    let Ok(ip) = host.parse::<std::net::Ipv4Addr>() else {
        return false;
    };
    if ip.is_unspecified() || ip.is_multicast() || ip == std::net::Ipv4Addr::BROADCAST {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A manifest whose display fields carry the characters the permission
    /// wall used to split on does not open (IC-292, K-242).
    ///
    /// The wall showed the app's name and each rationale, and the players
    /// joined those strings with U+001E and U+001C. Nothing checked the
    /// strings for those characters, so an app declaring only `ui.window`
    /// could call itself `Notes<RS>net.connect<FS>Sync your notes<FS>1` and
    /// the sheet drew a network request it had never declared -- under a name
    /// that still read "Notes".
    ///
    /// The players no longer pass structure as text, so this is the second
    /// lock. It is here because a bundle carrying these characters is hostile
    /// whatever any one caller does with the strings.
    #[test]
    fn a_name_or_rationale_carrying_wall_separators_is_refused() {
        // Exactly the injection that worked: the separators, plus the
        // Unicode paragraph separator and a bidi override, which reorder what
        // is drawn without changing what is parsed.
        // TOML's own escape, not Rust's: `\u{1e}` is a Rust literal and a TOML
        // parse error, so escape_default() here tests nothing but the TOML
        // reader's error message.
        let toml_escape = |ch: char| format!("\\u{:04X}", ch as u32);

        for bad in [
            '\u{1e}', '\u{1c}', '\u{0}', '\n', '\r', '\t', '\u{7f}', '\u{9f}', '\u{2028}',
            '\u{2029}', '\u{202e}', '\u{2066}',
        ] {
            let manifest = format!(
                "[app]\nid = \"com.example.notes\"\nname = \"Notes{}net.connect\"\n\
                 version = \"1.0.0\"\n\
                 entry = \"app.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n\n\
                 [[capabilities]]\ncap = \"ui.window:create\"\n\
                 rationale = \"Draw the notes window\"\nrequired = true\n",
                toml_escape(bad)
            );
            let error = Manifest::parse(&manifest).expect_err(&format!(
                "an app name containing U+{:04X} must not parse",
                bad as u32
            ));
            let text = error.to_string();
            assert!(
                text.contains("app.name"),
                "the refusal must name the field: {text}"
            );

            // And the same character in a rationale, which is the worse
            // direction: a crafted rationale could flip its own capability
            // from required to optional, and required rows are the ones a
            // person is not allowed to switch off.
            let manifest = format!(
                "[app]\nid = \"com.example.notes\"\nname = \"Notes\"\nversion = \"1.0.0\"\n\
                 entry = \"app.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n\n\
                 [[capabilities]]\ncap = \"ui.window:create\"\n\
                 rationale = \"Draw the window{}0\"\nrequired = true\n",
                toml_escape(bad)
            );
            let error = Manifest::parse(&manifest).expect_err(&format!(
                "a rationale containing U+{:04X} must not parse",
                bad as u32
            ));
            assert!(
                error.to_string().contains("capability.rationale"),
                "the refusal must name the field: {error}"
            );
        }
    }

    /// Every input the fuzz campaign found interesting, replayed on every
    /// test run (IC-163).
    ///
    /// The nightly fuzz explores; what it learns must not evaporate. Each
    /// file under fuzz/regressions/manifest_parse goes through the same call
    /// the fuzz target makes -- parse may accept or refuse, it must not
    /// panic. When a nightly finds a crash, its minimized input is added
    /// here and the crash can never quietly return.
    #[test]
    fn every_fuzz_regression_input_parses_without_panicking() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fuzz/regressions/manifest_parse");
        let mut fed = 0;
        for entry in std::fs::read_dir(&dir).expect("the regression corpus exists") {
            let path = entry.expect("readable dir entry").path();
            let bytes = std::fs::read(&path).expect("readable fixture");
            // Exactly the fuzz target's body (fuzz/fuzz_targets/manifest_parse.rs):
            // utf8 or skip, then parse, outcome ignored.
            if let Ok(input) = std::str::from_utf8(&bytes) {
                let _ = Manifest::parse(input);
            }
            fed += 1;
        }
        assert!(
            fed >= 4,
            "the regression corpus went missing or empty ({fed} inputs) -- \
             this lane only means something while it feeds real inputs"
        );
    }

    /// A shared-group name is refused rather than flattened (IC-738).
    ///
    /// The validator is here ahead of the capability it will gate. That is
    /// deliberate: `store.group` cannot be declarable until a host call
    /// exists for it -- a capability an app can ask for, a person can grant,
    /// and nothing then does is worse than no capability -- and the runtime
    /// cannot gain that interface until it can host older worlds (IC-016),
    /// or all 53 existing apps stop starting.
    ///
    /// What CAN be settled now is the naming rule, and settling it early is
    /// the point: names are refused, never sanitised, because two different
    /// names flattening to one identifier would silently share a store that
    /// was never meant to be shared -- the exact collision the capability
    /// exists to replace with a deliberate act.
    #[test]
    fn a_shared_group_name_is_refused_rather_than_flattened() {
        for good in ["family-budget", "notes2", "a", "team-1-plans"] {
            validate_group_name(good)
                .unwrap_or_else(|err| panic!("{good:?} is an ordinary group name: {err}"));
        }

        // Every one of these would flatten into a name that already exists,
        // or into something that is not a single path component.
        for hostile in [
            "family budget", // space -> would flatten onto family-budget
            "family/budget", // separator -> a second path component
            "family_budget", // underscore -> another flattening collision
            "Family-Budget", // case -> two names, one directory on macOS
            "../escape",
            "-leading",
            "trailing-",
            "",
        ] {
            assert!(
                validate_group_name(hostile).is_err(),
                "{hostile:?} must be refused, not flattened into some other \
                 group's store",
            );
        }

        // Long enough to be unreadable on the screen a person decides from.
        assert!(validate_group_name(&"a".repeat(65)).is_err());
    }

    /// The check must not become a filter on what an app may call itself.
    /// Every one of these is ordinary text somebody would legitimately use.
    #[test]
    fn ordinary_names_and_rationales_still_parse() {
        for name in [
            "Notes",
            "Café Notes",
            "メモ帳",
            "Notes — the good one",
            "Q&A: 100% offline",
            "Ünïcödé Nötes",
            "ملاحظات",
        ] {
            let manifest = format!(
                "[app]\nid = \"com.example.notes\"\nname = \"{name}\"\nversion = \"1.0.0\"\n\
                 entry = \"app.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n\n\
                 [[capabilities]]\ncap = \"ui.window:create\"\n\
                 rationale = \"Draw the window — it is where you type\"\nrequired = true\n"
            );
            Manifest::parse(&manifest)
                .unwrap_or_else(|err| panic!("{name:?} is an ordinary name: {err}"));
        }
    }

    /// Two truths about dialog resources exist -- the spec table and the
    /// validator's list -- and they drifted the day open-folder landed in
    /// one but not the other. This walks every dialog spec through parse so
    /// the drift cannot ship again.
    #[test]
    fn every_dialog_spec_resource_is_accepted_by_the_validator() {
        for spec in supported_capability_specs() {
            let name = spec.name();
            if !name.starts_with("ui.dialog:") {
                continue;
            }
            let manifest = format!(
                "[app]\nid = \"dev.krate.d\"\nname = \"D\"\nversion = \"0.1.0\"\n\
                 entry = \"code.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n\
                 [[capabilities]]\ncap = \"{name}\"\nrationale = \"r\"\nrequired = true\n"
            );
            Manifest::parse(&manifest).unwrap_or_else(|e| panic!("spec {name} must parse: {e:#}"));
        }
    }

    const EXAMPLE: &str = r#"
        [app]
        id = "com.example.hello"
        name = "Hello"
        version = "1.0.0"
        entry = "hello.wasm"
        world = "krate:app/cli@0.1.0"

        [[capabilities]]
        cap = "fs.read:documents/notes/**"
        rationale = "Read saved notes"
        required = true

        [[capabilities]]
        cap = "net.connect:api.example.com:443"
        rationale = "Sync to cloud"
        required = false
    "#;

    #[test]
    fn a_home_relative_path_is_refused_where_it_is_written() {
        // An image viewer was ported with `fs.read:~/Pictures/**`. It packed,
        // it ran, and it reported "not found" for a folder the person could
        // see in their file manager -- because every app path resolves inside
        // the sandbox root, so `~` was matched as a directory literally named
        // that. Expanding it would be a hole in the containment rather than a
        // fix, so it is refused at pack time where it costs one line.
        let manifest = format!(
            "{EXAMPLE}\n[[capabilities]]\ncap = \"fs.read:~/Pictures/**\"\n\
             rationale = \"show photos\"\nrequired = true\n"
        );
        let err = Manifest::parse(&manifest).expect_err("`~` must be refused");
        let text = err.to_string();
        assert!(
            text.contains('~') && text.contains("sandbox"),
            "the error must say why and what to do instead: {text}"
        );
        // And it points at the way an app legitimately reaches a file outside
        // its sandbox: the person choosing one.
        assert!(text.contains("ui.dialog:file-open"), "{text}");

        // A shell variable is the same mistake wearing a different hat: it
        // would be expanded by a shell and is a literal folder name here.
        for shell in ["fs.read:$HOME/x", "fs.read:%USERPROFILE%/x"] {
            let manifest = format!(
                "{EXAMPLE}\n[[capabilities]]\ncap = \"{shell}\"\n\
                 rationale = \"r\"\nrequired = true\n"
            );
            assert!(
                Manifest::parse(&manifest).is_err(),
                "{shell} must be refused"
            );
        }

        // And an ordinary sandbox-relative path still works, which is the
        // whole point -- this must not become a wall around normal paths.
        let ok = format!(
            "{EXAMPLE}\n[[capabilities]]\ncap = \"fs.read:images/**\"\n\
             rationale = \"r\"\nrequired = true\n"
        );
        assert!(Manifest::parse(&ok).is_ok());
    }

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
        assert_eq!(caps[0].resource(), Some("documents/notes/**"));
    }

    #[test]
    fn accepts_phase3_gui_world_as_draft_manifest_target() {
        let input = EXAMPLE.replace(PHASE2_CLI_WORLD, PHASE3_GUI_WORLD);
        let manifest = Manifest::parse(&input).expect("parse Phase 3 gui manifest");

        assert_eq!(
            manifest.app_world().expect("app world"),
            AppWorld::Phase3Gui
        );
    }

    #[test]
    fn rejects_unknown_worlds() {
        // A name Krate has never defined. Note this is NOT `gui@0.1.0`,
        // which this test used to use: that is a real older contract and
        // hosting it is the point of IC-016.
        for unknown in [
            "krate:app/cli@9.9.9",
            "krate:app/holodeck@0.1.0",
            "some:other/world@1.0.0",
            "",
        ] {
            let input = EXAMPLE.replace(PHASE2_CLI_WORLD, unknown);
            let err = Manifest::parse(&input)
                .expect_err(&format!("{unknown:?} is not a world Krate knows"));
            assert!(matches!(err, ManifestError::UnsupportedWorld { .. }));
        }
    }

    /// An app built against an older contract still opens (IC-016).
    ///
    /// Krate's promise is that a file somebody was sent keeps opening, and an
    /// app is not broken merely because its world has a lower number than
    /// today's. Before this, `gui@0.1.0` was refused at manifest validation
    /// -- before the linker it would have satisfied was ever consulted.
    #[test]
    fn an_app_built_against_an_older_world_still_opens() {
        let input = EXAMPLE.replace(PHASE2_CLI_WORLD, "krate:app/gui@0.1.0");
        let manifest = Manifest::parse(&input).expect("an older GUI world is still hosted");
        assert_eq!(
            manifest.app_world().expect("world"),
            AppWorld::Phase3Gui,
            "an older world is hosted by the one that superseded it",
        );
    }

    /// The window is a list, not a guess: every name in it resolves, and the
    /// current worlds keep resolving to themselves.
    #[test]
    fn every_supported_world_name_resolves() {
        for name in AppWorld::supported_world_names() {
            AppWorld::from_world_name(name)
                .unwrap_or_else(|err| panic!("{name} is advertised as supported: {err}"));
        }
        assert_eq!(
            AppWorld::from_world_name(PHASE2_CLI_WORLD).expect("cli"),
            AppWorld::Phase2Cli
        );
        assert_eq!(
            AppWorld::from_world_name(PHASE3_GUI_WORLD).expect("gui"),
            AppWorld::Phase3Gui
        );
    }

    /// A retired contract says what happened; an unknown name says something
    /// else. "Unsupported" for both tells a person nothing about which they
    /// have, and reads as a defect in their file.
    #[test]
    fn a_retired_world_is_explained_differently_from_an_unknown_one() {
        let retired = AppWorld::explain_unsupported("krate:phase1/host@0.0.1");
        assert!(
            retired.contains("no longer supported") && retired.contains("Make a change"),
            "a retired world must say what happened and what to do: {retired}"
        );

        let unknown = AppWorld::explain_unsupported("krate:app/holodeck@0.1.0");
        assert!(
            unknown.contains("newer Krate"),
            "an unknown world is most likely a newer one: {unknown}"
        );
        assert_ne!(retired, unknown, "the two situations differ");
    }

    #[test]
    fn rejects_duplicate_capabilities() {
        let input = format!(
            "{EXAMPLE}\n[[capabilities]]\ncap = \"fs.read:documents/notes/**\"\nrationale = \"again\"\nrequired = true\n"
        );
        let err = Manifest::parse(&input).expect_err("reject duplicate capability");

        assert!(matches!(err, ManifestError::DuplicateCapability { .. }));
    }

    #[test]
    fn parses_capability_parts() {
        let cap: Capability = "net.connect:API.Example.com:0443"
            .parse()
            .expect("parse cap");

        assert_eq!(cap.module(), "net");
        assert_eq!(cap.action(), "connect");
        assert_eq!(cap.resource(), Some("api.example.com:443"));
        assert_eq!(cap.to_string(), "net.connect:api.example.com:443");
    }

    #[test]
    fn rejects_duplicate_capabilities_after_net_resource_canonicalization() {
        let input = format!(
            "{EXAMPLE}\n[[capabilities]]\ncap = \"net.connect:API.Example.com:0443\"\nrationale = \"again\"\nrequired = false\n"
        );
        let err = Manifest::parse(&input).expect_err("reject canonical duplicate capability");

        assert!(matches!(err, ManifestError::DuplicateCapability { .. }));
    }

    #[test]
    fn rejects_unknown_capability_names() {
        let err = "net.listen:127.0.0.1:8080"
            .parse::<Capability>()
            .expect_err("reject unknown cap");

        assert!(matches!(err, ManifestError::InvalidCapability { .. }));
    }

    #[test]
    fn parses_phase3_gui_capability_names() {
        let window: Capability = "ui.window:create".parse().expect("window cap");
        let dialog: Capability = "ui.dialog:*".parse().expect("dialog wildcard cap");
        let gfx: Capability = "gfx.gpu:basic".parse().expect("gfx cap");
        let playback: Capability = "audio.playback".parse().expect("audio cap");

        assert_eq!(window.to_string(), "ui.window:create");
        assert!(window.is_default_granted());
        assert_eq!(dialog.resource(), Some("*"));
        // The wildcard stopped being default-granted the day it was found
        // silently covering the privileged dialogs (K-086): it is now an
        // explicit ask meaning "all dialogs".
        assert!(!dialog.is_default_granted());
        assert_eq!(gfx.to_string(), "gfx.gpu:basic");
        assert!(gfx.is_default_granted());
        assert_eq!(playback.to_string(), "audio.playback");
        // Speaker output is default-granted -- the app's obvious job, like a
        // window. The microphone is not: capturing the person's room is opt-in.
        assert!(playback.is_default_granted());
        let capture: Capability = "audio.capture".parse().expect("mic cap");
        assert!(!capture.is_default_granted());
    }

    #[test]
    fn rejects_invalid_phase3_gui_capability_resources() {
        for input in [
            "ui.window:read",
            "ui.clipboard:delete",
            "ui.dialog:camera",
            "gfx.gpu:admin",
            "ui.dropzone:not-a-mime",
        ] {
            let err = input
                .parse::<Capability>()
                .expect_err("invalid Phase 3 resource should fail");
            assert!(
                matches!(err, ManifestError::InvalidCapability { .. }),
                "unexpected error type for `{input}`: {err:?}"
            );
        }
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
        assert_eq!(valid.resource(), Some("notes/**"));

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
    fn rejects_duplicate_capabilities_after_fs_resource_canonicalization() {
        let input = r#"
        [app]
        id = "com.example.hello"
        name = "Hello"
        version = "1.0.0"
        entry = "hello.wasm"
        world = "krate:app/cli@0.1.0"

        [[capabilities]]
        cap = "fs.read:./notes/**"
        rationale = "Read notes"
        required = true

        [[capabilities]]
        cap = "fs.read:notes\\**"
        rationale = "Read notes again"
        required = true
    "#;

        let err = Manifest::parse(input).expect_err("reject canonical duplicate fs capability");
        assert!(matches!(err, ManifestError::DuplicateCapability { .. }));
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

        let long_host_pattern = format!(
            "net.connect:{}.{}.{}.{}:443",
            "a".repeat(63),
            "b".repeat(63),
            "c".repeat(63),
            "d".repeat(63)
        );

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
            "net.connect:001.2.3.4:443",
            "net.connect:api.*.example.com:443",
            "net.connect:*.*.example.com:443",
            "net.connect:exa*mple.com:443",
            "net.connect:api*.example.com:443",
            long_host_pattern.as_str(),
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
        let specs = supported_phase2_capability_specs().collect::<Vec<_>>();

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
    fn exposes_phase3_capability_specs_without_hiding_phase2_specs() {
        let specs = supported_capability_specs();

        assert!(specs.iter().any(|spec| {
            spec.display_pattern() == "ui.window:create"
                && spec.phase() == CapabilityPhase::Phase3
                && spec.default_granted()
        }));
        assert!(specs.iter().any(|spec| {
            spec.display_pattern() == "ui.clipboard:read"
                && spec.phase() == CapabilityPhase::Phase3
                && !spec.default_granted()
        }));
        assert!(specs.iter().any(|spec| {
            spec.display_pattern() == "audio.capture"
                && spec.phase() == CapabilityPhase::Phase3
                && !spec.default_granted()
        }));
        assert!(
            supported_phase2_capability_specs().all(|spec| spec.phase() == CapabilityPhase::Phase2)
        );
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
