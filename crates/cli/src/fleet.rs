//! The machine policy: what the organisation that manages this computer
//! allows Krate to do here (IC-949, FLEET-CONTRACT P).
//!
//! One rule over everything: **a policy only narrows.** Nothing in it makes
//! Krate allow what it would refuse without one. A policy that cannot be
//! read, names a version Krate does not know, or carries a key Krate does
//! not know, refuses to open apps and says why -- it never falls back to
//! "no policy", because a typo in a security setting must not read as
//! "allowed".
//!
//! Where it is read from, first source present wins:
//!
//! - macOS: the managed preferences domain `tech.krate.policy`
//!   (`/Library/Managed Preferences/tech.krate.policy.plist`, what a
//!   configuration profile writes), then
//!   `/Library/Application Support/Krate/policy.toml`.
//! - Windows: `HKLM\SOFTWARE\Policies\Krate`, then
//!   `%ProgramData%\Krate\policy.toml`.
//! - Linux: `/etc/krate/policy.toml`.
//!
//! A debug build also honours `KRATE_TEST_POLICY=<file>` in place of every
//! source (`none` for "no policy"), and `KRATE_TEST_POLICY_ROOT=<dir>`,
//! which puts the real source paths under a folder so the order between them
//! can be tested. A release build ignores both, like `KRATE_TEST_LEASE_KEY`.
//!
//! Every door that opens an app asks [`admit`] before the organisation
//! check in `access`, and writes the open record through [`crate::audit`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::Result;
use krate_policy::{
    DenyRule, OrgBlock, OrgCeiling, SessionPolicy, PRINT_CALL, SCREEN_CAPTURE_CALL,
};
use serde_json::{json, Value};

use crate::audit::{self, RecordDoor, Refused};

/// The environment variable that replaces every policy source, honoured
/// only in a debug build.
pub const TEST_POLICY_ENV: &str = "KRATE_TEST_POLICY";
/// The folder the real source paths are read under, debug builds only.
pub const TEST_POLICY_ROOT_ENV: &str = "KRATE_TEST_POLICY_ROOT";

/// The provider ids `[studio] providers` may name: the agent CLIs Krate
/// drives, and the two API vendors.
fn known_providers() -> Vec<&'static str> {
    let mut ids: Vec<&'static str> = crate::agent_provider::PROVIDERS
        .iter()
        .map(|provider| provider.name())
        .collect();
    ids.extend(["anthropic", "openai"]);
    ids
}

// ---------------------------------------------------------------- sources

/// What kind of place a policy came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    // Read only on macOS; elsewhere the variant is still the name a policy
    // record and the tests use, so it is not dead, only unbuilt.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    ManagedPreferences,
    Registry,
    File,
    Test,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ManagedPreferences => "managed-preferences",
            Self::Registry => "registry",
            Self::File => "file",
            Self::Test => "test",
        }
    }
}

/// One place a policy can be read from.
#[derive(Debug, Clone)]
pub struct Source {
    pub kind: SourceKind,
    pub location: String,
}

impl Source {
    fn json(&self) -> Value {
        json!({ "kind": self.kind.as_str(), "location": self.location })
    }
}

/// The policy in force on this computer, if any.
pub enum State {
    /// No source is present.
    None,
    /// A policy was read and every key in it is understood.
    InForce {
        policy: Box<Policy>,
        source: Source,
        ignored: Vec<Source>,
    },
    /// A source is present and cannot be used. Krate opens no apps.
    Unreadable {
        source: Source,
        problem: String,
        ignored: Vec<Source>,
    },
}

/// The policy in force, read once per process.
pub fn state() -> &'static State {
    static STATE: OnceLock<State> = OnceLock::new();
    STATE.get_or_init(load)
}

/// The policy in force, when there is one that can be used.
pub fn policy() -> Option<&'static Policy> {
    match state() {
        State::InForce { policy, .. } => Some(policy),
        _ => None,
    }
}

fn debug_env(name: &str) -> Option<String> {
    if cfg!(debug_assertions) {
        std::env::var(name).ok().filter(|value| !value.is_empty())
    } else {
        None
    }
}

/// The folder the system paths are read under: `/` except in a debug build
/// told otherwise. Windows re-roots `%ProgramData%` instead.
#[cfg_attr(windows, allow(dead_code))]
fn system_root() -> PathBuf {
    debug_env(TEST_POLICY_ROOT_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

#[cfg_attr(windows, allow(dead_code))]
fn under_root(path: &str) -> PathBuf {
    let root = system_root();
    if root == Path::new("/") {
        PathBuf::from(path)
    } else {
        root.join(path.trim_start_matches('/'))
    }
}

/// Where `%ProgramData%` is, re-rooted in a test.
#[cfg_attr(not(windows), allow(dead_code))]
fn program_data() -> PathBuf {
    if let Some(root) = debug_env(TEST_POLICY_ROOT_ENV) {
        return PathBuf::from(root).join("ProgramData");
    }
    std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
}

/// The sources this OS reads, in order.
fn candidates() -> Vec<Source> {
    let file = |path: PathBuf| Source {
        kind: SourceKind::File,
        location: path.display().to_string(),
    };
    #[cfg(target_os = "macos")]
    {
        vec![
            Source {
                kind: SourceKind::ManagedPreferences,
                location: under_root("/Library/Managed Preferences/tech.krate.policy.plist")
                    .display()
                    .to_string(),
            },
            file(under_root("/Library/Application Support/Krate/policy.toml")),
        ]
    }
    #[cfg(windows)]
    {
        let mut sources = Vec::new();
        // The registry cannot be put under a test folder, so a test that
        // re-roots the sources does not read it.
        if debug_env(TEST_POLICY_ROOT_ENV).is_none() {
            sources.push(Source {
                kind: SourceKind::Registry,
                location: format!(r"HKLM\{}", registry::KEY),
            });
        }
        sources.push(file(program_data().join("Krate").join("policy.toml")));
        sources
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        vec![file(under_root("/etc/krate/policy.toml"))]
    }
}

/// Is this source on this computer at all? A path Krate is not allowed to
/// look at is present: it cannot be read, which is not the same as absent.
fn present(source: &Source) -> bool {
    match source.kind {
        SourceKind::Registry => {
            #[cfg(windows)]
            {
                registry::present()
            }
            #[cfg(not(windows))]
            {
                false
            }
        }
        _ => match std::fs::symlink_metadata(&source.location) {
            Ok(_) => true,
            Err(err) => err.kind() != std::io::ErrorKind::NotFound,
        },
    }
}

fn load() -> State {
    if let Some(path) = debug_env(TEST_POLICY_ENV) {
        if path == "none" {
            return State::None;
        }
        let source = Source {
            kind: SourceKind::Test,
            location: path,
        };
        return read_source(source, Vec::new());
    }
    let mut found = candidates().into_iter().filter(present);
    let Some(winner) = found.next() else {
        return State::None;
    };
    read_source(winner, found.collect())
}

fn read_source(source: Source, ignored: Vec<Source>) -> State {
    match read_value(&source).and_then(|value| parse(&value, source.kind)) {
        Ok(policy) => State::InForce {
            policy: Box::new(policy),
            source,
            ignored,
        },
        Err(problem) => State::Unreadable {
            source,
            problem,
            ignored,
        },
    }
}

/// The source's contents as one tree, whatever format it is in.
fn read_value(source: &Source) -> std::result::Result<PValue, String> {
    match source.kind {
        SourceKind::Registry => {
            #[cfg(windows)]
            {
                registry::read()
            }
            #[cfg(not(windows))]
            {
                Err("the registry exists only on Windows".to_string())
            }
        }
        SourceKind::ManagedPreferences => read_plist(Path::new(&source.location)),
        SourceKind::File | SourceKind::Test => {
            if source.location.ends_with(".plist") {
                return read_plist(Path::new(&source.location));
            }
            let text = std::fs::read_to_string(&source.location)
                .map_err(|err| format!("it cannot be read ({err})"))?;
            let value: toml::Value =
                toml::from_str(&text).map_err(|err| format!("it is not valid TOML: {err}"))?;
            Ok(PValue::from_toml(&value))
        }
    }
}

/// A plist, binary or XML, read through the system's own `plutil`: a
/// configuration profile writes binary plists, and `plutil` is on every Mac.
fn read_plist(path: &Path) -> std::result::Result<PValue, String> {
    // Read first, so an unreadable file says so rather than what plutil says.
    std::fs::read(path).map_err(|err| format!("it cannot be read ({err})"))?;
    let output = std::process::Command::new("/usr/bin/plutil")
        .args(["-convert", "json", "-o", "-"])
        .arg(path)
        .output()
        .map_err(|err| format!("it could not be read with plutil ({err})"))?;
    if !output.status.success() {
        return Err(format!(
            "it is not a property list Krate can read: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let value: Value = serde_json::from_slice(&output.stdout)
        .map_err(|err| format!("it is not a property list Krate can read: {err}"))?;
    Ok(without_payload_keys(PValue::from_json(&value)))
}

/// A configuration profile install leaves `Payload*` keys beside the
/// domain's own (`PayloadUUID`, `PayloadVersion` ...). They are the
/// profile's, not the policy's, and are set aside before the strict read.
fn without_payload_keys(value: PValue) -> PValue {
    match value {
        PValue::Table(table) => PValue::Table(
            table
                .into_iter()
                .filter(|(key, _)| !key.starts_with("Payload"))
                .collect(),
        ),
        other => other,
    }
}

// ---------------------------------------------------------------- the tree

/// The three formats as one tree, so one strict parser reads them all.
#[derive(Debug, Clone)]
enum PValue {
    Bool(bool),
    Int(i64),
    Str(String),
    List(Vec<PValue>),
    Table(BTreeMap<String, PValue>),
    /// A value of a kind no key takes (a float, a date).
    Other(&'static str),
}

impl PValue {
    fn from_toml(value: &toml::Value) -> Self {
        match value {
            toml::Value::Boolean(b) => Self::Bool(*b),
            toml::Value::Integer(i) => Self::Int(*i),
            toml::Value::String(s) => Self::Str(s.clone()),
            toml::Value::Array(items) => Self::List(items.iter().map(Self::from_toml).collect()),
            toml::Value::Table(table) => Self::Table(
                table
                    .iter()
                    .map(|(key, value)| (key.clone(), Self::from_toml(value)))
                    .collect(),
            ),
            toml::Value::Float(_) => Self::Other("a number with a fraction"),
            toml::Value::Datetime(_) => Self::Other("a date"),
        }
    }

    fn from_json(value: &Value) -> Self {
        match value {
            Value::Bool(b) => Self::Bool(*b),
            Value::Number(n) => n.as_i64().map(Self::Int).unwrap_or(Self::Other("a number")),
            Value::String(s) => Self::Str(s.clone()),
            Value::Array(items) => Self::List(items.iter().map(Self::from_json).collect()),
            Value::Object(map) => Self::Table(
                map.iter()
                    .map(|(key, value)| (key.clone(), Self::from_json(value)))
                    .collect(),
            ),
            Value::Null => Self::Other("nothing"),
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::Bool(_) => "true or false",
            Self::Int(_) => "a number",
            Self::Str(_) => "text",
            Self::List(_) => "a list",
            Self::Table(_) => "a table",
            Self::Other(kind) => kind,
        }
    }
}

// ---------------------------------------------------------------- the policy

/// One signing root the organisation trusts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    pub name: String,
    /// Lowercase hex Ed25519 public key.
    pub key: String,
}

/// `[trust]`, with its defaults resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trust {
    pub roots: Vec<Root>,
    pub allow_unsigned: bool,
    pub allow_other_signed: bool,
}

/// `[updates] mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateMode {
    Auto,
    Notify,
    Off,
}

impl UpdateMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Notify => "notify",
            Self::Off => "off",
        }
    }
}

/// `[network] proxy`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Proxy {
    /// "direct": no proxy, whatever the environment says.
    Direct,
    /// An http proxy, as `http://[user:pass@]host:port`.
    Via(String),
}

/// A proxy a policy may name: "direct", `http://[user:pass@]host:port`, or
/// a bare `host:port` (read as http). Nothing else: no other scheme, no
/// path, a real port.
fn parse_proxy(text: &str) -> Option<Proxy> {
    let text = text.trim();
    if text == "direct" {
        return Some(Proxy::Direct);
    }
    let rest = match text.split_once("://") {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("http") => rest,
        Some(_) => return None,
        None => text,
    };
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    let (credentials, hostport) = match rest.rsplit_once('@') {
        Some((credentials, hostport)) => (Some(credentials), hostport),
        None => (None, rest),
    };
    if credentials.is_some_and(|c| c.is_empty() || c.contains(['/', '?', '#'])) {
        return None;
    }
    let (host, port) = hostport.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    let host_ok = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '[' | ']' | ':'));
    if port == 0 || !host_ok {
        return None;
    }
    Some(Proxy::Via(format!("http://{rest}")))
}

/// A proxy address with any password hidden, for words and `krate policy`.
fn redact_proxy(text: &str) -> String {
    match text.rsplit_once('@') {
        Some((before, after)) => {
            let (scheme, credentials) = before
                .split_once("://")
                .map(|(s, c)| (format!("{s}://"), c))
                .unwrap_or((String::new(), before));
            let user = credentials.split(':').next().unwrap_or_default();
            format!("{scheme}{user}:***@{after}")
        }
        None => text.to_string(),
    }
}

/// One `[apps."<app id>"]` table, or `[apps."*"]` for every
/// organisation's private app on this computer (G17): device-side data
/// rules. Each narrows; none allows anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AppRules {
    /// `clipboard_read = false`: the app cannot read the clipboard, and the
    /// person's own paste into it is refused too.
    pub no_clipboard_read: bool,
    /// `clipboard_write = false`: nothing leaves the app by the clipboard.
    pub no_clipboard_write: bool,
    /// `printing = false`: no printing, and no saving as PDF from the print
    /// dialog.
    pub no_printing: bool,
    /// `screen_capture = false`: the app's windows are kept out of
    /// screenshots, recordings and screen sharing where the system allows
    /// it (macOS, Windows), and Krate takes no picture of it.
    pub no_screen_capture: bool,
    /// `wipe_when_offline_ends = true`: when the app's offline access runs
    /// out, its team data on this computer is deleted.
    pub wipe_when_offline_ends: bool,
}

impl AppRules {
    fn merge(&mut self, other: &AppRules) {
        self.no_clipboard_read |= other.no_clipboard_read;
        self.no_clipboard_write |= other.no_clipboard_write;
        self.no_printing |= other.no_printing;
        self.no_screen_capture |= other.no_screen_capture;
        self.wipe_when_offline_ends |= other.wipe_when_offline_ends;
    }

    pub fn is_empty(&self) -> bool {
        *self == AppRules::default()
    }

    /// The rules as `krate policy` says them.
    pub fn words(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.no_clipboard_read {
            out.push("no reading the clipboard, pasting included");
        }
        if self.no_clipboard_write {
            out.push("no copying out by the clipboard");
        }
        if self.no_printing {
            out.push("no printing");
        }
        if self.no_screen_capture {
            out.push("kept out of screenshots and screen recordings");
        }
        if self.wipe_when_offline_ends {
            out.push("team data deleted from this computer when offline access ends");
        }
        out
    }

    fn json(&self) -> Value {
        json!({
            "clipboard_read": !self.no_clipboard_read,
            "clipboard_write": !self.no_clipboard_write,
            "printing": !self.no_printing,
            "screen_capture": !self.no_screen_capture,
            "wipe_when_offline_ends": self.wipe_when_offline_ends,
        })
    }
}

/// The app an `[apps]` rule is looked up for: its id, and whether it
/// carries an organisation's mark (a claim is enough: a rule only narrows).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppRef {
    pub id: Option<String>,
    pub org_app: bool,
}

/// A policy, version 1, every key understood.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub name: Option<String>,
    /// `None` when `[trust]` is absent: no trust limit at all.
    pub trust: Option<Trust>,
    pub deny: Vec<DenyRule>,
    /// `[network] allow`, normalised; `None` is no ceiling.
    pub network: Option<Vec<String>>,
    /// `[network] proxy`: how every connection leaves this computer. `None`
    /// is "the environment's proxy variables apply".
    pub proxy: Option<Proxy>,
    /// `[network] no_proxy`, in NO_PROXY form; meaningful only with `proxy`.
    pub no_proxy: Option<String>,
    pub counting: bool,
    /// The pinned hub's origin.
    pub hub: Option<String>,
    pub ai: bool,
    pub providers: Option<Vec<String>>,
    pub updates: UpdateMode,
    pub audit_path: Option<PathBuf>,
    pub interpreter: bool,
    /// `[org] slug`: the organisation this computer belongs to. Its apps are
    /// listed first, and a person who is not signed in is asked to sign in
    /// with its company sign-in (IC-953). It narrows nothing.
    pub org: Option<String>,
    /// `[apps]`: device-side data rules by app id, `"*"` for every
    /// organisation's private app (G17).
    pub apps: BTreeMap<String, AppRules>,
}

impl Policy {
    /// The `[apps]` rules that hold for one app: `"*"` for an organisation's
    /// app, and its own id's table, together.
    pub fn rules_for(&self, app: &AppRef) -> AppRules {
        let mut rules = AppRules::default();
        if app.org_app {
            if let Some(all) = self.apps.get("*") {
                rules.merge(all);
            }
        }
        if let Some(own) = app
            .id
            .as_deref()
            .and_then(|id| self.apps.get(&id.to_ascii_lowercase()))
        {
            rules.merge(own);
        }
        rules
    }
}

/// Reads one tree strictly: every table names only keys Krate knows.
struct Reader {
    /// The registry has no booleans and no lists of its own; DWORD 0/1 and
    /// a key of values stand in for them.
    registry: bool,
}

type Fields<'a> = &'a BTreeMap<String, PValue>;

impl Reader {
    fn table<'v>(&self, value: &'v PValue, at: &str) -> std::result::Result<Fields<'v>, String> {
        match value {
            PValue::Table(table) => Ok(table),
            other => Err(format!(
                "`{at}` should be a table, and it is {}",
                other.kind()
            )),
        }
    }

    fn only(&self, table: Fields, at: &str, known: &[&str]) -> std::result::Result<(), String> {
        for key in table.keys() {
            if !known.contains(&key.as_str()) {
                let path = if at.is_empty() {
                    key.clone()
                } else {
                    format!("{at}.{key}")
                };
                return Err(format!("`{path}` is not a setting Krate knows"));
            }
        }
        Ok(())
    }

    fn bool(&self, value: &PValue, at: &str) -> std::result::Result<bool, String> {
        match value {
            PValue::Bool(b) => Ok(*b),
            PValue::Int(0) if self.registry => Ok(false),
            PValue::Int(1) if self.registry => Ok(true),
            other => Err(format!(
                "`{at}` should be true or false, and it is {}",
                other.kind()
            )),
        }
    }

    fn string(&self, value: &PValue, at: &str) -> std::result::Result<String, String> {
        match value {
            PValue::Str(s) => Ok(s.clone()),
            other => Err(format!("`{at}` should be text, and it is {}", other.kind())),
        }
    }

    fn strings(&self, value: &PValue, at: &str) -> std::result::Result<Vec<String>, String> {
        let items: Vec<&PValue> = match value {
            PValue::List(items) => items.iter().collect(),
            other => {
                return Err(format!(
                    "`{at}` should be a list, and it is {}",
                    other.kind()
                ))
            }
        };
        items
            .into_iter()
            .map(|item| match item {
                PValue::Str(s) => Ok(s.clone()),
                other => Err(format!(
                    "every entry in `{at}` should be text, and one is {}",
                    other.kind()
                )),
            })
            .collect()
    }
}

fn hex_key(text: &str) -> Option<String> {
    let text = text.trim();
    (text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| text.to_ascii_lowercase())
}

/// A hub address a policy may pin: https, or http only to this machine.
fn hub_origin(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches('/');
    let origin = crate::org::origin(trimmed)?;
    if origin != trimmed.to_ascii_lowercase() {
        return None;
    }
    let (scheme, host) = origin.split_once("://")?;
    let bare = host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host);
    match scheme {
        "https" => Some(origin),
        "http" if matches!(bare, "localhost" | "127.0.0.1" | "[::1]") => Some(origin),
        _ => None,
    }
}

/// Parse a whole policy. The error names the key and what is wrong with it.
fn parse(value: &PValue, kind: SourceKind) -> std::result::Result<Policy, String> {
    let r = Reader {
        registry: kind == SourceKind::Registry,
    };
    let top = r.table(value, "the policy")?;
    r.only(
        top,
        "",
        &[
            "version",
            "name",
            "trust",
            "capabilities",
            "network",
            "usage",
            "hub",
            "studio",
            "updates",
            "audit",
            "runtime",
            "org",
            "apps",
        ],
    )?;
    match top.get("version") {
        None => return Err("`version` is missing, and it must be 1".to_string()),
        Some(PValue::Int(1)) => {}
        Some(PValue::Int(other)) => {
            return Err(format!(
                "`version` is {other}, and this Krate reads version 1 only"
            ))
        }
        Some(other) => {
            return Err(format!(
                "`version` should be the number 1, and it is {}",
                other.kind()
            ))
        }
    }
    let name = top
        .get("name")
        .map(|v| r.string(v, "name"))
        .transpose()?
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty());

    let trust = match top.get("trust") {
        None => None,
        Some(value) => {
            let t = r.table(value, "trust")?;
            r.only(
                t,
                "trust",
                &["roots", "allow_unsigned", "allow_other_signed"],
            )?;
            let mut roots = Vec::new();
            match t.get("roots") {
                None => {}
                // The registry's shape, as an ADMX list with explicit
                // names writes it: one value per root, name = key.
                Some(PValue::Table(table)) if r.registry => {
                    for (root_name, key) in table {
                        let key = r.string(key, &format!("trust.roots.{root_name}"))?;
                        roots.push(Root {
                            name: root_name.clone(),
                            key: hex_key(&key).ok_or_else(|| {
                                format!(
                                    "`trust.roots.{root_name}` should be 64 hex characters (an \
                                     Ed25519 public key)"
                                )
                            })?,
                        });
                    }
                }
                Some(PValue::List(items)) => {
                    for (index, item) in items.iter().enumerate() {
                        let at = format!("trust.roots[{index}]");
                        let entry = r.table(item, &at)?;
                        r.only(entry, &at, &["name", "key"])?;
                        let root_name = entry
                            .get("name")
                            .map(|v| r.string(v, &format!("{at}.name")))
                            .transpose()?
                            .unwrap_or_default();
                        let key = entry
                            .get("key")
                            .ok_or_else(|| format!("`{at}.key` is missing"))
                            .and_then(|v| r.string(v, &format!("{at}.key")))?;
                        roots.push(Root {
                            name: root_name,
                            key: hex_key(&key).ok_or_else(|| {
                                format!(
                                    "`{at}.key` should be 64 hex characters (an Ed25519 public \
                                     key)"
                                )
                            })?,
                        });
                    }
                }
                Some(other) => {
                    return Err(format!(
                        "`trust.roots` should be a list, and it is {}",
                        other.kind()
                    ))
                }
            }
            let has_roots = !roots.is_empty();
            let allow_unsigned = t
                .get("allow_unsigned")
                .map(|v| r.bool(v, "trust.allow_unsigned"))
                .transpose()?
                .unwrap_or(!has_roots);
            let allow_other_signed = t
                .get("allow_other_signed")
                .map(|v| r.bool(v, "trust.allow_other_signed"))
                .transpose()?
                .unwrap_or(!has_roots);
            Some(Trust {
                roots,
                allow_unsigned,
                allow_other_signed,
            })
        }
    };

    let mut deny = Vec::new();
    if let Some(value) = top.get("capabilities") {
        let t = r.table(value, "capabilities")?;
        r.only(t, "capabilities", &["deny"])?;
        if let Some(list) = t.get("deny") {
            for entry in r.strings(list, "capabilities.deny")? {
                deny.push(
                    DenyRule::parse(&entry).map_err(|why| format!("`capabilities.deny`: {why}"))?,
                );
            }
        }
    }

    let mut proxy = None;
    let mut no_proxy = None;
    let network = match top.get("network") {
        None => None,
        Some(value) => {
            let t = r.table(value, "network")?;
            r.only(t, "network", &["allow", "proxy", "no_proxy"])?;
            if let Some(v) = t.get("proxy") {
                let text = r.string(v, "network.proxy")?;
                proxy = Some(parse_proxy(&text).ok_or_else(|| {
                    format!(
                        "`network.proxy` is \"{}\", and it should be \"direct\" or a proxy \
                         address like http://proxy.acme.com:3128",
                        redact_proxy(&text)
                    )
                })?);
            }
            if let Some(v) = t.get("no_proxy") {
                no_proxy = Some(r.string(v, "network.no_proxy")?.trim().to_string())
                    .filter(|n| !n.is_empty());
            }
            match t.get("allow") {
                None => None,
                Some(list) => Some(
                    r.strings(list, "network.allow")?
                        .iter()
                        .map(|entry| {
                            OrgCeiling::parse_network(entry)
                                .map_err(|why| format!("`network.allow`: {why}"))
                        })
                        .collect::<std::result::Result<Vec<_>, _>>()?,
                ),
            }
        }
    };

    let mut counting = true;
    if let Some(value) = top.get("usage") {
        let t = r.table(value, "usage")?;
        r.only(t, "usage", &["counting"])?;
        if let Some(v) = t.get("counting") {
            counting = r.bool(v, "usage.counting")?;
        }
    }

    let mut hub = None;
    if let Some(value) = top.get("hub") {
        let t = r.table(value, "hub")?;
        r.only(t, "hub", &["url"])?;
        if let Some(v) = t.get("url") {
            let url = r.string(v, "hub.url")?;
            hub = Some(hub_origin(&url).ok_or_else(|| {
                format!(
                    "`hub.url` is \"{url}\", and it should be a hub's https address with no \
                     path, like https://hub.krate.tech"
                )
            })?);
        }
    }

    let mut ai = true;
    let mut providers = None;
    if let Some(value) = top.get("studio") {
        let t = r.table(value, "studio")?;
        r.only(t, "studio", &["ai", "providers"])?;
        if let Some(v) = t.get("ai") {
            ai = r.bool(v, "studio.ai")?;
        }
        if let Some(v) = t.get("providers") {
            let known = known_providers();
            let mut ids = Vec::new();
            for id in r.strings(v, "studio.providers")? {
                let id = id.trim().to_ascii_lowercase();
                if !known.contains(&id.as_str()) {
                    return Err(format!(
                        "`studio.providers` names \"{id}\", which is not a provider Krate knows \
                         ({})",
                        known.join(", ")
                    ));
                }
                ids.push(id);
            }
            providers = Some(ids);
        }
    }

    let mut updates = UpdateMode::Auto;
    if let Some(value) = top.get("updates") {
        let t = r.table(value, "updates")?;
        r.only(t, "updates", &["mode"])?;
        if let Some(v) = t.get("mode") {
            updates = match r.string(v, "updates.mode")?.as_str() {
                "auto" => UpdateMode::Auto,
                "notify" => UpdateMode::Notify,
                "off" => UpdateMode::Off,
                other => {
                    return Err(format!(
                        "`updates.mode` is \"{other}\", and it should be auto, notify or off"
                    ))
                }
            };
        }
    }

    let mut audit_path = None;
    if let Some(value) = top.get("audit") {
        let t = r.table(value, "audit")?;
        r.only(t, "audit", &["path"])?;
        if let Some(v) = t.get("path") {
            let path = PathBuf::from(r.string(v, "audit.path")?);
            if !path.is_absolute() {
                return Err(format!(
                    "`audit.path` is \"{}\", and it should be a full path",
                    path.display()
                ));
            }
            audit_path = Some(path);
        }
    }

    let mut interpreter = false;
    if let Some(value) = top.get("runtime") {
        let t = r.table(value, "runtime")?;
        r.only(t, "runtime", &["interpreter"])?;
        if let Some(v) = t.get("interpreter") {
            interpreter = r.bool(v, "runtime.interpreter")?;
        }
    }

    let mut org = None;
    if let Some(value) = top.get("org") {
        let t = r.table(value, "org")?;
        r.only(t, "org", &["slug"])?;
        if let Some(v) = t.get("slug") {
            let slug = r.string(v, "org.slug")?.trim().to_ascii_lowercase();
            if !org_slug(&slug) {
                return Err(format!(
                    "`org.slug` is \"{slug}\", and it should be an organisation's short name: 2 \
                     to 39 lower-case letters, digits and dashes, starting with a letter, like \
                     acme"
                ));
            }
            org = Some(slug);
        }
    }

    let mut apps = BTreeMap::new();
    if let Some(value) = top.get("apps") {
        let t = r.table(value, "apps")?;
        for (id, rules) in t {
            let at = format!("apps.{id}");
            let id = id.trim().to_ascii_lowercase();
            if id != "*" && !app_id_shape(&id) {
                return Err(format!(
                    "`{at}` should be named by an app's id, like dev.acme.payroll, or \"*\" for \
                     every organisation app"
                ));
            }
            let fields = r.table(rules, &at)?;
            r.only(
                fields,
                &at,
                &[
                    "clipboard_read",
                    "clipboard_write",
                    "printing",
                    "screen_capture",
                    "wipe_when_offline_ends",
                ],
            )?;
            let flag = |key: &str| -> std::result::Result<Option<bool>, String> {
                fields
                    .get(key)
                    .map(|v| r.bool(v, &format!("{at}.{key}")))
                    .transpose()
            };
            let rule = AppRules {
                no_clipboard_read: flag("clipboard_read")? == Some(false),
                no_clipboard_write: flag("clipboard_write")? == Some(false),
                no_printing: flag("printing")? == Some(false),
                no_screen_capture: flag("screen_capture")? == Some(false),
                wipe_when_offline_ends: flag("wipe_when_offline_ends")? == Some(true),
            };
            apps.entry(id)
                .or_insert_with(AppRules::default)
                .merge(&rule);
        }
    }

    Ok(Policy {
        name,
        trust,
        deny,
        network,
        proxy,
        no_proxy,
        counting,
        hub,
        ai,
        providers,
        updates,
        audit_path,
        interpreter,
        org,
        apps,
    })
}

/// An app id as a manifest spells one: two or more dot-separated parts of
/// lower-case letters, digits, dashes and underscores.
fn app_id_shape(id: &str) -> bool {
    let parts: Vec<&str> = id.split('.').collect();
    parts.len() >= 2
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.bytes().all(|b| {
                    b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_')
                })
        })
}

/// An organisation's short name, as the hub allows it (org.js SLUG).
fn org_slug(slug: &str) -> bool {
    let bytes = slug.as_bytes();
    (2..=39).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

/// The organisation this computer's policy names (`[org] slug`): its apps
/// are listed first, and its company sign-in is the one offered (IC-953).
pub fn policy_org() -> Option<&'static str> {
    policy().and_then(|p| p.org.as_deref())
}

// ---------------------------------------------------------------- words

/// "your organisation (Acme Labs)", or "your organisation".
pub fn who(policy: Option<&Policy>) -> String {
    match policy.and_then(|p| p.name.as_deref()) {
        Some(name) => format!("your organisation ({name})"),
        None => "your organisation".to_string(),
    }
}

/// Who to ask, at the end of every refusal.
fn ask(policy: Option<&Policy>) -> String {
    match policy.and_then(|p| p.name.as_deref()) {
        Some(name) => format!("Ask {name}'s IT team"),
        None => "Ask your IT team".to_string(),
    }
}

fn short(key: &str) -> String {
    key.chars().take(16).collect()
}

/// The words for a policy that cannot be used.
fn unreadable_words(source: &Source, problem: &str) -> String {
    format!(
        "Krate opens no apps on this computer until your organisation's policy is fixed: \
         {problem} (in {}). Ask your IT team",
        source.location
    )
}

/// A refusal for the policy being unreadable, when it is.
fn unreadable_refusal() -> Option<Refused> {
    match state() {
        State::Unreadable {
            source, problem, ..
        } => Some(Refused::new(
            "policy-unreadable",
            unreadable_words(source, problem),
        )),
        _ => None,
    }
}

// ---------------------------------------------------------------- trust

/// What an app's signature proves, for the trust check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SigFacts {
    Unsigned,
    /// Genuinely signed over its current contents, chain holding.
    Holds {
        signer: String,
        root: String,
    },
    /// A signature is there and does not hold: damaged, changed after
    /// signing, a bad key, or a chain that does not hold.
    Broken,
}

impl SigFacts {
    /// Read from a bundle; a loose component is unsigned.
    pub fn of(bundle: Option<&krate_bundle::OpenBundle>) -> Self {
        let Some(bundle) = bundle else {
            return Self::Unsigned;
        };
        match bundle.signature_state() {
            Ok(krate_bundle::SignatureState::Absent) => Self::Unsigned,
            Ok(krate_bundle::SignatureState::Damaged { .. }) | Err(_) => Self::Broken,
            Ok(krate_bundle::SignatureState::Signed(envelope)) => {
                let revocations = crate::revocations_known_here(
                    bundle.signing_authority().ok().flatten().as_deref(),
                );
                match bundle.full_verdict(&revocations) {
                    Ok(Some(verdict)) if verdict.is_trustworthy() => Self::Holds {
                        signer: envelope.public_key.to_ascii_lowercase(),
                        root: envelope
                            .delegation
                            .as_ref()
                            .map(|d| d.delegation.root.clone())
                            .unwrap_or_else(|| envelope.public_key.clone())
                            .to_ascii_lowercase(),
                    },
                    _ => Self::Broken,
                }
            }
        }
    }

    fn signer(&self) -> Option<&str> {
        match self {
            Self::Holds { signer, .. } => Some(signer),
            _ => None,
        }
    }

    fn root(&self) -> Option<&str> {
        match self {
            Self::Holds { root, .. } => Some(root),
            _ => None,
        }
    }
}

/// Does `[trust]` let this app open? `None` when it passes (P3).
pub fn trust_refusal(policy: &Policy, facts: &SigFacts) -> Option<Refused> {
    let trust = policy.trust.as_ref()?;
    let who = who(Some(policy));
    let ask = ask(Some(policy));
    match facts {
        // Never passes, whatever the flags: it is not unsigned, it is
        // tampered.
        SigFacts::Broken => Some(Refused::new(
            "policy-tampered",
            format!(
                "this app did not open: {who} allows only apps whose signature holds, and this \
                 app's signature does not -- it was changed after it was signed, or the copy is \
                 damaged. Get a fresh copy from whoever sent it. {ask} if it should be allowed"
            ),
        )),
        SigFacts::Unsigned if trust.allow_unsigned => None,
        SigFacts::Unsigned => Some(Refused::new(
            "policy-unsigned",
            format!(
                "this app did not open: {who} allows only signed apps on this computer, and \
                 this app is not signed. {ask} for a signed copy, or for this app to be allowed"
            ),
        )),
        SigFacts::Holds { root, .. } => {
            if trust.roots.is_empty() || trust.roots.iter().any(|r| r.key == *root) {
                return None;
            }
            if trust.allow_other_signed {
                return None;
            }
            let trusted = trust
                .roots
                .iter()
                .map(|r| {
                    if r.name.is_empty() {
                        short(&r.key)
                    } else {
                        r.name.clone()
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            Some(Refused::new(
                "policy-other-signer",
                format!(
                    "this app did not open: {who} allows only apps signed by {trusted}, and this \
                     app is signed by someone else (key {}). {ask} if it should be allowed",
                    short(root)
                ),
            ))
        }
    }
}

/// The ceiling this computer's policy puts on every run of `app`: the
/// `[capabilities]` and `[network]` limits, and the app's `[apps]` rules as
/// exact capability rules the runtime enforces at the call (G17).
pub fn ceiling_for(app: &AppRef) -> OrgCeiling {
    let Some(policy) = policy() else {
        return OrgCeiling::default();
    };
    let mut deny = policy.deny.clone();
    let rules = policy.rules_for(app);
    for (blocked, cap) in [
        (rules.no_clipboard_read, "read"),
        (rules.no_clipboard_write, "write"),
    ] {
        if blocked {
            let cap = krate_manifest::Capability::new("ui", "clipboard", Some(cap))
                .expect("ui.clipboard:read and :write are Krate capabilities");
            deny.push(DenyRule::exact(&cap));
        }
    }
    let mut calls = Vec::new();
    if rules.no_printing {
        calls.push(PRINT_CALL.to_string());
    }
    if rules.no_screen_capture {
        calls.push(SCREEN_CAPTURE_CALL.to_string());
    }
    OrgCeiling {
        deny,
        network: policy.network.clone(),
        calls,
    }
}

/// Put the organisation's ceiling for `app` on a policy this run built.
pub fn narrow(policy: SessionPolicy, app: &AppRef) -> SessionPolicy {
    let ceiling = ceiling_for(app);
    if ceiling.is_empty() {
        return policy;
    }
    policy.with_organisation(ceiling)
}

/// The words for one blocked capability.
fn blocked_words(
    policy: Option<&Policy>,
    cap: &krate_manifest::Capability,
    how: OrgBlock,
) -> Refused {
    let who = who(policy);
    let ask = ask(policy);
    match how {
        OrgBlock::Denied => Refused::new(
            "policy-capability",
            format!(
                "this app did not open: it needs to {} ({cap}), which {who} blocks on this \
                 computer. {ask} if it should be allowed",
                crate::human_label(cap)
            ),
        ),
        OrgBlock::OutsideNetwork => Refused::new(
            "policy-network",
            format!(
                "this app did not open: it needs to connect to {}, which is outside the places \
                 {who} lets apps reach from this computer. {ask} if it should be allowed",
                cap.resource().unwrap_or("the network")
            ),
        ),
    }
}

/// A required capability the organisation blocks, as the refusal a person
/// sees. Optional ones are shown as blocked in the consent prompt instead.
pub fn capability_refusal(manifest: &krate_manifest::Manifest, app: &AppRef) -> Option<Refused> {
    let ceiling = ceiling_for(app);
    if ceiling.is_empty() {
        return None;
    }
    let required = manifest.required_capabilities().ok()?;
    required.iter().find_map(|cap| {
        ceiling
            .blocks(cap)
            .map(|how| blocked_words(policy(), cap, how))
    })
}

/// "blocked by Acme Labs", for a consent row.
pub fn blocked_label() -> String {
    match policy().and_then(|p| p.name.as_deref()) {
        Some(name) => format!("blocked by {name}"),
        None => "blocked by your organisation".to_string(),
    }
}

// ---------------------------------------------------------------- the doors

/// What is being opened, for the gates and the open record.
pub struct Subject<'a> {
    pub bundle: Option<&'a krate_bundle::OpenBundle>,
    /// A loose component's manifest, when there is no bundle.
    pub manifest: Option<&'a krate_manifest::Manifest>,
    /// The file on disk the person named, hashed for the record.
    pub file: Option<&'a Path>,
    /// The sha256 already known, for a download.
    pub sha256: Option<&'a str>,
    /// What the signature proves, worked out once: it hashes every entry.
    facts: std::cell::OnceCell<SigFacts>,
}

impl<'a> Subject<'a> {
    pub fn new(
        bundle: Option<&'a krate_bundle::OpenBundle>,
        file: Option<&'a Path>,
        sha256: Option<&'a str>,
    ) -> Self {
        Self {
            bundle,
            manifest: None,
            file,
            sha256,
            facts: std::cell::OnceCell::new(),
        }
    }

    /// A loose component's manifest, once it is known.
    pub fn with_manifest(self, manifest: Option<&'a krate_manifest::Manifest>) -> Self {
        Self {
            manifest: manifest.filter(|_| self.bundle.is_none()),
            ..self
        }
    }

    fn manifest(&self) -> Option<&krate_manifest::Manifest> {
        self.bundle.map(|b| b.manifest()).or(self.manifest)
    }

    /// Who this app is for the `[apps]` rules: its id, and whether it
    /// carries an organisation's mark.
    pub fn app_ref(&self) -> AppRef {
        AppRef {
            id: self.manifest().map(|m| m.app.id.clone()),
            org_app: self
                .bundle
                .is_some_and(|b| b.org_mark().ok().flatten().is_some()),
        }
    }

    fn facts(&self) -> &SigFacts {
        self.facts.get_or_init(|| SigFacts::of(self.bundle))
    }
}

/// The machine policy's own gate: an unreadable policy, then `[trust]`.
/// Every door calls this first; a refusal is shown at the door and leaves a
/// `refused` record. With no policy it does nothing at all.
pub fn admit(subject: &Subject, door: RecordDoor) -> Result<()> {
    if matches!(state(), State::None) {
        return Ok(());
    }
    let refusal =
        unreadable_refusal().or_else(|| policy().and_then(|p| trust_refusal(p, subject.facts())));
    if let Some(refused) = refusal {
        return refuse(subject, door, refused);
    }
    Ok(())
}

/// The capability ceiling at the door: a required capability the
/// organisation blocks refuses the open, with a record.
pub fn admit_capabilities(subject: &Subject, door: RecordDoor) -> Result<()> {
    let Some(manifest) = subject.manifest() else {
        return Ok(());
    };
    if let Some(refused) = capability_refusal(manifest, &subject.app_ref()) {
        return refuse(subject, door, refused);
    }
    Ok(())
}

/// Show a refusal at the door and record it.
fn refuse(subject: &Subject, door: RecordDoor, refused: Refused) -> Result<()> {
    crate::access::alert_refusal(door.access(), &refused.words);
    audit::refused(&record_base(subject, door), &refused);
    Err(anyhow::Error::new(refused))
}

/// Run the organisation check in `access` and record what it refused.
pub fn org_gate(
    subject: &Subject,
    bytes: crate::access::Bytes,
    mark: crate::access::Mark,
    door: RecordDoor,
) -> Result<()> {
    org_gate_opened(subject, bytes, mark, door).map(|_| ())
}

/// [`org_gate`], answering what is known about the private app it let
/// through, for the app's team data (IC-951).
pub fn org_gate_opened(
    subject: &Subject,
    bytes: crate::access::Bytes,
    mark: crate::access::Mark,
    door: RecordDoor,
) -> Result<Option<crate::access::Opened>> {
    let result = crate::access::gate_opened(bytes, mark, door.access());
    note_refusal(subject, door, &result);
    // Offline access ran out: where the organisation says so for this app,
    // its team data leaves this computer now (G17).
    if let Err(err) = &result {
        let expired = err
            .downcast_ref::<Refused>()
            .is_some_and(|r| r.reason == "offline-expired");
        let app = subject.app_ref();
        let wipe = policy().is_some_and(|p| p.rules_for(&app).wipe_when_offline_ends);
        let slug = subject.bundle.and_then(|b| b.org_mark().ok().flatten());
        if let (true, true, Some(slug), Some(id)) = (expired, wipe, slug, app.id.as_deref()) {
            let gone = crate::access::wipe_team_data(&slug, id);
            let mut record = record_base(subject, door);
            record["event"] = json!("team-data-wiped");
            record["files"] = json!(gone);
            audit::write(record);
            eprintln!(
                "{}'s team data for this app was deleted from this computer, as {} asks when its \
                 offline access ends",
                slug,
                who(policy())
            );
        }
    }
    result
}

/// Record a refusal some other check made, if `result` is one.
pub fn note_refusal<T>(subject: &Subject, door: RecordDoor, result: &Result<T>) {
    if let Err(err) = result {
        if let Some(refused) = err.downcast_ref::<Refused>() {
            audit::refused(&record_base(subject, door), refused);
        }
    }
}

/// Record a refusal by its code and words, for the checks that print their
/// own refusal and return an exit code rather than an error.
pub fn note_refused(subject: &Subject, door: RecordDoor, reason: &str, words: &str) {
    audit::refused(
        &record_base(subject, door),
        &Refused::new(reason, words.to_string()),
    );
}

/// The app is about to run: write the `opened` record with what it was
/// granted and what it was not.
pub fn opened(subject: &Subject, door: RecordDoor, session: &SessionPolicy) {
    let mut record = record_base(subject, door);
    let declared = subject
        .manifest()
        .and_then(|m| m.declared_capabilities().ok())
        .unwrap_or_default();
    let granted: Vec<String> = declared
        .iter()
        .filter(|cap| session.allows(cap) && !cap.is_default_granted())
        .map(ToString::to_string)
        .collect();
    let denied: Vec<String> = declared
        .iter()
        .filter(|cap| !session.allows(cap))
        .map(ToString::to_string)
        .collect();
    record["event"] = json!("opened");
    record["granted"] = json!(granted);
    record["denied"] = json!(denied);
    audit::write(record);
}

/// The fields every record about this app carries.
fn record_base(subject: &Subject, door: RecordDoor) -> Value {
    let manifest = subject.manifest();
    let facts = subject.facts();
    let digest = subject
        .sha256
        .map(str::to_string)
        .or_else(|| subject.file.and_then(audit::sha256_of_file));
    json!({
        "app": manifest.map(|m| m.app.id.clone()),
        "name": manifest.map(|m| m.app.name.clone()),
        "version": manifest.map(|m| m.app.version.clone()),
        "digest": digest.map(|d| format!("sha256:{d}")),
        "signer": facts.signer(),
        "root": facts.root(),
        "org": subject.bundle.and_then(|b| b.org_mark().ok().flatten()),
        "door": door.as_str(),
        "granted": [],
        "denied": [],
    })
}

/// Would this app open here, as far as this computer's policy decides?
/// `krate policy check`: nothing is recorded and nothing runs.
pub fn check(
    bundle: Option<&krate_bundle::OpenBundle>,
    manifest: Option<&krate_manifest::Manifest>,
) -> Option<Refused> {
    if let Some(refused) = unreadable_refusal() {
        return Some(refused);
    }
    let policy = policy()?;
    if let Some(refused) = trust_refusal(policy, &SigFacts::of(bundle)) {
        return Some(refused);
    }
    let app = Subject::new(bundle, None, None)
        .with_manifest(manifest)
        .app_ref();
    bundle
        .map(|b| b.manifest())
        .or(manifest)
        .and_then(|m| capability_refusal(m, &app))
}

// ---------------------------------------------------------------- hub

/// The hub this machine talks to: `--hub`, else `KRATE_HUB_URL`, else the
/// public hub -- and under `[hub] url`, only that hub. A different one asked
/// for is refused in the organisation's words, never silently swapped.
///
/// Every hub base URL the CLI resolves comes through here (IC-949 P4).
pub fn hub(hub_override: Option<&str>) -> Result<String> {
    if let Some(refused) = unreadable_refusal() {
        return Err(anyhow::Error::new(refused));
    }
    let asked = hub_override.map(|h| (h.to_string(), "--hub")).or_else(|| {
        std::env::var("KRATE_HUB_URL")
            .ok()
            .filter(|h| !h.trim().is_empty())
            .map(|h| (h, "KRATE_HUB_URL"))
    });
    if let Some(pinned) = policy().and_then(|p| p.hub.clone()) {
        if let Some((url, from)) = &asked {
            let same = crate::org::origin(url.trim().trim_end_matches('/')).as_deref()
                == Some(pinned.as_str());
            if !same {
                return Err(anyhow::Error::new(Refused::new(
                    "policy-hub",
                    format!(
                        "{} lets this computer use only the hub at {pinned}, and {from} asks \
                         for {url}, so nothing was sent. {}, or use {pinned}",
                        capitalise(&who(policy())),
                        ask(policy())
                    ),
                )));
            }
        }
        return Ok(pinned);
    }
    Ok(asked
        .map(|(url, _)| url)
        .unwrap_or_else(|| crate::DEFAULT_HUB_URL.to_string())
        .trim_end_matches('/')
        .to_string())
}

/// The pinned hub's origin, when a policy pins one.
pub fn pinned_hub() -> Option<&'static str> {
    policy().and_then(|p| p.hub.as_deref())
}

fn capitalise(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

// ---------------------------------------------------------------- studio, usage, updates

/// May an AI build start with this provider? Asked before any provider is
/// contacted (`krate create --agent`, `plan`, `revise`, `port`, the
/// authoring agent). `provider` is the provider id, `None` while it is not
/// yet known, or `Some("author-cmd")` for a command of the person's own.
pub fn ai_build(provider: Option<&str>) -> Result<()> {
    if let Some(refused) = unreadable_refusal() {
        return Err(anyhow::Error::new(refused));
    }
    let Some(policy) = policy() else {
        return Ok(());
    };
    if !policy.ai {
        anyhow::bail!(
            "{} has turned off building apps with AI on this computer, so nothing was sent to \
             any AI. {} if you need it",
            capitalise(&who(Some(policy))),
            ask(Some(policy))
        );
    }
    if let (Some(allowed), Some(provider)) = (&policy.providers, provider) {
        if !allowed.iter().any(|id| id == provider) {
            anyhow::bail!(
                "{} allows building with {} only on this computer, so {provider} was not \
                 started and nothing was sent to it. {} if you need it",
                capitalise(&who(Some(policy))),
                allowed.join(", "),
                ask(Some(policy))
            );
        }
    }
    Ok(())
}

/// May usage events be counted and sent? An unreadable policy says no.
pub fn counting_allowed() -> bool {
    match state() {
        State::None => true,
        State::InForce { policy, .. } => policy.counting,
        State::Unreadable { .. } => false,
    }
}

/// How updates are handled here. An unreadable policy turns them off.
pub fn updates_mode() -> UpdateMode {
    match state() {
        State::None => UpdateMode::Auto,
        State::InForce { policy, .. } => policy.updates,
        State::Unreadable { .. } => UpdateMode::Off,
    }
}

/// `[network] proxy`, installed once at process start, before any
/// connection: the organisation decides how traffic leaves this computer,
/// and "direct" means no proxy whatever the environment says. With no proxy
/// key the environment's proxy variables apply, as they always have.
pub fn install_proxy() {
    let Some(policy) = policy() else {
        return;
    };
    let Some(proxy) = &policy.proxy else {
        return;
    };
    krate_http::set_policy_proxy(policy_proxy(proxy, policy.no_proxy.as_deref()));
}

fn policy_proxy(proxy: &Proxy, no_proxy: Option<&str>) -> krate_http::PolicyProxy {
    krate_http::PolicyProxy {
        proxy: match proxy {
            Proxy::Direct => None,
            Proxy::Via(url) => Some(url.clone()),
        },
        no_proxy: no_proxy.map(str::to_string),
    }
}

/// Does this computer run every app on the interpreter?
pub fn interpreter() -> bool {
    policy().is_some_and(|p| p.interpreter)
}

// ---------------------------------------------------------------- krate policy

/// `krate policy`, `krate policy --json`.
pub fn show(json_out: bool) -> Result<u8> {
    let state = state();
    let record = audit::location();
    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&describe(state, &record))?
        );
    } else {
        print!("{}", words_for(state, &record));
    }
    Ok(match state {
        State::Unreadable { .. } => 1,
        _ => 0,
    })
}

/// `krate policy check <app>`.
pub fn check_command(target: &Path, json_out: bool) -> Result<u8> {
    let (bundle, manifest) = if krate_bundle::is_bundle_path(target) {
        let bundle = krate_bundle::open(target)
            .map_err(|err| anyhow::anyhow!("could not open {}: {err}", target.display()))?;
        (Some(bundle), None)
    } else {
        if !target.exists() {
            anyhow::bail!("{} does not exist", target.display());
        }
        (None, None)
    };
    let verdict = check(bundle.as_ref(), manifest);
    // The root an IT team would add to let this publisher's apps in: the
    // short key in the refusal is for a person, a policy needs all of it.
    let root = match SigFacts::of(bundle.as_ref()) {
        SigFacts::Holds { root, .. } => Some(root),
        _ => None,
    };
    let name = bundle
        .as_ref()
        .map(|b| b.manifest().app.name.clone())
        .unwrap_or_else(|| target.display().to_string());
    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "v": 1,
                "app": name,
                "opens": verdict.is_none(),
                "reason": verdict.as_ref().map(|r| r.reason.clone()),
                "words": verdict.as_ref().map(|r| r.words.clone()),
                "root": root,
            }))?
        );
    } else {
        match &verdict {
            None if matches!(state(), State::None) => {
                println!(
                    "{name} would open here: there is no organisation policy on this computer."
                )
            }
            None => println!("{name} would open here: it fits your organisation's policy."),
            Some(refused) => {
                println!("{name} would not open here. A person would see:");
                println!();
                println!("  {}", refused.words);
                if let (Some(root), "policy-other-signer") = (&root, refused.reason.as_str()) {
                    println!();
                    println!("To allow apps from this publisher, add its root to [trust] roots:");
                    println!();
                    println!("  {{ name = \"<publisher>\", key = \"{root}\" }}");
                }
            }
        }
    }
    Ok(if verdict.is_none() { 0 } else { 1 })
}

fn policy_json(policy: &Policy) -> Value {
    json!({
        "version": 1,
        "name": policy.name,
        "trust": policy.trust.as_ref().map(|t| json!({
            "roots": t.roots.iter().map(|r| json!({"name": r.name, "key": r.key})).collect::<Vec<_>>(),
            "allow_unsigned": t.allow_unsigned,
            "allow_other_signed": t.allow_other_signed,
        })),
        "capabilities": { "deny": policy.deny.iter().map(ToString::to_string).collect::<Vec<_>>() },
        "network": {
            "allow": policy.network,
            "proxy": policy.proxy.as_ref().map(|proxy| match proxy {
                Proxy::Direct => "direct".to_string(),
                Proxy::Via(url) => redact_proxy(url),
            }),
            "no_proxy": policy.no_proxy,
        },
        "usage": { "counting": policy.counting },
        "hub": policy.hub.as_ref().map(|url| json!({ "url": url })),
        "studio": { "ai": policy.ai, "providers": policy.providers },
        "updates": { "mode": policy.updates.as_str() },
        "audit": { "path": policy.audit_path.as_ref().map(|p| p.display().to_string()) },
        "runtime": { "interpreter": policy.interpreter },
        "org": policy.org.as_ref().map(|slug| json!({ "slug": slug })),
        "apps": policy
            .apps
            .iter()
            .map(|(id, rules)| (id.clone(), rules.json()))
            .collect::<serde_json::Map<String, Value>>(),
    })
}

fn describe(state: &State, record: &audit::Location) -> Value {
    let record_json = json!({
        "path": record.path.display().to_string(),
        "fallback_from": record.fallback_from.as_ref().map(|p| p.display().to_string()),
    });
    match state {
        State::None => json!({
            "v": 1, "state": "none", "source": null, "ignored": [], "problem": null,
            "policy": null, "record": record_json,
        }),
        State::InForce {
            policy,
            source,
            ignored,
        } => json!({
            "v": 1, "state": "in-force", "source": source.json(),
            "ignored": ignored.iter().map(Source::json).collect::<Vec<_>>(),
            "problem": null, "policy": policy_json(policy), "record": record_json,
        }),
        State::Unreadable {
            source,
            problem,
            ignored,
        } => json!({
            "v": 1, "state": "unreadable", "source": source.json(),
            "ignored": ignored.iter().map(Source::json).collect::<Vec<_>>(),
            "problem": problem, "words": unreadable_words(source, problem),
            "policy": null, "record": record_json,
        }),
    }
}

fn words_for(state: &State, record: &audit::Location) -> String {
    let mut out = String::new();
    let mut line = |text: String| {
        out.push_str(&text);
        out.push('\n');
    };
    let record_lines = |line: &mut dyn FnMut(String)| {
        line(format!("Open record: {}", record.path.display()));
        if let Some(from) = &record.fallback_from {
            line(format!(
                "  {} cannot be written by you, so the record is kept here instead.",
                from.display()
            ));
        }
    };
    let ignored_lines = |line: &mut dyn FnMut(String), ignored: &[Source]| {
        for source in ignored {
            line(format!(
                "  Ignored: {} ({}) -- the source above wins.",
                source.location,
                source.kind.as_str()
            ));
        }
    };
    match state {
        State::None => {
            line("No organisation policy on this computer.".to_string());
            record_lines(&mut line);
            line(
                "  Your own history of what opened here. It never leaves this computer."
                    .to_string(),
            );
        }
        State::Unreadable {
            source,
            problem,
            ignored,
        } => {
            line(
                "Your organisation's policy on this computer cannot be used, so Krate opens no \
                 apps until it is fixed."
                    .to_string(),
            );
            line(format!(
                "  From: {} ({})",
                source.location,
                source.kind.as_str()
            ));
            line(format!("  The problem: {problem}"));
            ignored_lines(&mut line, ignored);
            record_lines(&mut line);
        }
        State::InForce {
            policy,
            source,
            ignored,
        } => {
            line(format!(
                "{} sets how Krate works on this computer.",
                capitalise(&who(Some(policy)))
            ));
            line(format!(
                "  From: {} ({})",
                source.location,
                source.kind.as_str()
            ));
            ignored_lines(&mut line, ignored);
            match &policy.trust {
                None => line("Apps: any app may open, signed or not.".to_string()),
                Some(trust) => {
                    if trust.roots.is_empty() {
                        line("Apps: any signer is accepted.".to_string());
                    } else {
                        let names = trust
                            .roots
                            .iter()
                            .map(|r| {
                                let label = if r.name.is_empty() { "a root" } else { &r.name };
                                format!("{label} ({}...)", short(&r.key))
                            })
                            .collect::<Vec<_>>()
                            .join(", ");
                        line(format!("Apps: signed by {names}."));
                    }
                    line(format!(
                        "  Unsigned apps: {}.",
                        if trust.allow_unsigned {
                            "allowed"
                        } else {
                            "not allowed"
                        }
                    ));
                    if !trust.roots.is_empty() {
                        line(format!(
                            "  Apps signed by anyone else: {}.",
                            if trust.allow_other_signed {
                                "allowed"
                            } else {
                                "not allowed"
                            }
                        ));
                    }
                    line("  A signature that does not hold never opens.".to_string());
                }
            }
            if !policy.deny.is_empty() {
                line(format!(
                    "Blocked for every app: {}.",
                    policy
                        .deny
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            match &policy.proxy {
                Some(Proxy::Direct) => {
                    line("Connections: direct, whatever proxy the environment names.".to_string())
                }
                Some(Proxy::Via(url)) => line(format!(
                    "Connections: through the proxy {}{}.",
                    redact_proxy(url),
                    policy
                        .no_proxy
                        .as_ref()
                        .map(|n| format!(", except {n}"))
                        .unwrap_or_default()
                )),
                None => {}
            }
            if let Some(allow) = &policy.network {
                line(format!("Apps may reach only: {}.", allow.join(", ")));
            }
            for (id, rules) in policy.apps.iter().filter(|(_, r)| !r.is_empty()) {
                let whom = if id == "*" {
                    "Every organisation app".to_string()
                } else {
                    id.clone()
                };
                line(format!("{whom}: {}.", rules.words().join("; ")));
            }
            line(format!(
                "Usage counting: {}.",
                if policy.counting {
                    "as the person chooses"
                } else {
                    "off, nothing is sent"
                }
            ));
            if let Some(hub) = &policy.hub {
                line(format!("Hub: {hub} only."));
            }
            if let Some(slug) = &policy.org {
                line(format!(
                    "Organisation: {slug}. Its apps are listed first (`krate org apps`), and \
                     signing in uses its company sign-in."
                ));
            }
            match (&policy.ai, &policy.providers) {
                (false, _) => line("Building apps with AI: off.".to_string()),
                (true, Some(providers)) => line(format!(
                    "Building apps with AI: with {} only.",
                    providers.join(", ")
                )),
                (true, None) => line("Building apps with AI: on.".to_string()),
            }
            line(format!(
                "Updates: {}.",
                match policy.updates {
                    UpdateMode::Auto => "installed as usual",
                    UpdateMode::Notify => "Krate says when one is out and never installs it",
                    UpdateMode::Off => "never checked",
                }
            ));
            if policy.interpreter {
                line(
                    "Apps run on the interpreter rather than compiled for this computer; they \
                     may run slower this way."
                        .to_string(),
                );
            }
            record_lines(&mut line);
        }
    }
    out
}

// ---------------------------------------------------------------- Windows registry

#[cfg(windows)]
mod registry {
    //! `HKLM\SOFTWARE\Policies\Krate`, as the ADMX template writes it:
    //! each TOML table is a subkey (`trust`, `capabilities`, ...), booleans
    //! and `version` are REG_DWORD, text is REG_SZ, a list is one
    //! REG_MULTI_SZ named after its key, and the roots are the subkey
    //! `trust\roots` with one REG_SZ per root (name = the root's name, data
    //! = its 64-hex key).

    use super::PValue;
    use std::collections::BTreeMap;
    use windows_sys::Win32::Foundation::{
        ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS,
    };
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegEnumValueW, RegOpenKeyExW, HKEY, HKEY_LOCAL_MACHINE,
        KEY_READ, REG_DWORD, REG_EXPAND_SZ, REG_MULTI_SZ, REG_QWORD, REG_SZ,
    };

    pub const KEY: &str = r"SOFTWARE\Policies\Krate";

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    struct Key(HKEY);

    impl Drop for Key {
        fn drop(&mut self) {
            unsafe {
                RegCloseKey(self.0);
            }
        }
    }

    fn open(parent: HKEY, path: &str) -> Result<Option<Key>, String> {
        let mut key: HKEY = std::ptr::null_mut();
        let path = wide(path);
        let status = unsafe { RegOpenKeyExW(parent, path.as_ptr(), 0, KEY_READ, &mut key) };
        match status {
            ERROR_SUCCESS => Ok(Some(Key(key))),
            ERROR_FILE_NOT_FOUND => Ok(None),
            other => Err(format!("the registry key cannot be read (error {other})")),
        }
    }

    pub fn present() -> bool {
        !matches!(open(HKEY_LOCAL_MACHINE, KEY), Ok(None))
    }

    pub fn read() -> Result<PValue, String> {
        match open(HKEY_LOCAL_MACHINE, KEY)? {
            Some(key) => read_key(&key, 0),
            None => Err("the registry key is gone".to_string()),
        }
    }

    fn text(units: &[u16]) -> String {
        let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
        String::from_utf16_lossy(&units[..end])
    }

    fn read_key(key: &Key, depth: usize) -> Result<PValue, String> {
        if depth > 8 {
            return Err("the registry key is nested too deep".to_string());
        }
        let mut table = BTreeMap::new();
        let mut index = 0u32;
        loop {
            let mut name = vec![0u16; 16_384];
            let mut name_len = name.len() as u32;
            let mut kind = 0u32;
            let mut data = vec![0u8; 65_536];
            let mut data_len = data.len() as u32;
            let status = unsafe {
                RegEnumValueW(
                    key.0,
                    index,
                    name.as_mut_ptr(),
                    &mut name_len,
                    std::ptr::null(),
                    &mut kind,
                    data.as_mut_ptr(),
                    &mut data_len,
                )
            };
            if status == ERROR_NO_MORE_ITEMS {
                break;
            }
            if status != ERROR_SUCCESS {
                return Err(format!("a registry value cannot be read (error {status})"));
            }
            data.truncate(data_len as usize);
            let name = String::from_utf16_lossy(&name[..name_len as usize]);
            let units: Vec<u16> = data
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect();
            let value = match kind {
                REG_DWORD if data.len() >= 4 => PValue::Int(i64::from(u32::from_le_bytes([
                    data[0], data[1], data[2], data[3],
                ]))),
                REG_QWORD if data.len() >= 8 => {
                    let mut bytes = [0u8; 8];
                    bytes.copy_from_slice(&data[..8]);
                    PValue::Int(i64::from_le_bytes(bytes))
                }
                REG_SZ | REG_EXPAND_SZ => PValue::Str(text(&units)),
                REG_MULTI_SZ => PValue::List(
                    units
                        .split(|&u| u == 0)
                        .filter(|part| !part.is_empty())
                        .map(|part| PValue::Str(String::from_utf16_lossy(part)))
                        .collect(),
                ),
                _ => PValue::Other("a registry type Krate does not read"),
            };
            table.insert(name, value);
            index += 1;
        }
        let mut index = 0u32;
        loop {
            let mut name = vec![0u16; 256];
            let mut name_len = name.len() as u32;
            let status = unsafe {
                RegEnumKeyExW(
                    key.0,
                    index,
                    name.as_mut_ptr(),
                    &mut name_len,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            if status == ERROR_NO_MORE_ITEMS {
                break;
            }
            if status != ERROR_SUCCESS {
                return Err(format!("a registry key cannot be read (error {status})"));
            }
            let child_name = String::from_utf16_lossy(&name[..name_len as usize]);
            if let Some(child) = open(key.0, &child_name)? {
                table.insert(child_name, read_key(&child, depth + 1)?);
            }
            index += 1;
        }
        Ok(PValue::Table(table))
    }
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    fn toml_policy(text: &str) -> std::result::Result<Policy, String> {
        let value: toml::Value = toml::from_str(text).expect("valid toml");
        parse(&PValue::from_toml(&value), SourceKind::File)
    }

    const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn the_contract_example_parses_with_its_defaults() {
        let policy = toml_policy(&format!(
            r#"
version = 1
name = "Acme Labs"
[trust]
roots = [ {{ name = "Acme Labs", key = "{KEY}" }} ]
[capabilities]
deny = ["camera.capture", "audio.capture"]
[network]
allow = ["*.acme.com:443", "api.github.com:443"]
[usage]
counting = false
[hub]
url = "https://hub.krate.tech"
[studio]
ai = true
providers = ["claude", "codex"]
[updates]
mode = "notify"
[audit]
path = "/var/log/krate/opens.jsonl"
[runtime]
interpreter = true
"#
        ))
        .expect("the contract's example is a valid policy");
        let trust = policy.trust.expect("trust");
        assert_eq!(trust.roots.len(), 1);
        assert!(
            !trust.allow_unsigned,
            "roots given: unsigned defaults to no"
        );
        assert!(!trust.allow_other_signed);
        assert!(!policy.counting);
        assert_eq!(policy.hub.as_deref(), Some("https://hub.krate.tech"));
        assert_eq!(policy.updates, UpdateMode::Notify);
        assert!(policy.interpreter);
        assert_eq!(policy.network.expect("ceiling").len(), 2);
    }

    #[test]
    fn trust_without_roots_keeps_both_doors_open_unless_told() {
        let policy = toml_policy("version = 1\n[trust]\nallow_unsigned = false\n").expect("ok");
        let trust = policy.trust.clone().expect("trust");
        assert!(!trust.allow_unsigned);
        assert!(trust.allow_other_signed);
        // Rule 2: no roots, any signer passes; unsigned is refused.
        let signed = SigFacts::Holds {
            signer: KEY.into(),
            root: KEY.into(),
        };
        assert!(trust_refusal(&policy, &signed).is_none());
        assert_eq!(
            trust_refusal(&policy, &SigFacts::Unsigned).map(|r| r.reason),
            Some("policy-unsigned".to_string())
        );
    }

    #[test]
    fn a_broken_signature_never_passes_whatever_the_flags() {
        let policy =
            toml_policy("version = 1\n[trust]\nallow_unsigned = true\nallow_other_signed = true\n")
                .expect("ok");
        assert_eq!(
            trust_refusal(&policy, &SigFacts::Broken).map(|r| r.reason),
            Some("policy-tampered".to_string())
        );
        assert!(trust_refusal(&policy, &SigFacts::Unsigned).is_none());
    }

    #[test]
    fn a_root_not_listed_is_refused_unless_others_are_allowed() {
        let other = "f".repeat(64);
        let listed = SigFacts::Holds {
            signer: other.clone(),
            root: KEY.into(),
        };
        let stranger = SigFacts::Holds {
            signer: other.clone(),
            root: other.clone(),
        };
        let strict = toml_policy(&format!(
            "version = 1\nname = \"Acme\"\n[trust]\nroots = [{{ name = \"Acme\", key = \"{KEY}\" }}]\n"
        ))
        .expect("ok");
        assert!(
            trust_refusal(&strict, &listed).is_none(),
            "delegated to a listed root"
        );
        let refused = trust_refusal(&strict, &stranger).expect("refused");
        assert_eq!(refused.reason, "policy-other-signer");
        assert!(refused.words.contains("your organisation (Acme)"));
        assert!(refused.words.contains("Ask Acme's IT team"));
        let loose = toml_policy(&format!(
            "version = 1\n[trust]\nroots = [{{ key = \"{KEY}\" }}]\nallow_other_signed = true\n"
        ))
        .expect("ok");
        assert!(trust_refusal(&loose, &stranger).is_none());
    }

    #[test]
    fn every_mistake_names_its_key_and_fails_closed() {
        for (text, names) in [
            ("name = \"x\"\n", "`version` is missing"),
            ("version = 2\n", "`version` is 2"),
            ("version = \"1\"\n", "`version` should be the number 1"),
            (
                "version = 1\ncolour = \"red\"\n",
                "`colour` is not a setting",
            ),
            (
                "version = 1\n[trust]\nallow_unsinged = false\n",
                "`trust.allow_unsinged`",
            ),
            (
                "version = 1\n[trust]\nroots = [{ name = \"a\", key = \"abc\" }]\n",
                "`trust.roots[0].key` should be 64 hex",
            ),
            (
                "version = 1\n[capabilities]\ndeny = [\"camra\"]\n",
                "`camra`",
            ),
            (
                "version = 1\n[network]\nallow = [\"acme.com\"]\n",
                "`network.allow`",
            ),
            (
                "version = 1\n[usage]\ncounting = \"no\"\n",
                "`usage.counting` should be true",
            ),
            (
                "version = 1\n[hub]\nurl = \"http://hub.example\"\n",
                "`hub.url`",
            ),
            (
                "version = 1\n[studio]\nproviders = [\"skynet\"]\n",
                "\"skynet\"",
            ),
            (
                "version = 1\n[updates]\nmode = \"sometimes\"\n",
                "`updates.mode`",
            ),
            ("version = 1\n[audit]\npath = \"logs/x\"\n", "`audit.path`"),
            (
                "version = 1\n[runtime]\ninterpreter = 1\n",
                "`runtime.interpreter`",
            ),
            ("version = 1\n[runtime]\njit = false\n", "`runtime.jit`"),
        ] {
            let err = toml_policy(text).expect_err(text);
            assert!(err.contains(names), "{text:?} -> {err}");
        }
    }

    #[test]
    fn the_registry_shape_reads_like_the_file() {
        // DWORD 0/1 for booleans, MULTI_SZ for lists, and roots as
        // name = key values under `trust\roots`.
        let mut roots = BTreeMap::new();
        roots.insert("Acme".to_string(), PValue::Str(KEY.to_string()));
        let mut trust = BTreeMap::new();
        trust.insert("roots".to_string(), PValue::Table(roots));
        trust.insert("allow_unsigned".to_string(), PValue::Int(0));
        let mut caps = BTreeMap::new();
        caps.insert(
            "deny".to_string(),
            PValue::List(vec![PValue::Str("camera.capture".to_string())]),
        );
        let mut top = BTreeMap::new();
        top.insert("version".to_string(), PValue::Int(1));
        top.insert("trust".to_string(), PValue::Table(trust));
        top.insert("capabilities".to_string(), PValue::Table(caps));
        let policy = parse(&PValue::Table(top.clone()), SourceKind::Registry).expect("registry");
        assert_eq!(policy.trust.expect("trust").roots[0].name, "Acme");
        assert_eq!(policy.deny.len(), 1);
        // The same shape is not accepted from a file: a file has real lists.
        assert!(parse(&PValue::Table(top), SourceKind::File).is_err());
    }

    #[test]
    fn a_hub_pin_must_be_an_address_with_no_path() {
        assert_eq!(
            hub_origin("https://hub.acme.com/").as_deref(),
            Some("https://hub.acme.com")
        );
        assert_eq!(
            hub_origin("http://127.0.0.1:8787").as_deref(),
            Some("http://127.0.0.1:8787")
        );
        assert!(hub_origin("https://hub.acme.com/a/b").is_none());
        assert!(hub_origin("http://hub.acme.com").is_none());
        assert!(hub_origin("ftp://hub.acme.com").is_none());
    }

    /// The real key for the samples' placeholder, so the rest of each
    /// sample is what is read.
    const SAMPLE_PLACEHOLDER: &str = "REPLACE-WITH-YOUR-ROOT-PUBLIC-KEY-64-HEX-CHARACTERS";

    fn check_the_sample(policy: &Policy) {
        assert_eq!(policy.name.as_deref(), Some("Acme Labs"));
        let trust = policy.trust.clone().expect("trust");
        assert_eq!(
            trust.roots,
            vec![Root {
                name: "Acme Labs".into(),
                key: KEY.into()
            }]
        );
        assert!(!trust.allow_unsigned && !trust.allow_other_signed);
        let deny: Vec<String> = policy.deny.iter().map(ToString::to_string).collect();
        assert_eq!(deny, ["camera.capture", "audio.capture", "location.read"]);
        assert_eq!(
            policy.network.clone().expect("ceiling"),
            ["*.acme.com:443", "api.github.com:443"]
        );
        assert!(!policy.counting);
        assert_eq!(policy.hub.as_deref(), Some("https://hub.krate.tech"));
        assert!(policy.ai);
        assert_eq!(
            policy.providers.clone().expect("providers"),
            ["claude", "codex"]
        );
        assert_eq!(policy.updates, UpdateMode::Notify);
        assert!(!policy.interpreter);
        assert_eq!(policy.org.as_deref(), Some("acme-labs"));
    }

    /// `[org] slug` (IC-953) names the organisation whose apps come first:
    /// a hub's slug, read strictly like every other key.
    #[test]
    fn the_org_slug_is_a_hub_slug_and_nothing_else() {
        let base = "version = 1\n";
        assert_eq!(toml_policy(base).expect("no org").org, None);
        let read = toml_policy(&format!("{base}[org]\nslug = \"Acme-Labs\"\n")).expect("org");
        assert_eq!(read.org.as_deref(), Some("acme-labs"));
        for bad in ["a", "1acme", "acme labs", "acme_labs", &"a".repeat(40), ""] {
            let err =
                toml_policy(&format!("{base}[org]\nslug = \"{bad}\"\n")).expect_err("not a slug");
            assert!(err.contains("`org.slug`"), "{bad}: {err}");
        }
        let err = toml_policy(&format!("{base}[org]\nslug = \"acme\"\nname = \"x\"\n"))
            .expect_err("unknown key");
        assert!(
            err.contains("`org.name` is not a setting Krate knows"),
            "{err}"
        );
        let err = toml_policy(&format!("{base}org = \"acme\"\n")).expect_err("not a table");
        assert!(err.contains("`org` should be a table"), "{err}");
    }

    /// The sample policy.toml published at /deploy (read from
    /// docs/landing/deploy itself, so the two cannot drift) is a policy this parser reads, key for
    /// key -- and as published, with its placeholder root key, it refuses
    /// in words naming that key, which is what an IT team who forgot to
    /// replace it should see.
    #[test]
    fn the_published_sample_policy_toml_is_read_key_for_key() {
        let sample = include_str!("../../../docs/landing/deploy/policy.toml");
        let err = toml_policy(sample).expect_err("the placeholder key is not a key");
        assert!(
            err.contains("`trust.roots[0].key` should be 64 hex"),
            "{err}"
        );
        let policy = toml_policy(&sample.replace(SAMPLE_PLACEHOLDER, KEY)).expect("the sample");
        check_the_sample(&policy);
    }

    /// The sample managed-preferences plist, with the `Payload*` keys a
    /// profile install leaves beside the domain's own.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_published_sample_plist_is_read_key_for_key() {
        let sample = include_str!("../../../docs/landing/deploy/tech.krate.policy.plist")
            .replace(SAMPLE_PLACEHOLDER, KEY)
            .replacen(
                "<dict>",
                "<dict>\n\t<key>PayloadUUID</key>\n\t<string>0F3C</string>\n\t\
                 <key>PayloadVersion</key>\n\t<integer>1</integer>",
                1,
            );
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tech.krate.policy.plist");
        std::fs::write(&path, sample).expect("plist");
        let value = read_plist(&path).expect("plutil reads it");
        let policy = parse(&value, SourceKind::ManagedPreferences).expect("the sample");
        check_the_sample(&policy);
    }

    /// Read a `.reg` export (UTF-16, `dword:`, `hex(7):` MULTI_SZ) into the
    /// tree the registry reader builds, so the layout the ADMX template
    /// writes is checked on every OS, not only on Windows.
    fn read_reg_export(bytes: &[u8]) -> PValue {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        let text = String::from_utf16_lossy(&units).replace('\u{feff}', "");
        let mut root: BTreeMap<String, PValue> = BTreeMap::new();
        let mut path: Vec<String> = Vec::new();
        for line in text.lines().map(str::trim) {
            // A comment, as regedit reads one.
            if line.starts_with(';') {
                continue;
            }
            if let Some(key) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                let key = key
                    .strip_prefix(r"HKEY_LOCAL_MACHINE\SOFTWARE\Policies\Krate")
                    .expect("under the policy key");
                path = key
                    .split('\\')
                    .filter(|p| !p.is_empty())
                    .map(str::to_string)
                    .collect();
                continue;
            }
            let Some((name, data)) = line.split_once('=') else {
                continue;
            };
            let name = name.trim_matches('"').to_string();
            let value = if let Some(hex) = data.strip_prefix("dword:") {
                PValue::Int(i64::from_str_radix(hex, 16).expect("dword"))
            } else if let Some(hex) = data.strip_prefix("hex(7):") {
                let bytes: Vec<u8> = hex
                    .split(',')
                    .map(|b| u8::from_str_radix(b.trim(), 16).expect("hex"))
                    .collect();
                let units: Vec<u16> = bytes
                    .chunks_exact(2)
                    .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                    .collect();
                PValue::List(
                    units
                        .split(|&u| u == 0)
                        .filter(|part| !part.is_empty())
                        .map(|part| PValue::Str(String::from_utf16_lossy(part)))
                        .collect(),
                )
            } else {
                PValue::Str(data.trim_matches('"').to_string())
            };
            let mut table = &mut root;
            for part in &path {
                let entry = table
                    .entry(part.clone())
                    .or_insert_with(|| PValue::Table(BTreeMap::new()));
                let PValue::Table(inner) = entry else {
                    panic!("{part} is a value and a key");
                };
                table = inner;
            }
            table.insert(name, value);
        }
        PValue::Table(root)
    }

    /// The sample `.reg` export of the ADMX layout reads as the same policy.
    #[test]
    fn the_published_sample_registry_layout_is_read_key_for_key() {
        let mut value = read_reg_export(include_bytes!(
            "../../../docs/landing/deploy/windows/krate-policy-example.reg"
        ));
        // The placeholder, as in the other samples.
        if let PValue::Table(top) = &mut value {
            if let Some(PValue::Table(trust)) = top.get_mut("trust") {
                if let Some(PValue::Table(roots)) = trust.get_mut("roots") {
                    for key in roots.values_mut() {
                        *key = PValue::Str(KEY.to_string());
                    }
                }
            }
        }
        let policy = parse(&value, SourceKind::Registry).expect("the registry sample");
        check_the_sample(&policy);
    }

    /// G17: `[apps]` rules, for every organisation app or one app by id,
    /// read strictly and fail closed; they narrow only the apps they name.
    #[test]
    fn app_rules_are_read_strictly_and_narrow_only_their_apps() {
        let policy = toml_policy(
            "version = 1\n\
             [apps.\"*\"]\nclipboard_write = false\nwipe_when_offline_ends = true\n\
             [apps.\"Dev.Acme.Payroll\"]\nclipboard_read = false\nprinting = false\n\
             screen_capture = false\n",
        )
        .expect("rules");
        let payroll = AppRef {
            id: Some("dev.acme.payroll".into()),
            org_app: true,
        };
        let other_org = AppRef {
            id: Some("dev.acme.notes".into()),
            org_app: true,
        };
        let not_org = AppRef {
            id: Some("dev.someone.game".into()),
            org_app: false,
        };
        let all = policy.rules_for(&payroll);
        assert!(all.no_clipboard_write && all.no_clipboard_read && all.no_printing);
        assert!(all.no_screen_capture && all.wipe_when_offline_ends);
        let notes = policy.rules_for(&other_org);
        assert!(notes.no_clipboard_write && notes.wipe_when_offline_ends);
        assert!(!notes.no_clipboard_read && !notes.no_printing && !notes.no_screen_capture);
        assert!(
            policy.rules_for(&not_org).is_empty(),
            "not an organisation app"
        );
        // A rule that says the default narrows nothing.
        let open = toml_policy("version = 1\n[apps.\"*\"]\nprinting = true\n").expect("open");
        assert!(open.rules_for(&other_org).is_empty());
        // Unknown keys, wrong types and a name that is not an app id refuse.
        for (bad, says) in [
            (
                "[apps.\"*\"]\ncopy = false\n",
                "`apps.*.copy` is not a setting Krate knows",
            ),
            (
                "[apps.\"*\"]\nprinting = \"no\"\n",
                "`apps.*.printing` should be true or false",
            ),
            (
                "[apps.payroll]\nprinting = false\n",
                "`apps.payroll` should be named by an app's id",
            ),
            ("apps = 1\n", "`apps` should be a table"),
        ] {
            let err = toml_policy(&format!("version = 1\n{bad}")).expect_err(bad);
            assert!(err.contains(says), "{bad}: {err}");
        }
        // The published sample's commented-out section, uncommented, is a
        // policy this parser reads key for key.
        let sample = include_str!("../../../docs/landing/deploy/policy.toml");
        let section: String = sample
            .lines()
            .skip_while(|l| !l.starts_with("# [apps."))
            .filter_map(|l| l.strip_prefix("# "))
            .filter(|l| l.starts_with("[apps.") || l.contains(" = "))
            .map(|l| format!("{l}\n"))
            .collect();
        let rules = toml_policy(&format!("version = 1\n{section}")).expect("the sample's rules");
        let every = rules.rules_for(&payroll);
        assert!(every.no_clipboard_read && every.no_clipboard_write && every.no_printing);
        assert!(every.no_screen_capture && every.wipe_when_offline_ends);
        // The registry sample's commented-out section, uncommented, is the
        // same rules as the registry reads them (DWORD 0 and 1).
        let reg = include_bytes!("../../../docs/landing/deploy/windows/krate-policy-example.reg");
        let units: Vec<u16> = reg
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        let text: String = String::from_utf16_lossy(&units)
            .lines()
            .filter(|l| l.starts_with(";[") || l.starts_with(";\"") || l.starts_with('['))
            .map(|l| format!("{}\r\n", l.trim_start_matches(';')))
            .filter(|l| !l.contains("trust"))
            .collect::<String>()
            + "[HKEY_LOCAL_MACHINE\\SOFTWARE\\Policies\\Krate]\r\n\"version\"=dword:00000001\r\n";
        let bytes: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let from_registry =
            parse(&read_reg_export(&bytes), SourceKind::Registry).expect("registry rules");
        let every = from_registry.rules_for(&payroll);
        assert!(every.no_clipboard_read && every.no_clipboard_write && every.no_printing);
        assert!(every.no_screen_capture && every.wipe_when_offline_ends);
        // The Windows template sets them for every organisation app, by
        // the same key names, in apps\*.
        let admx = include_str!("../../../docs/landing/deploy/windows/krate.admx");
        for key in [
            "clipboard_read",
            "clipboard_write",
            "printing",
            "screen_capture",
            "wipe_when_offline_ends",
        ] {
            assert!(
                admx.contains(&format!(
                    "key=\"Software\\Policies\\Krate\\apps\\*\" valueName=\"{key}\""
                )),
                "{key}"
            );
        }
    }

    #[test]
    fn deny_names_are_the_manifests_exactly() {
        for bad in [
            "camera",
            "microphone",
            "screen.capture",
            "ui.clipboard:read",
        ] {
            let err = toml_policy(&format!(
                "version = 1\n[capabilities]\ndeny = [\"{bad}\"]\n"
            ))
            .expect_err(bad);
            assert!(
                err.contains("`capabilities.deny`") && err.contains(bad),
                "{err}"
            );
        }
    }

    #[test]
    fn a_proxy_is_direct_or_an_http_address_and_nothing_else() {
        assert_eq!(parse_proxy("direct"), Some(Proxy::Direct));
        assert_eq!(
            parse_proxy("http://proxy.acme.com:3128"),
            Some(Proxy::Via("http://proxy.acme.com:3128".into()))
        );
        assert_eq!(
            parse_proxy("proxy.acme.com:8080"),
            Some(Proxy::Via("http://proxy.acme.com:8080".into()))
        );
        assert_eq!(
            parse_proxy("http://me:secret@proxy.acme.com:3128/"),
            Some(Proxy::Via("http://me:secret@proxy.acme.com:3128".into()))
        );
        for bad in [
            "",
            "socks5://proxy:1080",
            "https://proxy.acme.com:3128",
            "proxy.acme.com",
            "http://proxy.acme.com:3128/path",
            "http://proxy.acme.com:0",
            "http://proxy.acme.com:99999",
            "http://@proxy:3128",
            "DIRECT please",
        ] {
            assert!(parse_proxy(bad).is_none(), "{bad:?} must be refused");
        }
        assert_eq!(
            redact_proxy("http://me:secret@proxy.acme.com:3128"),
            "http://me:***@proxy.acme.com:3128"
        );
        let policy = toml_policy(
            "version = 1\n[network]\nproxy = \"http://proxy.acme.com:3128\"\nno_proxy = \".acme.com\"\n",
        )
        .expect("proxy");
        assert_eq!(
            policy.proxy,
            Some(Proxy::Via("http://proxy.acme.com:3128".into()))
        );
        assert_eq!(policy.no_proxy.as_deref(), Some(".acme.com"));
        assert_eq!(policy.network, None, "a proxy is not a ceiling");
        let err = toml_policy("version = 1\n[network]\nproxy = \"ftp://x:21\"\n").expect_err("ftp");
        assert!(err.contains("`network.proxy`"), "{err}");
    }
}
