//! Phase 2 UCap session policy.
//!
//! This crate decides whether a capability requested by an app is available in
//! the current run session. It is intentionally session-scoped. Persistent
//! grants and revocation are later-phase work.

use std::{collections::BTreeSet, str::FromStr};

use krate_adapter_common::path::LogicalPath;
use krate_manifest::{default_granted_capabilities, Capability, Manifest, ManifestError};
use thiserror::Error;

/// Why a capability is not available, when it is not (CP2, IC-236, IC-573).
///
/// The policy layer used to answer one question -- granted, or not -- and an
/// app could not tell "you refused this" from "this machine cannot do it".
/// Those are different facts and they deserve different behaviour: a refusal
/// is the recipient's decision and the app should degrade politely; an
/// unavailable capability is a platform gap and the app may reasonably say
/// "not on this computer". The clipboard is the case that already exists in
/// the tree: wired in the Linux adapter, `Unsupported` on macOS, and today
/// both reach the guest as the same flat denial.
///
/// Ordering matters where two reasons could both apply. A capability that was
/// never declared is `Undeclared` whatever else is true of it, because the
/// manifest is the contract a recipient read; and a revoked grant reads as
/// `Revoked` rather than a plain denial, so an app can tell that it *had*
/// this and lost it mid-run.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Refusal {
    /// The manifest never declared it. Declaration is the contract; an
    /// undeclared call is denied at dispatch no matter what was granted
    /// (IC-733).
    Undeclared,
    /// Declared, and the recipient did not grant it.
    Denied,
    /// Granted, then withdrawn during the run.
    Revoked,
    /// Granted, and the grant has passed its expiry.
    Expired,
    /// Granted, but this platform or build cannot provide it -- the macOS
    /// clipboard case. Not a permission decision at all.
    Unavailable,
    /// Declared optional, not yet decided, and the app asked before anyone
    /// was available to answer. The app may ask again later.
    Deferred,
}

impl Refusal {
    /// The stable word for this reason: what a `--json` result carries and
    /// what a guest sees, so the vocabulary does not drift per call site.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Undeclared => "undeclared",
            Self::Denied => "denied",
            Self::Revoked => "revoked",
            Self::Expired => "expired",
            Self::Unavailable => "unavailable",
            Self::Deferred => "deferred",
        }
    }

    /// Is this the recipient's decision, as opposed to a fact about the
    /// machine or the manifest?
    ///
    /// The distinction an app needs before it writes an error message: a
    /// person chose this and can choose differently, or nobody chose
    /// anything and asking again is pointless.
    pub fn is_recipient_decision(&self) -> bool {
        matches!(self, Self::Denied | Self::Revoked)
    }

    /// Could asking again in this same run plausibly succeed?
    ///
    /// Only a deferred decision. A denial stands until the recipient changes
    /// it, and nothing about an undeclared, expired or unavailable capability
    /// changes by asking twice -- an app that retries those is a busy loop.
    pub fn may_retry(&self) -> bool {
        matches!(self, Self::Deferred)
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What the broker decided about one capability, and why.
///
/// `Granted` carries where the authority came from, so a result can say
/// whether a person approved this or it arrived from the ambient defaults --
/// which is the difference between "the recipient allowed the microphone"
/// and "nobody was ever asked" (IC-236 provenance).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Granted { via: Provenance },
    Refused { reason: Refusal },
}

impl Decision {
    pub fn is_granted(&self) -> bool {
        matches!(self, Self::Granted { .. })
    }

    /// The refusal reason, or `None` when this was granted.
    pub fn refusal(&self) -> Option<&Refusal> {
        match self {
            Self::Refused { reason } => Some(reason),
            Self::Granted { .. } => None,
        }
    }
}

/// Where a grant's authority came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Provenance {
    /// Allowed to every app without asking: the ambient defaults that cannot
    /// reach past the app's own window.
    Ambient,
    /// A person said yes to this capability.
    Recipient,
    /// The operator passed it on the command line (`--grant`).
    CommandLine,
    /// An automated path granted it with nobody asked -- a thumbnail run, a
    /// preview, `--auto-grant`. Recorded so a result can never present this
    /// as though a person approved it (IC-219, IC-221, IC-341).
    Automatic,
}

impl Provenance {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ambient => "ambient",
            Self::Recipient => "recipient",
            Self::CommandLine => "command-line",
            Self::Automatic => "automatic",
        }
    }

    /// Did a person actually decide this?
    ///
    /// The question every "the user approved X" sentence has to pass before
    /// it is written down.
    pub fn is_person(&self) -> bool {
        matches!(self, Self::Recipient)
    }
}

impl std::fmt::Display for Provenance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPolicy {
    grants: BTreeSet<Capability>,
    /// Where each grant's authority came from. A grant with no entry here is
    /// `Ambient`: the defaults are added by `from_grants` itself, so the map
    /// only ever needs the ones somebody decided.
    provenance: std::collections::BTreeMap<Capability, Provenance>,
    /// Withdrawn during this run, and why. Kept separate from removing the
    /// grant so the decision can say `Revoked` or `Expired` rather than a
    /// bare `Denied` -- an app that is told "denied" for something it has
    /// been using will go looking for a permission prompt, when what it
    /// needs to do is stop.
    ///
    /// One map rather than a set per reason: withdrawal is one mechanism and
    /// the reason is an attribute of it, so a new reason cannot be added
    /// without deciding what `decide` reports for it.
    withdrawn: std::collections::BTreeMap<Capability, Refusal>,
}

impl SessionPolicy {
    pub fn from_grants(grants: impl IntoIterator<Item = Capability>) -> Self {
        let defaults = default_granted_capabilities();
        let mut resolved = defaults.clone();
        let mut provenance = std::collections::BTreeMap::new();
        for cap in grants {
            // Anything the caller passes was decided by somebody; the
            // defaults it may re-state stay ambient. Callers that know who
            // decided use `with_provenance` and overwrite this.
            if !defaults.contains(&cap) {
                provenance.insert(cap.clone(), Provenance::Recipient);
            }
            resolved.insert(cap);
        }
        Self {
            grants: resolved,
            provenance,
            withdrawn: std::collections::BTreeMap::new(),
        }
    }

    /// Same grants, recorded as coming from `via`.
    ///
    /// Used where the caller knows the authority: `--grant` on the command
    /// line, an automated thumbnail run, a person answering a prompt. Without
    /// this every grant would read as though a recipient approved it, and a
    /// `--auto-grant` preview would be indistinguishable from consent
    /// (IC-219, IC-221, IC-341).
    pub fn with_provenance(grants: impl IntoIterator<Item = Capability>, via: Provenance) -> Self {
        let defaults = default_granted_capabilities();
        let mut resolved = defaults.clone();
        let mut provenance = std::collections::BTreeMap::new();
        for cap in grants {
            if !defaults.contains(&cap) {
                provenance.insert(cap.clone(), via);
            }
            resolved.insert(cap);
        }
        Self {
            grants: resolved,
            provenance,
            withdrawn: std::collections::BTreeMap::new(),
        }
    }

    /// Withdraw a grant mid-run.
    ///
    /// The grant stays in `grants` and joins `revoked`, so `decide` can say
    /// `Revoked` instead of a bare `Denied` -- an app can tell it had this
    /// and lost it, which is the difference between "ask the user to enable
    /// the microphone" and "stop recording, they just turned it off".
    pub fn revoke(&mut self, cap: Capability) {
        self.withdrawn.insert(cap, Refusal::Revoked);
    }

    /// Withdraw a grant because it ran out, not because anyone changed their
    /// mind.
    ///
    /// Distinct from `revoke` on purpose. A revocation is a person acting; an
    /// expiry is a clock, and an app may reasonably ask for the capability
    /// again where it would not re-prompt after a refusal. The clock itself
    /// lives with persistent grants in a later phase -- this crate is
    /// session-scoped and has no time source -- so what CP2 owes is the
    /// vocabulary and a decision that can carry it.
    pub fn expire(&mut self, cap: Capability) {
        self.withdrawn.insert(cap, Refusal::Expired);
    }

    /// Has this capability been withdrawn during this run?
    pub fn is_revoked(&self, cap: &Capability) -> bool {
        self.withdrawal_reason(cap).is_some()
    }

    /// Why this capability was withdrawn, if it was.
    pub fn withdrawal_reason(&self, cap: &Capability) -> Option<Refusal> {
        self.withdrawn
            .iter()
            .find(|(withdrawn, _)| capability_allows(withdrawn, cap))
            .map(|(_, reason)| reason.clone())
    }

    /// Where this capability's authority came from, if it is granted.
    pub fn provenance_of(&self, cap: &Capability) -> Option<Provenance> {
        if !self.allows(cap) {
            return None;
        }
        // The most specific recorded grant that covers this call wins; with
        // none recorded the capability came from the ambient defaults.
        self.provenance
            .iter()
            .filter(|(grant, _)| capability_allows(grant, cap))
            .map(|(_, via)| *via)
            .max()
            .or(Some(Provenance::Ambient))
    }

    /// The full decision for one capability, with its reason (CP2).
    ///
    /// `declared` is what the manifest asked for. Declaration is the
    /// contract a recipient read, so a call outside it is `Undeclared` even
    /// if a grant would otherwise cover it -- that is IC-733's rule, and it
    /// is checked first for exactly that reason. Pass `None` where the
    /// declaration is not available at the call site; the check is then
    /// skipped rather than silently failing open on an empty list.
    pub fn decide(
        &self,
        required: &Capability,
        declared: Option<&BTreeSet<Capability>>,
    ) -> Decision {
        if let Some(declared) = declared {
            let ambient = default_granted_capabilities();
            let covered = declared.iter().any(|cap| capability_allows(cap, required))
                || ambient.iter().any(|cap| capability_allows(cap, required));
            if !covered {
                return Decision::Refused {
                    reason: Refusal::Undeclared,
                };
            }
        }
        if let Some(reason) = self.withdrawal_reason(required) {
            // Revoked or expired: report which, so an app can tell a person
            // changing their mind from a grant running out.
            return Decision::Refused { reason };
        }
        match self.provenance_of(required) {
            Some(via) => Decision::Granted { via },
            None => Decision::Refused {
                reason: Refusal::Denied,
            },
        }
    }

    /// The same decision, knowing which declared capabilities are optional.
    ///
    /// An optional capability nobody has decided about is `Deferred`, not
    /// `Denied`. The difference is the whole point of declaring something
    /// optional: a denial stands until the recipient changes it, so an app
    /// that reads one correctly stops asking, and an optional capability
    /// would then be dead the first time it was touched -- before the person
    /// had been asked about it at all. `Deferred` is the only refusal
    /// `may_retry()` allows, which is exactly the "ask at feature time"
    /// behaviour F-049 and F-103 describe.
    ///
    /// A withdrawal still wins: an optional capability a person turned OFF
    /// reports `Revoked`, because they have now decided and asking again
    /// would be pestering them.
    pub fn decide_with_optional(
        &self,
        required: &Capability,
        declared: Option<&BTreeSet<Capability>>,
        optional: &BTreeSet<Capability>,
    ) -> Decision {
        let decision = self.decide(required, declared);
        let Decision::Refused {
            reason: Refusal::Denied,
        } = decision
        else {
            // Undeclared, revoked, expired or granted: all unchanged. Only a
            // plain denial can be a not-yet-asked optional capability.
            return decision;
        };
        if optional.iter().any(|cap| capability_allows(cap, required)) {
            return Decision::Refused {
                reason: Refusal::Deferred,
            };
        }
        decision
    }

    /// What an app may have when Krate is only taking its picture (IC-219,
    /// IC-221, IC-341).
    ///
    /// Several paths ran a bundle with `--auto-grant` -- every capability the
    /// manifest declared -- purely to get a frame out of it: the card, the
    /// publish preview, the TUI's "open an app", port verification, MCP. A
    /// microphone, a camera, the network and the user's files were all
    /// granted with nobody asked, in order to produce a thumbnail.
    ///
    /// Nothing in the list below can reach past the app's own window. The
    /// ambient defaults come free from `from_grants`; on top of them this
    /// allows only the two declared capabilities an app legitimately needs to
    /// paint a representative first frame:
    ///
    /// - `random.bytes`, because an app that shuffles or picks a colour on
    ///   startup otherwise refuses to run at all.
    /// - `store.kv` and `store.sql`, because an app draws its saved state,
    ///   and a screenshot of an app that could not read its own data is a
    ///   screenshot of an empty app.
    ///
    /// Everything else -- net, fs, media, secrets, shared state -- is refused.
    /// The app still starts and still paints; it simply cannot use the moment
    /// to reach the world. An app that genuinely cannot draw without the
    /// network gets no picture, which is the honest outcome: Krate should not
    /// silently spend the recipient's authority to make its own preview
    /// prettier.
    pub fn for_screenshot(manifest: &Manifest) -> Result<Self> {
        const SAFE_FOR_A_PICTURE: [&str; 3] = ["random.bytes", "store.kv", "store.sql"];
        let declared = manifest.declared_capabilities()?;
        let kept = declared.into_iter().filter(|cap| {
            let named = format!("{}.{}", cap.module(), cap.action());
            SAFE_FOR_A_PICTURE.contains(&named.as_str())
        });
        Ok(Self::from_grants(kept))
    }

    pub fn allow_all_declared(manifest: &Manifest) -> Result<Self> {
        Ok(Self::from_grants(manifest.declared_capabilities()?))
    }

    /// What an automated verification run may have, and what it may not
    /// (IC-364, the founder rule: no automatic real-world authority).
    ///
    /// Verification proves two things -- the app runs with its grants, and
    /// refuses without them -- and it used to prove the first by handing the
    /// app everything it declared. A freshly AI-authored app naming the
    /// microphone or a network destination was given the real microphone and
    /// the real network, automatically, as the last step of `krate create`.
    ///
    /// The split is by where the effect lands. A capability whose effects stay
    /// inside the run is granted: the filesystem is sandboxed to the verify
    /// directory, the stores are app-scoped files, a headless window is no
    /// window. A capability that reaches the person or the world is withheld:
    /// their microphone, camera, clipboard, browser, notifications, and every
    /// network path. Withheld is not denied-and-hidden -- the caller records
    /// exactly what was not exercised, so "verified" never quietly means
    /// "verified except the part that needed your hardware".
    ///
    /// Returns the policy and the declared capabilities it withheld.
    pub fn for_verification(manifest: &Manifest) -> Result<(Self, Vec<Capability>)> {
        let declared = manifest.declared_capabilities()?;
        let (kept, withheld): (Vec<_>, Vec<_>) = declared
            .into_iter()
            .partition(|cap| !reaches_outside_the_run(cap));
        Ok((Self::from_grants(kept), withheld))
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
        // Revocation has to bite here too, not only in `decide`. Every one of
        // the existing call sites goes through this, so a withdrawn grant
        // that still answered `true` would be a revocation in name only.
        if self.is_revoked(required) {
            return false;
        }
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

    /// The same missing capabilities, each with the reason it is missing.
    ///
    /// `missing_required_for_manifest` answers *what*; a caller that wants to
    /// tell somebody what to do about it needs *why*. The difference is not
    /// cosmetic: "grant this and run it again" is sound advice for a denial
    /// and useless for a revoked grant, wrong for an expired one, and a
    /// retry loop for a capability this platform cannot provide at all.
    ///
    /// Declared here means the manifest's required set, so an undeclared
    /// capability cannot appear -- this list is built from the declaration
    /// itself. That is why nothing returns `Undeclared`: reaching this
    /// function already proves the capability was declared.
    pub fn missing_required_with_reasons(
        &self,
        manifest: &Manifest,
    ) -> Result<Vec<(Capability, Refusal)>> {
        let declared: BTreeSet<Capability> =
            manifest.required_capabilities()?.into_iter().collect();
        Ok(declared
            .iter()
            .filter_map(|cap| match self.decide(cap, Some(&declared)) {
                Decision::Granted { .. } => None,
                Decision::Refused { reason } => Some((cap.clone(), reason)),
            })
            .collect())
    }
}

impl Default for SessionPolicy {
    fn default() -> Self {
        Self::from_grants([])
    }
}

/// Does exercising this capability affect anything beyond the run itself?
///
/// The question an automated run has to ask before granting anything: if the
/// answer is yes, only a person may say so (IC-364). "Beyond the run" means
/// the person's hardware, their data outside the sandbox, or the network --
/// not the size of the capability. `fs.write` is powerful but lands inside
/// the verification directory the run was started in; `ui.notify` is tiny
/// and pops a real notification on someone's desktop.
///
/// The names are checked against the registry by a test, so a renamed
/// capability breaks the build here instead of silently becoming grantable.
pub fn reaches_outside_the_run(cap: &Capability) -> bool {
    const REACHES_OUTSIDE: &[&str] = &[
        // The network, in every form it exists.
        "net.connect",
        "store.shared",
        // The person's hardware.
        "audio.capture",
        "camera.capture",
        // The person's attention, data, and machine outside the sandbox.
        "ui.notify",
        "ui.open-url",
        "ui.clipboard",
        // Real file pickers: they open the person's disk, and they block on a
        // click nobody in an automated run can make.
        "ui.dialog:file-open",
        "ui.dialog:file-save",
        "ui.dialog:open-folder",
        "ui.dialog:*",
    ];
    let name = format!("{}.{}", cap.module(), cap.action());
    let with_resource = match cap.resource() {
        Some(resource) => format!("{name}:{resource}"),
        None => name.clone(),
    };
    REACHES_OUTSIDE
        .iter()
        .any(|entry| *entry == name || *entry == with_resource)
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

    fn cap(s: &str) -> Capability {
        Capability::from_str(s).expect("a capability the manifest crate accepts")
    }

    /// The reason a capability is unavailable is not always "you said no",
    /// and an app cannot behave well until it can tell the cases apart.
    ///
    /// This is the shape CP2 exists to fix: before it, the policy layer had
    /// exactly one refusal, so a recipient who revoked the microphone and a
    /// Mac with no clipboard support produced the same flat denial.
    #[test]
    fn a_refusal_says_which_kind_of_no_it_is() {
        let declared: BTreeSet<Capability> = [cap("audio.capture")].into_iter().collect();

        // Declared, never granted: the recipient's decision.
        let nothing_granted = SessionPolicy::from_grants([]);
        let denied = nothing_granted.decide(&cap("audio.capture"), Some(&declared));
        assert_eq!(denied.refusal(), Some(&Refusal::Denied));
        assert!(
            denied.refusal().expect("refused").is_recipient_decision(),
            "a plain denial is a person's choice, and an app may reasonably ask again later"
        );

        // Not declared at all: the manifest is the contract, so this is
        // refused whatever was granted (IC-733).
        let generous = SessionPolicy::from_grants([cap("net.connect:example.com:443")]);
        let undeclared = generous.decide(&cap("net.connect:example.com:443"), Some(&declared));
        assert_eq!(
            undeclared.refusal(),
            Some(&Refusal::Undeclared),
            "a grant cannot authorize what the manifest never declared"
        );
        assert!(
            !undeclared
                .refusal()
                .expect("refused")
                .is_recipient_decision(),
            "nobody chose this; the app asked for something outside its own contract"
        );

        // Granted and then withdrawn mid-run: distinguishable from never
        // having had it, which is what lets an app stop cleanly.
        let mut live = SessionPolicy::from_grants([cap("audio.capture")]);
        assert!(live
            .decide(&cap("audio.capture"), Some(&declared))
            .is_granted());
        live.revoke(cap("audio.capture"));
        let revoked = live.decide(&cap("audio.capture"), Some(&declared));
        assert_eq!(revoked.refusal(), Some(&Refusal::Revoked));
        assert!(
            revoked.refusal().expect("refused").is_recipient_decision(),
            "a revocation is a person changing their mind, not a platform gap"
        );

        // Every reason carries a stable word, and only a deferred one is
        // worth retrying -- an app that retries a denial is a busy loop.
        assert_eq!(Refusal::Unavailable.as_str(), "unavailable");
        assert!(Refusal::Deferred.may_retry());
        for reason in [
            Refusal::Undeclared,
            Refusal::Denied,
            Refusal::Revoked,
            Refusal::Expired,
            Refusal::Unavailable,
        ] {
            assert!(
                !reason.may_retry(),
                "{reason} does not change by asking twice"
            );
        }
    }

    /// An expired grant is not a revoked one, and neither is a denial.
    ///
    /// All three stop the capability working, so it would be easy to fold
    /// them together -- and then an app told "denied" for something it had
    /// been using all session would go looking for a permission prompt when
    /// what it should do is stop. Both withdrawals must also bite on
    /// `allows`, since that is the path the runtime actually calls.
    #[test]
    fn an_expired_grant_is_reported_as_expired_and_not_as_revoked() {
        let declared: BTreeSet<Capability> = [cap("audio.capture")].into_iter().collect();

        let mut ran_out = SessionPolicy::from_grants([cap("audio.capture")]);
        ran_out.expire(cap("audio.capture"));
        assert_eq!(
            ran_out
                .decide(&cap("audio.capture"), Some(&declared))
                .refusal(),
            Some(&Refusal::Expired)
        );
        assert!(
            !ran_out.allows(&cap("audio.capture")),
            "expiry has to stop the capability on the path the runtime calls"
        );

        let mut withdrawn = SessionPolicy::from_grants([cap("audio.capture")]);
        withdrawn.revoke(cap("audio.capture"));
        assert_eq!(
            withdrawn
                .decide(&cap("audio.capture"), Some(&declared))
                .refusal(),
            Some(&Refusal::Revoked)
        );

        // The distinction an app acts on: a person changed their mind, or a
        // clock ran out. Only the first is somebody's decision.
        assert!(Refusal::Revoked.is_recipient_decision());
        assert!(
            !Refusal::Expired.is_recipient_decision(),
            "a clock is not a person; re-prompting for an expiry is the wrong response"
        );
    }

    /// A caller that reports missing capabilities needs the reason with them.
    ///
    /// The MCP run report tells an agent "grant these and try again". That is
    /// right for a denial and wrong for everything else -- a retry after an
    /// expiry, a revocation, or on a platform that cannot provide the
    /// capability is a loop. The reasoned list is what lets a caller tell
    /// those apart.
    ///
    /// Nothing here can be `Undeclared`: the list is built from the
    /// manifest's own required set, so reaching it proves declaration.
    #[test]
    fn the_missing_list_carries_why_each_one_is_missing() {
        let manifest = manifest_declaring(&["audio.capture", "fs.write:data/**"]);

        // Nothing granted: both required capabilities are plain denials.
        let none = SessionPolicy::from_grants([]);
        let missing = none
            .missing_required_with_reasons(&manifest)
            .expect("reasons");
        assert_eq!(missing.len(), 2, "both required capabilities are missing");
        assert!(
            missing.iter().all(|(_, why)| *why == Refusal::Denied),
            "ungranted declared capabilities are denials: {missing:?}"
        );

        // Grant both, then take one back each way. The list must now
        // distinguish them rather than reporting two identical denials.
        let mut live = SessionPolicy::from_grants([cap("audio.capture"), cap("fs.write:data/**")]);
        assert!(
            live.missing_required_with_reasons(&manifest)
                .expect("reasons")
                .is_empty(),
            "everything required is granted"
        );
        live.revoke(cap("audio.capture"));
        live.expire(cap("fs.write:data/**"));

        let missing = live
            .missing_required_with_reasons(&manifest)
            .expect("reasons");
        let reasons: std::collections::BTreeMap<String, Refusal> = missing
            .into_iter()
            .map(|(cap, why)| (cap.to_string(), why))
            .collect();
        assert_eq!(reasons.get("audio.capture"), Some(&Refusal::Revoked));
        assert_eq!(reasons.get("fs.write:data/**"), Some(&Refusal::Expired));
        assert!(
            reasons.values().all(|why| !why.may_retry()),
            "none of these are fixed by asking again, so a caller must not advise a retry"
        );
    }

    /// Revoking has to bite on the OLD path too.
    ///
    /// Every existing call site in the tree goes through `allows`, not
    /// `decide`. If revocation only affected the new method it would be a
    /// revocation in name only -- the capability would keep working
    /// everywhere that matters. Written because that is exactly the mistake
    /// this shape invites.
    #[test]
    fn a_revoked_grant_stops_working_everywhere_not_just_in_decide() {
        let mut policy = SessionPolicy::from_grants([cap("audio.capture")]);
        assert!(policy.allows(&cap("audio.capture")));
        assert!(policy.check(&cap("audio.capture")).is_ok());

        policy.revoke(cap("audio.capture"));

        assert!(
            !policy.allows(&cap("audio.capture")),
            "allows() is what the runtime actually calls"
        );
        assert!(
            policy.check(&cap("audio.capture")).is_err(),
            "check() must refuse it too, or the guard lets it through"
        );
        assert!(policy.provenance_of(&cap("audio.capture")).is_none());
    }

    /// A thumbnail run must never look like consent.
    ///
    /// Several paths grant every declared capability just to paint a frame.
    /// Recording WHO decided is what stops a result saying "the recipient
    /// approved the microphone" when nobody was asked (IC-219, IC-221,
    /// IC-341).
    #[test]
    fn provenance_separates_a_person_from_an_automatic_grant() {
        let asked = SessionPolicy::with_provenance([cap("audio.capture")], Provenance::Recipient);
        let unasked = SessionPolicy::with_provenance([cap("audio.capture")], Provenance::Automatic);

        assert!(asked.decide(&cap("audio.capture"), None).is_granted());
        assert!(unasked.decide(&cap("audio.capture"), None).is_granted());

        let Decision::Granted { via: by_person } = asked.decide(&cap("audio.capture"), None) else {
            panic!("granted");
        };
        let Decision::Granted { via: by_machine } = unasked.decide(&cap("audio.capture"), None)
        else {
            panic!("granted");
        };

        assert!(by_person.is_person());
        assert!(
            !by_machine.is_person(),
            "an --auto-grant preview is not a person approving anything"
        );
        assert_eq!(by_machine.as_str(), "automatic");
        assert_ne!(
            by_person, by_machine,
            "the two must be distinguishable, or a preview can be reported as consent"
        );
    }

    /// The ambient defaults are granted, and they are not anyone's decision.
    ///
    /// Without this an app that draws its own window would be reported as a
    /// capability the recipient approved.
    #[test]
    fn the_ambient_defaults_are_granted_but_nobody_chose_them() {
        let policy = SessionPolicy::default();
        for cap in default_granted_capabilities() {
            let decision = policy.decide(&cap, None);
            assert!(decision.is_granted(), "{cap} is an ambient default");
            let Decision::Granted { via } = decision else {
                unreachable!()
            };
            assert_eq!(via, Provenance::Ambient, "{cap} was never asked about");
            assert!(!via.is_person());
        }
    }

    /// Declaration is checked before the grant, not after.
    ///
    /// Order matters: a capability that is both undeclared AND ungranted has
    /// to report `Undeclared`, because that is the fact that explains it. If
    /// the grant check ran first the app would be told "denied" and a
    /// developer would go looking for a permission prompt that was never the
    /// problem.
    #[test]
    fn an_undeclared_capability_reads_as_undeclared_even_when_it_is_also_ungranted() {
        let declared: BTreeSet<Capability> = [cap("audio.capture")].into_iter().collect();
        let policy = SessionPolicy::from_grants([]);
        assert_eq!(
            policy
                .decide(&cap("net.connect:example.com:443"), Some(&declared))
                .refusal(),
            Some(&Refusal::Undeclared)
        );
    }

    /// The fuzz campaign's interesting inputs, replayed every run (IC-163).
    /// Same body as fuzz/fuzz_targets/policy_match.rs: grant on line one,
    /// required on line two, then allows() and check(). A panic fails;
    /// nothing else can.
    #[test]
    fn every_fuzz_regression_input_runs_without_panicking() {
        use std::str::FromStr;
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fuzz/regressions/policy_match");
        let mut fed = 0;
        for entry in std::fs::read_dir(&dir).expect("the regression corpus exists") {
            let bytes = std::fs::read(entry.expect("entry").path()).expect("fixture");
            let Ok(input) = std::str::from_utf8(&bytes) else {
                continue;
            };
            let mut lines = input.lines();
            let (Some(grant_line), Some(required_line)) = (lines.next(), lines.next()) else {
                continue;
            };
            let (Ok(grant), Ok(required)) = (
                Capability::from_str(grant_line),
                Capability::from_str(required_line),
            ) else {
                continue;
            };
            let policy = SessionPolicy::from_grants([grant]);
            let _ = policy.allows(&required);
            let _ = policy.check(&required);
            fed += 1;
        }
        assert!(
            fed >= 3,
            "the regression corpus went missing or empty ({fed} usable inputs)"
        );
    }

    fn manifest_declaring(caps: &[&str]) -> Manifest {
        let mut source = String::from(
            "[app]\nid = \"dev.krate.test\"\nname = \"Test\"\nversion = \"1.0.0\"\n\
             entry = \"app.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n\n",
        );
        for cap in caps {
            source.push_str(&format!(
                "[[capabilities]]\ncap = \"{cap}\"\nrationale = \"test\"\nrequired = true\n\n"
            ));
        }
        Manifest::parse(&source).expect("manifest")
    }

    /// A manifest with both kinds of capability, so a test can tell what the
    /// broker does with each.
    fn manifest_with(required: &[&str], optional: &[&str]) -> Manifest {
        let mut source = String::from(
            "[app]\nid = \"dev.krate.test\"\nname = \"Test\"\nversion = \"1.0.0\"\n\
             entry = \"app.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n\n",
        );
        for (caps, required) in [(required, true), (optional, false)] {
            for cap in caps {
                source.push_str(&format!(
                    "[[capabilities]]\ncap = \"{cap}\"\nrationale = \"test\"\n\
                     required = {required}\n\n"
                ));
            }
        }
        Manifest::parse(&source).expect("manifest")
    }

    /// An optional capability nobody has decided about is deferred, not
    /// denied -- and that difference is the whole point of declaring one.
    ///
    /// A denial stands until the recipient changes it, so an app that reads
    /// one correctly stops asking. If an undecided optional capability
    /// reported `Denied`, it would be dead the first time the app touched it,
    /// before the person had been asked about it at all. `Deferred` is the
    /// only refusal `may_retry()` allows, which is exactly the ask-at-feature
    /// -time behaviour (F-049, F-103).
    #[test]
    fn an_undecided_optional_capability_is_deferred_and_a_required_one_is_denied() {
        let manifest = manifest_with(&["fs.write:data/**"], &["audio.capture"]);
        let declared: BTreeSet<Capability> = manifest
            .declared_capabilities()
            .expect("declared")
            .into_iter()
            .collect();
        let optional: BTreeSet<Capability> = manifest
            .optional_capabilities()
            .expect("optional")
            .into_iter()
            .collect();
        assert_eq!(
            optional.len(),
            1,
            "the manifest distinguishes the two kinds: {optional:?}"
        );

        let policy = SessionPolicy::from_grants([]);

        let deferred =
            policy.decide_with_optional(&cap("audio.capture"), Some(&declared), &optional);
        assert_eq!(deferred.refusal(), Some(&Refusal::Deferred));
        assert!(
            deferred.refusal().expect("refused").may_retry(),
            "the app may ask for this when the feature is used"
        );

        let denied =
            policy.decide_with_optional(&cap("fs.write:data/**"), Some(&declared), &optional);
        assert_eq!(
            denied.refusal(),
            Some(&Refusal::Denied),
            "a required capability is not deferred just because another one was optional"
        );
        assert!(!denied.refusal().expect("refused").may_retry());
    }

    /// Deciding about an optional capability is final, both ways.
    ///
    /// Granting it makes it granted; turning it off makes it `Revoked`, not
    /// `Deferred`. Without the second half an app would keep re-asking for
    /// something the person had just switched off, which is the exact
    /// behaviour that makes permission prompts hated.
    #[test]
    fn an_optional_capability_stops_being_deferred_once_somebody_decides() {
        let manifest = manifest_with(&[], &["audio.capture"]);
        let declared: BTreeSet<Capability> = manifest
            .declared_capabilities()
            .expect("declared")
            .into_iter()
            .collect();
        let optional: BTreeSet<Capability> = manifest
            .optional_capabilities()
            .expect("optional")
            .into_iter()
            .collect();

        let granted = SessionPolicy::from_grants([cap("audio.capture")]);
        assert!(granted
            .decide_with_optional(&cap("audio.capture"), Some(&declared), &optional)
            .is_granted());

        let mut turned_off = SessionPolicy::from_grants([cap("audio.capture")]);
        turned_off.revoke(cap("audio.capture"));
        let decision =
            turned_off.decide_with_optional(&cap("audio.capture"), Some(&declared), &optional);
        assert_eq!(
            decision.refusal(),
            Some(&Refusal::Revoked),
            "they decided; asking again would be pestering them"
        );
        assert!(!decision.refusal().expect("refused").may_retry());
    }

    /// Optional does not mean undeclared.
    ///
    /// A capability outside the manifest stays `Undeclared` even when the
    /// caller passes an optional set -- otherwise "optional" would become a
    /// way around the declaration contract, and an app could reach anything
    /// by asking for it at feature time (IC-733).
    #[test]
    fn the_optional_set_cannot_smuggle_in_an_undeclared_capability() {
        let manifest = manifest_with(&[], &["audio.capture"]);
        let declared: BTreeSet<Capability> = manifest
            .declared_capabilities()
            .expect("declared")
            .into_iter()
            .collect();
        // A caller that wrongly lists something the manifest never declared.
        let optional: BTreeSet<Capability> =
            [cap("net.connect:example.com:443")].into_iter().collect();

        let policy = SessionPolicy::from_grants([]);
        assert_eq!(
            policy
                .decide_with_optional(
                    &cap("net.connect:example.com:443"),
                    Some(&declared),
                    &optional
                )
                .refusal(),
            Some(&Refusal::Undeclared),
            "the manifest is the contract; an optional list cannot widen it"
        );
    }

    /// IC-219/IC-221. Several paths ran a bundle with every capability it
    /// declared, purely to take its picture -- microphone, camera, network
    /// and the user's files granted so Krate could make a thumbnail. A
    /// screenshot never needs any of them.
    #[test]
    fn a_screenshot_run_cannot_reach_the_world() {
        let manifest = manifest_declaring(&[
            "net.connect:example.com:443",
            "fs.read:documents/**",
            "audio.capture",
            "store.kv",
            "random.bytes",
        ]);
        let policy = SessionPolicy::for_screenshot(&manifest).expect("policy");

        for refused in [
            "net.connect:example.com:443",
            "fs.read:documents/**",
            "audio.capture",
        ] {
            let cap: Capability = refused.parse().expect("cap");
            assert!(
                !policy.allows(&cap),
                "a screenshot run must not be granted {refused}"
            );
        }
        // An app draws its saved state, and a picture of an app that could
        // not read its own data is a picture of an empty app.
        for allowed in ["store.kv", "random.bytes"] {
            let cap: Capability = allowed.parse().expect("cap");
            assert!(policy.allows(&cap), "a screenshot run needs {allowed}");
        }
    }

    /// The screenshot policy must still be narrower than auto-grant even
    /// when the manifest declares nothing sensitive -- otherwise the two
    /// would quietly converge and the distinction would stop being real.
    #[test]
    fn a_screenshot_run_is_never_wider_than_what_was_declared() {
        let manifest = manifest_declaring(&["store.kv"]);
        let policy = SessionPolicy::for_screenshot(&manifest).expect("policy");
        let undeclared: Capability = "net.connect:example.com:443".parse().expect("cap");
        assert!(!policy.allows(&undeclared));
    }

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

    /// The four relations a grant can have to a requirement (IC-017).
    ///
    /// Every grant decision reduces to one of these, and each existing test
    /// covers a nuance of one of them without naming the discipline. This is
    /// the table, so a regression in any relation is caught by the test that
    /// says which relation broke:
    ///
    ///   same          grant == requirement            -> allow
    ///   broader grant grant strictly covers it        -> allow
    ///   narrower      grant covers strictly less      -> DENY
    ///   incomparable  overlap without containment,    -> DENY
    ///                 or different module/action
    ///
    /// The narrower row is the security one: a grant of one file must never
    /// satisfy an app that asks for the tree.
    #[test]
    fn a_grant_allows_exactly_its_own_relation_to_the_requirement() {
        let table: [(&str, &str, bool, &str); 7] = [
            ("fs.read:data/a.txt", "fs.read:data/a.txt", true, "same"),
            (
                "fs.read:data/**",
                "fs.read:data/a.txt",
                true,
                "broader grant",
            ),
            (
                "fs.read:data/**",
                "fs.read:data/deep/b.txt",
                true,
                "broader grant, nested",
            ),
            (
                "fs.read:data/a.txt",
                "fs.read:data/**",
                false,
                "narrower grant",
            ),
            (
                "fs.read:data/**",
                "fs.read:other/a.txt",
                false,
                "incomparable resource",
            ),
            (
                "fs.read:data/**",
                "fs.write:data/a.txt",
                false,
                "incomparable action",
            ),
            (
                "fs.read:data/**",
                "net.fetch:example.com",
                false,
                "incomparable module",
            ),
        ];
        for (grant, required, expected, relation) in table {
            let grant_cap = Capability::from_str(grant).expect("grant parses");
            let Ok(required_cap) = Capability::from_str(required) else {
                // A requirement that does not parse can never be granted,
                // which satisfies the deny rows it appears in.
                assert!(
                    !expected,
                    "{relation}: requirement must parse to be allowed"
                );
                continue;
            };
            assert_eq!(
                capability_allows(&grant_cap, &required_cap),
                expected,
                "{relation}: grant {grant:?} against requirement {required:?}",
            );
        }
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

    /// The verification split, on the shapes that matter (IC-364).
    ///
    /// Withheld: everything that reaches the person or the world. Granted:
    /// everything whose effects land inside the run. The voice prompter is
    /// the live case -- it requires the real microphone, so an automated
    /// verify must withhold it and say so rather than open the mic.
    #[test]
    fn verification_withholds_the_world_and_keeps_the_sandbox() {
        for outside in [
            "net.connect:api.example.com:443",
            "store.shared",
            "audio.capture",
            "camera.capture",
            "ui.notify",
            "ui.open-url",
            "ui.clipboard:read",
            "ui.clipboard:write",
            "ui.dialog:file-open",
        ] {
            let cap = outside.parse().expect("parse capability");
            assert!(
                reaches_outside_the_run(&cap),
                "{outside} reaches the person or the network and must be withheld"
            );
        }
        for contained in [
            "fs.read:notes/**",
            "fs.write:notes/**",
            "store.kv",
            "store.sql",
            "store.secret",
            "random.bytes",
            "ui.window:create",
            "ui.dialog:message",
            "audio.playback",
            "gfx.gpu:basic",
        ] {
            let cap = contained.parse().expect("parse capability");
            assert!(
                !reaches_outside_the_run(&cap),
                "{contained} stays inside the run and must remain grantable"
            );
        }
    }

    /// Every name on the withheld list must exist in the capability registry,
    /// so a renamed capability breaks this test instead of silently becoming
    /// grantable to automated runs.
    #[test]
    fn the_withheld_list_stays_in_step_with_the_registry() {
        let known: std::collections::BTreeSet<String> =
            krate_manifest::supported_capability_specs()
                .iter()
                .map(|spec| spec.name())
                .collect();
        for entry in [
            "net.connect",
            "store.shared",
            "audio.capture",
            "camera.capture",
            "ui.notify",
            "ui.open-url",
            "ui.clipboard",
            "ui.dialog",
        ] {
            assert!(
                known.contains(entry),
                "{entry} is on the withheld list but not in the registry -- \
                 renamed there and orphaned here"
            );
        }
    }

    /// The split itself: a manifest declaring both kinds gets the contained
    /// capability granted and the real-world one withheld and reported.
    #[test]
    fn for_verification_splits_a_mixed_manifest() {
        let manifest = Manifest::parse(
            r#"
[app]
id = "dev.test.mixed"
name = "Mixed"
version = "0.0.1"
entry = "code.wasm"
world = "krate:app/gui@0.2.0"

[[capabilities]]
cap = "fs.read:input/**"
required = true
rationale = "reads its input"

[[capabilities]]
cap = "audio.capture"
required = true
rationale = "listens"
"#,
        )
        .expect("parse manifest");
        let (policy, withheld) =
            SessionPolicy::for_verification(&manifest).expect("verification split");
        let fs_read = "fs.read:input/sample.txt".parse().expect("parse");
        let mic = "audio.capture".parse().expect("parse");
        assert!(policy.allows(&fs_read), "sandboxed fs stays granted");
        assert!(!policy.allows(&mic), "the microphone is never granted");
        assert_eq!(withheld.len(), 1);
        assert_eq!(withheld[0].to_string(), "audio.capture");
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
