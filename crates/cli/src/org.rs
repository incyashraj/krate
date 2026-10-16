//! Organisations on a hub (IC-946): members, invitations, a root key,
//! candidates that a second person approves, private releases, and the
//! organisation's audit.
//!
//! The hub is the only authority. Everything here asks it and says what it
//! answered in plain words: a success says what happened, and a refusal says
//! the hub's stable reason in a sentence with the next step, never as JSON.
//! The routes, the JSON shapes and the reason strings are the contract in
//! `Plan/org-pilot/CONTRACT.md`, which the hub builds to as well.
//!
//! Sign-in is the one `krate publish` already uses: the GitHub identity in
//! ~/.krate/github.json, sent as `Authorization: Bearer <token>`, which the
//! hub checks with GitHub. Every request carries an `Idempotency-Key`, one per
//! command, so a retry after a dropped connection is the same request rather
//! than a second invitation or a second removal.

use std::path::Path;

use anyhow::{Context, Result};
use clap::{Subcommand, ValueEnum};

use crate::github_auth;

/// `krate org apps` (IC-953).
mod apps;
/// `krate org connection ...` (IC-952).
mod connection;

/// Signing nobody has to think about (IC-958): the organisation's signing
/// key on its owners' and admins' computers, permission to publish asked
/// for and given in the background, withdrawal when someone leaves.
mod signing;
pub use signing::{
    automatic as automatic_permission, finish_background, in_background,
    permission as publish_permission, Permission, EXIT_WAITING,
};

/// A role in an organisation, exactly as the hub names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Role {
    Owner,
    Admin,
    Developer,
    Reviewer,
    Recipient,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Role::Owner => "owner",
            Role::Admin => "admin",
            Role::Developer => "developer",
            Role::Reviewer => "reviewer",
            Role::Recipient => "recipient",
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum OrgCommand {
    /// Create an organisation. You become its first owner.
    Create {
        /// Its short name: 2 to 39 lower-case letters, digits and dashes,
        /// starting with a letter. It cannot be taken twice.
        slug: String,
        /// The name people see, e.g. "Acme". Defaults to the slug.
        #[arg(long)]
        name: Option<String>,
    },
    /// The organisations you are in, with your role in each.
    List {
        /// The hub's answer as JSON, `[{slug, name, role}]`, for Studio.
        #[arg(long)]
        json: bool,
    },
    /// Your release key for an organisation: made on this machine, kept
    /// here, and named by its public half so an owner can delegate it.
    /// With --delegation, install what the owner sent back; then
    /// `krate publish app.krate --org <slug>` signs with it.
    #[command(name = "release-key")]
    ReleaseKey {
        slug: String,
        /// The delegation file an owner wrote with `krate delegate`.
        #[arg(long, value_name = "FILE")]
        delegation: Option<std::path::PathBuf>,
    },
    /// Who is in an organisation, and your own role in it.
    Show { slug: String },
    /// Invite one GitHub account, with one role. The invitation works once,
    /// only for that account, for 7 days.
    Invite {
        slug: String,
        /// The GitHub login being invited. Give this or --email, not both.
        #[arg(required_unless_present = "email", conflicts_with = "email")]
        login: Option<String>,
        /// Invite a work email instead: whoever proves that inbox (an email
        /// link, or a Google sign-in) accepts it.
        #[arg(long, value_name = "ADDRESS")]
        email: Option<String>,
        #[arg(long, value_enum)]
        role: Role,
    },
    /// Cancel an invitation that has not been used yet.
    #[command(name = "cancel-invite")]
    CancelInvite { slug: String, code: String },
    /// Accept an invitation sent to you.
    Join { slug: String, code: String },
    /// Change someone's role. Name them by GitHub login or by email.
    Role {
        slug: String,
        login: String,
        #[arg(value_enum)]
        role: Role,
    },
    /// Remove someone, or yourself. Their access to the organisation's apps
    /// ends at once; their own apps and account are untouched.
    Remove {
        slug: String,
        /// A GitHub login, or an email address.
        login: String,
    },
    /// Register (or rotate) the organisation's root public key: the
    /// publisher root that `krate delegate` uses. Give the public key (64 hex
    /// characters in a file) or the root's key file, of which only the public
    /// half leaves this machine.
    #[command(name = "root-key")]
    RootKey {
        slug: String,
        /// The root's public key file, or its PKCS#8 key file. With --new,
        /// where the new root key is written.
        key: std::path::PathBuf,
        /// Make a new root key at that path first (it refuses to overwrite
        /// one), then register its public half. The key stays on this
        /// computer: keep it where your company keeps signing keys.
        #[arg(long)]
        new: bool,
        /// The signed list `krate revoke` wrote with this root: the hub
        /// refuses anything those release keys signed, from now on.
        #[arg(long, value_name = "FILE")]
        revocations: Option<std::path::PathBuf>,
    },
    /// Approve a candidate by its exact digest, so it becomes a release.
    /// You cannot approve your own upload.
    Approve {
        slug: String,
        digest: String,
        /// The new capabilities you checked, by their exact names
        /// (`krate org candidates` lists them): an update that asks for more
        /// than the version it replaces is approved only with each named.
        #[arg(long, value_delimiter = ',', value_name = "CAPABILITY")]
        accept: Vec<String>,
    },
    /// The organisation's releases, with who uploaded and who approved each.
    Releases { slug: String },
    /// Every candidate, waiting or released, with who uploaded and who
    /// approved it.
    Candidates { slug: String },
    /// The organisation's audit: who did what, and every refusal, with the
    /// hub's check that the chain of events is unbroken.
    Audit { slug: String },
    /// Withdraw a release, choose which release of an app opens, or limit
    /// who may open one.
    Release {
        #[command(subcommand)]
        command: ReleaseCommand,
    },
    /// Groups of members, like `finance`, that a release can be limited to.
    Group {
        #[command(subcommand)]
        command: GroupCommand,
    },
    /// Company connections: an API's address and credential, set up once,
    /// that the organisation's apps use without ever holding the credential.
    Connection {
        #[command(subcommand)]
        command: connection::ConnectionCommand,
    },
    /// Your company's apps: each one you may open, with what it can do.
    /// With no organisation named, every one you are in, the one your
    /// computer belongs to first. Apps that may be used offline are kept
    /// ready on this computer, so they open at once and without a
    /// connection while their offline access lasts.
    Apps {
        /// One organisation's apps only.
        slug: Option<String>,
        /// Open this app (its id, from the list) as it is released now:
        /// `krate org apps acme --open com.acme.expenses`.
        #[arg(long, value_name = "APP_ID")]
        open: Option<String>,
        /// The list as JSON, for Studio.
        #[arg(long)]
        json: bool,
        /// With --open: open it in its own window, the way a double-click
        /// does (`krate launch`), as Studio's Open does.
        #[arg(long, requires = "open")]
        window: bool,
        /// Passed on to `krate run` when opening, after `--`.
        #[arg(last = true, hide = true)]
        run_args: Vec<String>,
    },
    /// The organisation's key: its apps' data is sealed with it, and only
    /// its members' devices hold it. Krate's hub stores ciphertext only.
    Keys {
        #[command(subcommand)]
        command: KeysCommand,
    },
    /// Do this computer's share for your organisations now: pass their keys
    /// to new devices, approve people waiting to publish, withdraw the
    /// rights of people who left. Krate does this by itself after every
    /// `krate org` command, while an organisation's app runs, and every
    /// minute while Studio is open.
    Sync {
        /// Say only what was done.
        #[arg(long)]
        quiet: bool,
    },
    /// Make this computer one of an organisation's devices (what Studio
    /// does for a krate://org/setup link): an owner of an organisation that
    /// has no keys yet makes them here; anyone else's computer registers and
    /// takes what it may hold from the devices that already do.
    Setup { slug: String },
}

impl OrgCommand {
    /// The organisation the command is about, when there is one.
    fn slug(&self) -> Option<&str> {
        use OrgCommand::*;
        match self {
            Create { slug, .. }
            | ReleaseKey { slug, .. }
            | Show { slug }
            | Invite { slug, .. }
            | CancelInvite { slug, .. }
            | Join { slug, .. }
            | Role { slug, .. }
            | Remove { slug, .. }
            | RootKey { slug, .. }
            | Approve { slug, .. }
            | Releases { slug }
            | Candidates { slug }
            | Audit { slug }
            | Setup { slug } => Some(slug),
            Release { command } => Some(match command {
                ReleaseCommand::Withdraw { slug, .. }
                | ReleaseCommand::Current { slug, .. }
                | ReleaseCommand::Audience { slug, .. } => slug,
            }),
            Group { command } => Some(match command {
                GroupCommand::Create { slug, .. }
                | GroupCommand::Add { slug, .. }
                | GroupCommand::Remove { slug, .. }
                | GroupCommand::List { slug } => slug,
            }),
            Keys { command } => Some(match command {
                KeysCommand::Init { slug }
                | KeysCommand::Show { slug }
                | KeysCommand::Recover { slug, .. } => slug,
            }),
            Apps { slug, .. } => slug.as_deref(),
            Connection { .. } | List { .. } | Sync { .. } => None,
        }
    }
}

/// `krate org keys ...` (F-316).
#[derive(Debug, Subcommand)]
pub enum KeysCommand {
    /// Make the organisation's key on this computer (an owner's), and show
    /// its recovery code once. `krate org create` does this already.
    Init { slug: String },
    /// Which devices hold the organisation's key, and this one's standing.
    Show { slug: String },
    /// Restore the key on this computer from the recovery code (an owner
    /// or admin, on a new computer). Without --code it is read from the
    /// terminal.
    Recover {
        slug: String,
        #[arg(long)]
        code: Option<String>,
    },
}

/// `krate org release ...` (IC-949 R).
#[derive(Debug, Subcommand)]
pub enum ReleaseCommand {
    /// Withdraw a release. Downloads and offline renewals stop at once, and
    /// every computer refuses it when it next connects; offline access
    /// already given out holds until it runs out. Not undone.
    Withdraw {
        slug: String,
        digest: String,
        /// Why, in a few words: the people who had it are shown this.
        #[arg(long)]
        reason: String,
    },
    /// Which release of an app opens. With a digest, make that approved
    /// release the one that opens (an older one is the rollback).
    Current {
        slug: String,
        /// The app's id, from its manifest (e.g. com.acme.ledger).
        app_id: String,
        digest: Option<String>,
    },
    /// Who may open a release: some groups, or everyone. Owners, admins and
    /// reviewers always may.
    Audience {
        slug: String,
        digest: String,
        /// The groups, comma-separated: --groups finance,ops.
        #[arg(
            long,
            value_delimiter = ',',
            required_unless_present = "everyone",
            conflicts_with = "everyone"
        )]
        groups: Vec<String>,
        /// Everyone in the organisation again.
        #[arg(long)]
        everyone: bool,
    },
}

/// `krate org group ...` (IC-949 R).
#[derive(Debug, Subcommand)]
pub enum GroupCommand {
    /// Make a group: lower-case letters, digits and dashes, like finance.
    Create { slug: String, name: String },
    /// Put a member in a group. Name them by GitHub login or by email.
    Add {
        slug: String,
        name: String,
        login: String,
    },
    /// Take a member out of a group.
    Remove {
        slug: String,
        name: String,
        login: String,
    },
    /// The organisation's groups and who is in each.
    List { slug: String },
}

/// The hub's refusal: the contract's stable reason, and the hub's
/// own sentence. Shown to a person only through [`words`].
#[derive(Debug)]
pub struct HubRefusal {
    pub reason: String,
    pub message: String,
}

impl std::fmt::Display for HubRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the hub refused: {}", self.message)
    }
}

impl std::error::Error for HubRefusal {}

/// Turn a hub's error answer into an error a person can read: a
/// [`HubRefusal`] when it carries a stable reason, otherwise the status and
/// the hub's message -- never the raw body when it is JSON.
pub fn refusal_of(status: u16, response: ureq::Response) -> anyhow::Error {
    let body = response.into_string().unwrap_or_default();
    match serde_json::from_str::<serde_json::Value>(&body) {
        Ok(answer) => match answer["error"].as_str() {
            Some(reason) => HubRefusal {
                reason: reason.to_string(),
                message: answer["message"]
                    .as_str()
                    .unwrap_or("no reason was given")
                    .to_string(),
            }
            .into(),
            None => match answer["message"].as_str().or(answer["problem"].as_str()) {
                Some(message) => anyhow::anyhow!("the hub answered HTTP {status}: {message}"),
                None => anyhow::anyhow!("the hub answered HTTP {status}"),
            },
        },
        Err(_) => {
            let first = body.lines().next().unwrap_or("").trim();
            let first: String = first.chars().take(200).collect();
            if first.is_empty() {
                anyhow::anyhow!("the hub answered HTTP {status}")
            } else {
                anyhow::anyhow!("the hub answered HTTP {status}: {first}")
            }
        }
    }
}

/// The hub to talk to: `--hub`, else `KRATE_HUB_URL`, else the public hub,
/// and only the organisation's hub when its policy pins one. One function
/// for every caller, so a pin cannot be missed (IC-949 [hub]).
pub fn hub_url(hub_override: Option<&str>) -> Result<String> {
    crate::fleet::hub(hub_override)
}

/// One key per command: the same command retried is the same request.
fn new_idempotency_key() -> String {
    use sha2::{Digest, Sha256};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seed = format!(
        "{nanos}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    );
    Sha256::digest(seed.as_bytes())
        .iter()
        .take(16)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A path segment, percent-encoded, so a slug, login or code can never walk
/// out of the route it was put in. One made only of dots is refused: a URL
/// parser reads it as "up a level" however it is encoded.
fn seg(text: &str) -> Result<String> {
    if text.is_empty() || text.bytes().all(|b| b == b'.') {
        anyhow::bail!("\"{text}\" is not a name the hub can look up");
    }
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    Ok(out)
}

enum Body<'a> {
    Empty,
    Json(serde_json::Value),
    Bundle(&'a [u8], Vec<(&'static str, String)>),
}

/// The signed-in person and the hub they talk to.
struct Client {
    hub: String,
    identity: github_auth::Identity,
    key: String,
    agent: ureq::Agent,
}

impl Client {
    fn new(hub: String, identity: github_auth::Identity) -> Self {
        let agent = krate_http::builder(&hub)
            .timeout_connect(std::time::Duration::from_secs(20))
            .timeout_read(std::time::Duration::from_secs(120))
            .build();
        Client {
            hub,
            identity,
            key: new_idempotency_key(),
            agent,
        }
    }

    fn me(&self) -> String {
        self.identity.login.to_ascii_lowercase()
    }

    /// The principal this sign-in stands for, as the hub would name it.
    fn my_principal(&self) -> String {
        principal_of(&self.identity.login)
    }

    /// Send one request; a dropped connection is retried with the SAME key,
    /// which is what makes the retry safe.
    fn send(&self, method: &str, path: &str, body: &Body) -> Result<serde_json::Value> {
        let url = format!("{}{path}", self.hub);
        let mut last = None;
        for attempt in 0..3 {
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_millis(400 * attempt));
            }
            let request = self
                .agent
                .request(method, &url)
                .set("Authorization", &format!("Bearer {}", self.identity.token))
                .set("Idempotency-Key", &self.key)
                .set("Accept", "application/json");
            let sent = match body {
                Body::Empty => request.call(),
                Body::Json(value) => request
                    .set("Content-Type", "application/json")
                    .send_string(&value.to_string()),
                Body::Bundle(bytes, headers) => {
                    let mut request = request.set("Content-Type", "application/octet-stream");
                    for (name, value) in headers {
                        request = request.set(name, value);
                    }
                    request.send_bytes(bytes)
                }
            };
            match sent {
                Ok(response) => {
                    if response.status() == 204 {
                        return Ok(serde_json::Value::Null);
                    }
                    let text = response
                        .into_string()
                        .context("could not read the hub's answer")?;
                    if text.trim().is_empty() {
                        return Ok(serde_json::Value::Null);
                    }
                    return serde_json::from_str(&text).map_err(|_| {
                        anyhow::anyhow!(
                            "the hub at {} answered with something that is not a Krate hub's \
                             answer. Check --hub or KRATE_HUB_URL.",
                            self.hub
                        )
                    });
                }
                Err(ureq::Error::Status(status, response)) => {
                    return Err(refusal_of(status, response));
                }
                Err(ureq::Error::Transport(err)) => last = Some(err),
            }
        }
        Err(anyhow::Error::new(Unreachable {
            hub: self.hub.clone(),
            why: last.map(|err| err.to_string()).unwrap_or_default(),
        }))
    }
}

/// The hub did not answer at all: no connection, or nothing listening.
/// Kept apart from a refusal, so a command that works offline (`krate org
/// apps`) can tell "offline" from "no".
#[derive(Debug)]
pub struct Unreachable {
    pub hub: String,
    pub why: String,
}

impl std::fmt::Display for Unreachable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "could not reach the hub at {}: {}. Is it running? Point --hub or KRATE_HUB_URL at \
             a reachable one.",
            self.hub, self.why
        )
    }
}

impl std::error::Error for Unreachable {}

/// The signed-in identity, signing in first when a person is there to do
/// it -- the same as `krate publish`.
pub fn identity_or_sign_in(why: &str) -> Result<github_auth::Identity> {
    if let Some(identity) = github_auth::current() {
        return Ok(identity);
    }
    use std::io::IsTerminal;
    if !std::io::stdout().is_terminal() {
        anyhow::bail!("you are not signed in: run `krate account login`, then run this again");
    }
    println!("{why}");
    github_auth::sign_in().map_err(|err| anyhow::anyhow!("could not sign in: {err}"))
}

/// What a command was trying to do, for the words of a refusal.
#[derive(Clone, Copy)]
pub enum Action<'a> {
    Create,
    List,
    Show,
    Invite(Role),
    CancelInvite,
    Join,
    Role(&'a str, Role),
    Remove(&'a str),
    RootKey,
    Publish,
    Approve(&'a str),
    Releases,
    Candidates,
    Audit,
    Withdraw,
    SetCurrent,
    Audience,
    Groups,
    Connection,
    Apps,
}

/// A refusal in plain words: what the reason means here, and the next step.
pub fn words(refusal: &HubRefusal, org: &str, me: &str, action: Action) -> String {
    let owner_or_admin = format!("an owner or admin of {org}");
    let an_owner = format!("an owner of {org}");
    match refusal.reason.as_str() {
        "not-signed-in" => {
            "you are not signed in: run `krate account login`, then run this again".to_string()
        }
        "not-a-member" => {
            let invite = if me.contains('@') {
                format!("--email {me}")
            } else {
                me.to_string()
            };
            format!(
                "{me} is not a member of {org}: ask {owner_or_admin} to invite {me} \
                 (krate org invite {org} {invite} --role <role>)"
            )
        }
        "removed" => format!(
            "you were removed from {org}, so you can no longer act in it: ask {an_owner} \
             to invite you again if that is a mistake"
        ),
        "role-cannot" => {
            let (what, who) = match action {
                Action::Create => ("create it".to_string(), an_owner.clone()),
                Action::List => ("list organisations".to_string(), an_owner.clone()),
                Action::Show => (format!("see {org}'s members"), owner_or_admin.clone()),
                Action::Invite(Role::Owner | Role::Admin) => {
                    ("invite an owner or admin".to_string(), an_owner.clone())
                }
                Action::Invite(_) => ("invite people".to_string(), owner_or_admin.clone()),
                Action::CancelInvite => ("cancel invitations".to_string(), owner_or_admin.clone()),
                Action::Join => ("join with that invitation".to_string(), an_owner.clone()),
                Action::Role(login, Role::Owner) => {
                    (format!("make {login} an owner"), an_owner.clone())
                }
                Action::Role(login, Role::Admin) => {
                    (format!("make {login} an admin"), an_owner.clone())
                }
                Action::Role(login, _) => (
                    format!("change {login}'s role"),
                    format!(
                        "{an_owner} (an admin moves people only between developer, reviewer \
                         and recipient)"
                    ),
                ),
                Action::Remove(login) => (format!("remove {login}"), owner_or_admin.clone()),
                Action::RootKey => (format!("set {org}'s root key"), an_owner.clone()),
                Action::Publish => (
                    format!("publish to {org}"),
                    format!("{owner_or_admin} to make you a developer"),
                ),
                Action::Approve(_) => (
                    "approve uploads".to_string(),
                    format!("a reviewer in {org}"),
                ),
                Action::Releases => (format!("list {org}'s releases"), owner_or_admin.clone()),
                Action::Candidates => (format!("list {org}'s candidates"), owner_or_admin.clone()),
                Action::Audit => (format!("read {org}'s audit"), owner_or_admin.clone()),
                Action::Withdraw => (
                    format!("withdraw {org}'s releases"),
                    format!("an owner, admin or reviewer of {org}"),
                ),
                Action::SetCurrent => (
                    "choose which release of an app opens".to_string(),
                    owner_or_admin.clone(),
                ),
                Action::Audience => (
                    "choose who may open a release".to_string(),
                    owner_or_admin.clone(),
                ),
                Action::Groups => (format!("manage {org}'s groups"), owner_or_admin.clone()),
                Action::Connection => (
                    format!("set up or change {org}'s connections"),
                    owner_or_admin.clone(),
                ),
                Action::Apps => (format!("open {org}'s apps"), owner_or_admin.clone()),
            };
            format!("your role in {org} cannot {what}: ask {who}")
        }
        "last-owner" => format!(
            "that would leave {org} with no owner, so nothing changed: make someone else an \
             owner first (krate org role {org} <login> owner)"
        ),
        "self-approval" => {
            format!("you can't approve your own upload: ask a reviewer in {org}")
        }
        "new-capabilities" => format!(
            "nothing was approved: {}. `krate org candidates {org}` lists what is new; approve \
             with --accept and each one's name",
            refusal.message
        ),
        "digest-mismatch" => match action {
            Action::Approve(digest) => format!(
                "nothing was approved: {org} has no candidate with the digest {}. Approve the \
                 digest `krate publish --org {org}` printed for the upload you checked; \
                 `krate org candidates {org}` lists them",
                short(digest)
            ),
            _ => "the hub received different bytes than were sent, so nothing was taken: \
                  run the same command again"
                .to_string(),
        },
        "not-signed-for-this-org" => {
            // The hub says which rule it failed: no signature that chains to
            // the root, a delegation already expired, a withdrawn key.
            let why = Some(refusal.message.trim())
                .filter(|m| !m.is_empty() && *m != "no reason was given")
                .map(str::to_string)
                .unwrap_or_else(|| {
                    format!(
                        "it is not signed by {org}'s root key, or by a release key that root \
                         delegated"
                    )
                });
            let next = format!(
                "{org} takes only apps signed by its root key, or by a release key that root \
                 delegated which has not expired and was not withdrawn. Ask {an_owner} for a \
                 current delegation (krate delegate --root <{org}-root.pkcs8> --key \
                 <your-release.pkcs8> --namespace '{org}/*' -o {org}-release.json), sign with \
                 it (krate sign app.krate --org {org} --key <your-release.pkcs8> \
                 --delegation {org}-release.json), and publish again"
            );
            match action {
                Action::Approve(_) => format!(
                    "nothing was approved: {why}. Ask the uploader to sign it again with a \
                     current release key and publish it again; then approve the new digest"
                ),
                Action::RootKey => format!(
                    "the root key was not changed: {why}. Give the list `krate revoke` wrote \
                     with this same root key"
                ),
                _ => format!("{org} did not take it: {why}. {next}"),
            }
        }
        "invalid-bundle" => format!(
            "the hub could not read this as a Krate app, so nothing was taken: {}. Pack it \
             again with `krate pack`, then publish again",
            refusal.message
        ),
        "not-marked-for-this-org" => format!(
            "{org} did not take it: it is not marked as {org}'s private app under a signature \
             that holds. Mark and sign it for {org} (krate sign app.krate --org {org} --key \
             <your-release.pkcs8> --delegation {org}-release.json), and publish again"
        ),
        "private-app" => format!(
            "this app carries an organisation's mark, so it never goes to the public gallery: {}",
            Some(refusal.message.as_str())
                .filter(|m| !m.is_empty())
                .unwrap_or("this app is private: publish it with krate publish --org <org>")
        ),
        "bytes-held-elsewhere" => format!(
            "these exact bytes are already on the hub outside {org} -- published publicly, or \
             held by another organisation -- so they cannot be {org}'s private release: change \
             the app (a new version number is enough), sign it again and publish again"
        ),
        "too-large" => format!(
            "the hub would not take it in one request: {}. Run the same publish again with this \
             version of Krate, which sends big apps in parts",
            refusal.message
        ),
        "store-failed" => format!(
            "the hub could not store it ({}), so nothing was taken: run the same command again",
            refusal.message
        ),
        "bad-request" => format!(
            "the hub could not read the request ({}): update Krate, then run this again",
            refusal.message
        ),
        "orgs-off" => "organisations are switched off on this hub: use a hub that has them \
             (--hub or KRATE_HUB_URL), or ask whoever runs this one"
            .to_string(),
        "invite-expired" => format!(
            "that invitation has run out (they last 7 days): ask {owner_or_admin} for a new one"
        ),
        "invite-used" => format!(
            "that invitation was already used: ask {owner_or_admin} for a new one, and tell \
             them if it was not you who used it"
        ),
        "invite-for-someone-else" => format!(
            "that invitation is for a different GitHub account, not {me}: sign in as the \
             invited account with `krate account login`, or ask {owner_or_admin} to invite {me}"
        ),
        "invite-revoked" => {
            format!("that invitation was cancelled: ask {owner_or_admin} for a new one")
        }
        "slug-taken" => format!(
            "an organisation called {org} already exists, so nothing was created: pick another name"
        ),
        "slug-invalid" => format!(
            "\"{org}\" cannot name an organisation: use 2 to 39 lower-case letters, digits and \
             dashes, starting with a letter (like acme or acme-labs)"
        ),
        "idempotency-key-reused" => "the hub already took a different request under this \
             command's key, so nothing changed: run the command again"
            .to_string(),
        "no-such-org" => format!(
            "there is no organisation called {org} on this hub: check the name, or create it \
             with `krate org create {org}`"
        ),
        "release-withdrawn" => match action {
            Action::Withdraw => format!(
                "nothing changed: that release is already withdrawn ({}). A withdrawal is not \
                 undone; to release the app again, publish a new version and approve it",
                hub_detail(refusal)
            ),
            Action::Approve(_) => format!(
                "nothing was approved: these bytes were withdrawn, and a withdrawal is not \
                 undone. Publish a new version with `krate publish app.krate --org {org}` and \
                 approve that"
            ),
            _ => format!(
                "that release was withdrawn, so it cannot be chosen: pick another approved \
                 release (`krate org releases {org}` lists them). The hub said: {}",
                hub_detail(refusal)
            ),
        },
        "not-in-audience" => format!(
            "{}: ask {owner_or_admin} to add you to one of its groups (krate org group add {org} \
             <group> {me})",
            // The hub's sentence ends in its own next step; this one says it.
            hub_detail(refusal).split(" -- ").next().unwrap_or_default()
        ),
        "reason-required" => "nothing was withdrawn: say why with --reason \"...\", in up to \
             300 characters. The people who had it are shown the reason"
            .to_string(),
        "no-such-group" => format!(
            "nothing changed: {}. Make it first with `krate org group create {org} <name>`",
            hub_detail(refusal)
        ),
        "group-taken" => format!(
            "nothing changed: {org} already has a group by that name. Add people to it with \
             `krate org group add {org} <name> <login>`"
        ),
        "group-invalid" => "nothing changed: a group name is 1 to 39 lower-case letters, \
             digits and dashes, starting with a letter (like finance or field-ops)"
            .to_string(),
        "no-such-connection" => format!(
            "nothing changed: {org} has no connection by that name. `krate org connection list \
             {org}` lists them; {owner_or_admin} makes one with `krate org connection create`"
        ),
        "connection-taken" => format!(
            "nothing changed: {org} already has a connection by that name. Give it a new \
             credential with `krate org connection replace {org} <name>`, or pick another name"
        ),
        "connection-invalid" => format!(
            "nothing changed: {}. `krate org connection create --help` shows each part",
            hub_detail(refusal)
        ),
        "credential-required" => "nothing changed: moving a connection to another address \
             needs its credential again, because the stored one is never sent anywhere new: \
             remove it and create it again with the new address"
            .to_string(),
        "connections-off" => format!(
            "company connections are switched off on this hub: {}",
            hub_detail(refusal)
        ),
        // The organisation keyring (F-316): a credential is encrypted to it.
        "no-org-key" => format!(
            "nothing was sent: {org} has not turned on encryption yet, and a credential is always \
             encrypted so that only {org}'s computers can read it. An owner turns it on once: \
             `krate org keys init {org}`"
        ),
        "key-rotated" => format!(
            "nothing was saved: {org}'s encryption was just renewed after someone left. Run the \
             same command again and the credential is encrypted the new way"
        ),
        "connection-not-in-audience" | "app-not-allowed" | "connection-not-requested" => {
            format!(
                "the app does not get the connection's credential, so nothing is sent with it \
                 ({}): ask {owner_or_admin} to change who and which apps may use it",
                hub_detail(refusal).split(" -- ").next().unwrap_or_default()
            )
        }
        // Signing nobody has to think about (IC-958).
        "needs-approval" => format!(
            "{org} approves new publishers by hand: {owner_or_admin} approves you in the \
             dashboard (https://krate.tech/org/?o={org}&tab=people), then publish again"
        ),
        "no-signing-key" => format!(
            "{org} cannot sign apps by itself yet: {an_owner} opens Krate once (or runs `krate \
             org sync`), and Krate sets it up"
        ),
        "signing-key-not-held" => format!(
            "this computer cannot sign for {org} yet: another owner's or admin's computer passes \
             it on the next time it runs Krate"
        ),
        "signing-key-exists" => format!("nothing changed: {}", hub_detail(refusal)),
        "not-a-publisher" | "slip-invalid" => format!(
            "nothing changed: {}. Publish again and Krate asks {org} for you",
            hub_detail(refusal)
        ),
        _ => format!("the hub refused: {}", refusal.message),
    }
}

/// The hub's own sentence for a refusal, or a placeholder when it gave
/// none.
fn hub_detail(refusal: &HubRefusal) -> String {
    Some(refusal.message.trim())
        .filter(|m| !m.is_empty() && *m != "no reason was given")
        .map(str::to_string)
        .unwrap_or_else(|| "the hub gave no detail".to_string())
}

/// An audit target with any digest in it shortened: `candidate:<64 hex>`
/// reads as `candidate:1a2b3c4d5e6f`.
fn spoken_target(target: &str) -> String {
    target
        .split(':')
        .map(|part| {
            if part.len() == 64 && part.bytes().all(|b| b.is_ascii_hexdigit()) {
                short(part)
            } else {
                part
            }
        })
        .collect::<Vec<_>>()
        .join(":")
}

/// The principal a person is named by on the command line: an address
/// with `@` is `email:<address>`, anything else a GitHub login,
/// `github:<login>`. Both lower-case. An explicit `github:` or `email:`
/// prefix is kept.
pub fn principal_of(name: &str) -> String {
    let name = name.trim().to_ascii_lowercase();
    if name.starts_with("github:") || name.starts_with("email:") {
        name
    } else if name.contains('@') {
        format!("email:{name}")
    } else {
        format!("github:{name}")
    }
}

/// A principal as a person reads it: `alice (GitHub)`, `bob@corp.com
/// (email)`. Takes the hub's `{principal, display, kind}` object, a bare
/// principal string, or (from an older hub) a plain login.
pub fn spoken_principal(value: &serde_json::Value) -> String {
    let label = |kind: &str| match kind {
        "github" => "GitHub".to_string(),
        other => other.to_string(),
    };
    if let Some(principal) = value.as_str() {
        return match principal.split_once(':') {
            Some((kind @ ("github" | "email"), who)) => format!("{who} ({})", label(kind)),
            _ => principal.to_string(),
        };
    }
    let principal = text(value, "principal");
    let (kind_of, who_of) = principal.split_once(':').unwrap_or(("", principal));
    let kind = Some(text(value, "kind"))
        .filter(|k| !k.is_empty())
        .unwrap_or(kind_of);
    let display = Some(text(value, "display"))
        .filter(|d| !d.is_empty())
        .or(Some(who_of).filter(|w| !w.is_empty()))
        .unwrap_or_else(|| text(value, "login"));
    if kind.is_empty() {
        display.to_string()
    } else {
        format!("{display} ({})", label(kind))
    }
}

/// Is this origin this machine (`http://127.0.0.1:8787`, a `wrangler dev`
/// hub or a copy of the site served here)?
fn loopback(url: &str) -> bool {
    origin(url).is_some_and(|o| {
        let host = o.split_once("://").map_or("", |(_, h)| h);
        let bare = host.rsplit_once(':').map_or(host, |(h, _)| h);
        matches!(bare, "127.0.0.1" | "localhost" | "[::1]")
    })
}

/// Is this Krate's own hub, whose site is krate.tech?
fn krates_hub(hub: &str) -> bool {
    matches!(
        origin(hub).as_deref(),
        Some("https://hub.krate.tech") | Some("https://krate.tech")
    )
}

/// The join page for an invitation on `hub`, as the hub's answer gives it
/// or as the addendum spells it. A link must go to a site that talks to
/// that hub: krate.tech's join page talks to Krate's hub only, so a local
/// or company hub that names no site of its own gets no link -- its
/// invitation is taken with `krate org join` (K-1105). A hub on this
/// machine may name a site on this machine (`SITE_BASE`), over http.
fn join_url(answer: &serde_json::Value, slug: &str, code: &str, hub: &str) -> Option<String> {
    for key in ["join_url", "url"] {
        let Some(url) = answer[key].as_str() else {
            continue;
        };
        let reachable = url.starts_with("https://") || (loopback(hub) && loopback(url));
        let krate_site = origin(url).as_deref() == Some("https://krate.tech");
        if reachable && (krates_hub(hub) || !krate_site) {
            return Some(url.to_string());
        }
    }
    if !krates_hub(hub) {
        return None;
    }
    let q = |text: &str| {
        text.bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect::<String>()
    };
    Some(format!(
        "https://krate.tech/join/?o={}&c={}",
        q(slug),
        q(code)
    ))
}

/// The first twelve characters of a digest, which is how one is spoken.
fn short(digest: &str) -> &str {
    &digest[..digest.len().min(12)]
}

/// `Err` carrying a [`HubRefusal`] becomes the words for it; anything else
/// passes through unchanged.
fn worded<T>(result: Result<T>, org: &str, me: &str, action: Action) -> Result<T> {
    result.map_err(|err| match err.downcast_ref::<HubRefusal>() {
        Some(refusal) => anyhow::anyhow!("{}", words(refusal, org, me, action)),
        None => err,
    })
}

/// A time the hub gave: an RFC 3339 string is shown as it is; unix seconds
/// (or milliseconds) become a UTC date.
pub(crate) fn when(value: &serde_json::Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_string();
    }
    let Some(mut secs) = value.as_u64() else {
        return String::new();
    };
    if secs > 100_000_000_000 {
        secs /= 1000;
    }
    let days = (secs / 86_400) as i64;
    let rest = secs % 86_400;
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        rest / 3600,
        (rest % 3600) / 60
    )
}

fn text<'a>(value: &'a serde_json::Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or("")
}

/// `krate org ...`.
pub fn run(command: OrgCommand, hub_override: Option<&str>) -> Result<u8> {
    // A release key is made and kept on this machine; nothing to sign in for.
    if let OrgCommand::ReleaseKey { slug, delegation } = &command {
        return release_key_command(slug, delegation.as_deref());
    }
    let hub = hub_url(hub_override)?;
    // The company's apps work signed out (it says how to sign in) and
    // offline (from what this computer kept), so they sign in their own way.
    if let OrgCommand::Apps {
        slug,
        open,
        json,
        window,
        run_args,
    } = command
    {
        return apps::run(
            hub,
            apps::Ask {
                slug,
                open,
                json,
                window,
                run_args,
            },
        );
    }
    let identity = identity_or_sign_in("Organisations know people by their sign-in.")?;
    let client = Client::new(hub, identity);
    let slug = command.slug().map(str::to_string);
    let code = run_command(&client, command)?;
    // This computer's share for the organisation, after every command
    // about one (IC-958): quiet unless it did something.
    if let Some(slug) = slug {
        signing::after_command(&client, &slug);
    }
    Ok(code)
}

fn run_command(client: &Client, command: OrgCommand) -> Result<u8> {
    let me = client.me();
    match command {
        OrgCommand::Create { slug, name } => {
            let name = name.unwrap_or_else(|| slug.clone());
            let answer = worded(
                client.send(
                    "POST",
                    "/orgs",
                    &Body::Json(serde_json::json!({ "slug": slug, "name": name })),
                ),
                &slug,
                &me,
                Action::Create,
            )?;
            let org = &answer["org"];
            let slug = Some(text(org, "slug"))
                .filter(|s| !s.is_empty())
                .unwrap_or(&slug);
            let name = Some(text(org, "name"))
                .filter(|s| !s.is_empty())
                .unwrap_or(&name);
            println!("created {slug} (\"{name}\"). You are its owner.");
            // The key its apps' data is sealed with, made here, on the
            // owner's computer (F-316). A hub that cannot take it yet is
            // not a failed create: the line to run is said.
            if let Err(err) = keys_init(client, slug) {
                println!("The organisation's key was not made yet ({err}). Make it with:");
                println!("  krate org keys init {slug}");
            }
            // And the key its apps are signed with, held by its owners' and
            // admins' computers (IC-958): nobody handles it.
            match signing::ring_init(client, slug) {
                Ok(()) => println!(
                    "Every app {name} releases is signed by {name}: this computer does it, and \
                     the computers of {name}'s owners and admins do it too. Nobody has to handle \
                     anything."
                ),
                Err(err) => println!(
                    "{name} cannot sign apps by itself yet ({err}): run `krate org sync` to try again."
                ),
            }
            println!("Next:");
            println!("  release an app:  krate publish app.krate --org {slug}");
            println!("  invite people:   krate org invite {slug} <github-login> --role developer");
            Ok(0)
        }
        OrgCommand::ReleaseKey { .. } => unreachable!("handled before signing in"),
        OrgCommand::List { json } => {
            let answer = worded(
                client.send("GET", "/orgs", &Body::Empty),
                "",
                &me,
                Action::List,
            )?;
            let orgs = answer
                .as_array()
                .cloned()
                .or_else(|| answer["orgs"].as_array().cloned())
                .unwrap_or_default();
            if json {
                println!("{}", serde_json::json!(orgs));
                return Ok(0);
            }
            if orgs.is_empty() {
                println!("you're in no organisations yet: krate org create <name>");
                return Ok(0);
            }
            println!(
                "you are in {} organisation{}:",
                orgs.len(),
                if orgs.len() == 1 { "" } else { "s" }
            );
            for org in &orgs {
                let slug = text(org, "slug");
                let name = Some(text(org, "name"))
                    .filter(|n| !n.is_empty())
                    .unwrap_or(slug);
                println!("  {slug:<20} {name:<28} {}", text(org, "role"));
            }
            println!("See one with `krate org show <slug>`.");
            Ok(0)
        }
        OrgCommand::Show { slug } => {
            let answer = worded(
                client.send("GET", &format!("/orgs/{}", seg(&slug)?), &Body::Empty),
                &slug,
                &me,
                Action::Show,
            )?;
            let org = &answer["org"];
            let name = Some(text(org, "name"))
                .filter(|s| !s.is_empty())
                .unwrap_or(&slug);
            println!("{slug} -- {name}");
            // The whole key, for a machine policy's `[trust] roots` (IC-949
            // P2): the short form is for reading, this is for copying.
            match org["root_key"].as_str().filter(|k| !k.is_empty()) {
                Some(key) => println!("  root key {key}"),
                None => println!(
                    "  no root key yet: an owner registers one with `krate org root-key {slug} \
                     <root.pub>`"
                ),
            }
            let my_role = text(&answer["me"], "role");
            if !my_role.is_empty() {
                let mine = spoken_principal(&answer["me"]);
                let mine = if mine.is_empty() { me.clone() } else { mine };
                println!("  you are {mine}, {my_role}");
            }
            let members = answer["members"].as_array().cloned().unwrap_or_default();
            let active = members
                .iter()
                .filter(|m| matches!(text(m, "state"), "" | "active"))
                .count();
            println!(
                "  {active} {}:",
                if active == 1 { "person" } else { "people" }
            );
            for member in &members {
                let state = text(member, "state");
                let state = if state.is_empty() || state == "active" {
                    String::new()
                } else {
                    format!("  ({state})")
                };
                let since = when(&member["since"]);
                let since = if since.is_empty() {
                    String::new()
                } else {
                    format!("  since {since}")
                };
                println!(
                    "    {:<28} {:<10}{since}{state}",
                    spoken_principal(member),
                    text(member, "role")
                );
            }
            Ok(0)
        }
        OrgCommand::Invite {
            slug,
            login,
            email,
            role,
        } => {
            // Exactly one of the two: a GitHub login or a work email.
            let (body, principal) = match (login, email) {
                (Some(login), None) => {
                    let login = login.trim().to_ascii_lowercase();
                    if login.contains('@') {
                        anyhow::bail!(
                            "{login} looks like an email address: invite it with --email {login}"
                        );
                    }
                    let login = login.trim_start_matches("github:").to_string();
                    (
                        serde_json::json!({ "github": login, "role": role.as_str() }),
                        format!("github:{login}"),
                    )
                }
                (None, Some(email)) => {
                    let email = email.trim().to_ascii_lowercase();
                    let email = email.trim_start_matches("email:").to_string();
                    if !email.contains('@') {
                        anyhow::bail!(
                            "{email} is not an email address: a GitHub login goes without --email"
                        );
                    }
                    (
                        serde_json::json!({ "email": email, "role": role.as_str() }),
                        format!("email:{email}"),
                    )
                }
                _ => anyhow::bail!("name one person: a GitHub login, or --email <address>"),
            };
            let answer = worded(
                client.send(
                    "POST",
                    &format!("/orgs/{}/invites", seg(&slug)?),
                    &Body::Json(body),
                ),
                &slug,
                &me,
                Action::Invite(role),
            )?;
            let code = text(&answer, "code");
            let who = spoken_principal(&serde_json::json!(principal));
            println!("invited {who} to {slug} as {}.", role.as_str());
            let expires = when(&answer["expires_at"]);
            if expires.is_empty() {
                println!("Send them this. It works once, and only for {who}:");
            } else {
                println!("Send them this. It works once, only for {who}, until {expires}:");
            }
            println!("  krate org join {slug} {code}");
            if let Some(link) = join_url(&answer, &slug, code, &client.hub) {
                println!("  or open {link}");
            }
            Ok(0)
        }
        OrgCommand::CancelInvite { slug, code } => {
            worded(
                client.send(
                    "DELETE",
                    &format!("/orgs/{}/invites/{}", seg(&slug)?, seg(&code)?),
                    &Body::Empty,
                ),
                &slug,
                &me,
                Action::CancelInvite,
            )?;
            println!("cancelled that invitation to {slug}. It can no longer be used.");
            Ok(0)
        }
        OrgCommand::Join { slug, code } => {
            let answer = worded(
                client.send(
                    "POST",
                    &format!("/orgs/{}/invites/{}/accept", seg(&slug)?, seg(&code)?),
                    &Body::Empty,
                ),
                &slug,
                &me,
                Action::Join,
            )?;
            let role = text(&answer["membership"], "role");
            let as_who = spoken_principal(&answer["membership"]);
            let as_who = if as_who.is_empty() {
                String::new()
            } else {
                format!(", signed in as {as_who}")
            };
            if role.is_empty() {
                println!("you joined {slug}{as_who}.");
            } else {
                println!("you joined {slug} as {role}{as_who}.");
            }
            if role == "recipient" {
                println!("{slug}'s releases now open for you with `krate run <link>`.");
            }
            Ok(0)
        }
        OrgCommand::Role { slug, login, role } => {
            let principal = principal_of(&login);
            let login = spoken_principal(&serde_json::json!(principal));
            let answer = worded(
                client.send(
                    "PUT",
                    &format!("/orgs/{}/members/{}", seg(&slug)?, seg(&principal)?),
                    &Body::Json(serde_json::json!({ "role": role.as_str() })),
                ),
                &slug,
                &me,
                Action::Role(&login, role),
            )?;
            let now = Some(text(&answer["membership"], "role"))
                .filter(|r| !r.is_empty())
                .unwrap_or(role.as_str());
            println!("{login} is now {now} in {slug}.");
            Ok(0)
        }
        OrgCommand::Remove { slug, login } => {
            let principal = principal_of(&login);
            let login = spoken_principal(&serde_json::json!(principal));
            worded(
                client.send(
                    "DELETE",
                    &format!("/orgs/{}/members/{}", seg(&slug)?, seg(&principal)?),
                    &Body::Empty,
                ),
                &slug,
                &me,
                Action::Remove(&login),
            )?;
            if principal == client.my_principal() {
                println!("you left {slug}. Its apps no longer open for you; your own apps are untouched.");
            } else {
                println!("removed {login} from {slug}.");
                println!(
                    "Their access to {slug}'s apps ended now. Their own apps and account are \
                     untouched, and {slug}'s releases keep their history."
                );
            }
            Ok(0)
        }
        OrgCommand::RootKey {
            slug,
            key,
            new,
            revocations,
        } => {
            if new {
                if key.exists() {
                    anyhow::bail!(
                        "{} already exists, so no new root key was made: register that one \
                         with `krate org root-key {slug} {}`, or name a new file",
                        key.display(),
                        key.display()
                    );
                }
                let pkcs8 = krate_bundle::signing::SigningKey::generate_pkcs8()
                    .map_err(|err| anyhow::anyhow!("could not make a root key: {err}"))?;
                write_private_key(&key, &pkcs8)?;
                println!(
                    "made {slug}'s root key at {}. It never leaves this computer: keep it where \
                     your company keeps signing keys, and make release keys from it with \
                     `krate delegate`.",
                    key.display()
                );
            }
            let (public_key, from_private) = public_key_from(&key)?;
            let mut body = serde_json::json!({ "public_key": public_key });
            if let Some(path) = &revocations {
                body["revocations"] = revocation_list(path, &public_key)?;
            }
            let answer = worded(
                client.send(
                    "PUT",
                    &format!("/orgs/{}/root-key", seg(&slug)?),
                    &Body::Json(body),
                ),
                &slug,
                &me,
                Action::RootKey,
            )?;
            if from_private {
                println!(
                    "read the public half of {}; the key itself stayed here.",
                    key.display()
                );
            }
            // In full: a machine policy's `[trust] roots` takes the whole
            // 64-hex key (IC-949 P2).
            println!("{slug}'s root key is now {public_key}");
            let key = &answer["key"];
            let replaced = key["rotated"].as_bool() == Some(true)
                || key["previous"].as_str().is_some_and(|p| !p.is_empty());
            if replaced {
                println!("This replaced the previous key; the change is in {slug}'s audit.");
            }
            if let Some(withdrawn) = key["revoked_now"].as_u64().filter(|n| *n > 0) {
                println!(
                    "Withdrew {withdrawn} release key{}: {slug} refuses what {} signed, from now on.",
                    if withdrawn == 1 { "" } else { "s" },
                    if withdrawn == 1 { "it" } else { "they" }
                );
            }
            println!(
                "Apps published to {slug} must be signed by it, or by a release key it \
                 delegated (krate delegate --root <root key> --key <release key> \
                 --namespace '{slug}/*' -o {slug}-release.json)."
            );
            Ok(0)
        }
        OrgCommand::Approve {
            slug,
            digest,
            accept,
        } => {
            let digest = digest.trim().to_ascii_lowercase();
            let accept: Vec<String> = accept
                .iter()
                .map(|c| c.trim().to_string())
                .filter(|c| !c.is_empty())
                .collect();
            // What this update adds that a person is asked for (IC-950),
            // said before anything is sent. The hub checks the same; this is
            // so the words come with the exact command. A list this person
            // may not read is the hub's to refuse, with its own words.
            if let Ok(list) = client.send(
                "GET",
                &format!("/orgs/{}/candidates", seg(&slug)?),
                &Body::Empty,
            ) {
                let found = list
                    .as_array()
                    .and_then(|all| all.iter().find(|c| text(c, "digest") == digest))
                    .cloned();
                if let Some(candidate) = found {
                    let missing = new_capabilities(&candidate)
                        .into_iter()
                        .filter(|(cap, _)| !accept.iter().any(|a| a == cap))
                        .collect::<Vec<_>>();
                    if !missing.is_empty() {
                        let against = text(&candidate["changes"]["against"], "version");
                        let mut said = format!(
                            "nothing was approved: {} {} asks for more than {}:\n",
                            text(&candidate, "name"),
                            text(&candidate, "version"),
                            if against.is_empty() {
                                "the version that opens now"
                            } else {
                                against
                            }
                        );
                        for (cap, words) in &missing {
                            said.push_str(&format!("  - it can {words} ({cap})\n"));
                        }
                        let all = new_capabilities(&candidate)
                            .into_iter()
                            .map(|(cap, _)| cap)
                            .collect::<Vec<_>>()
                            .join(",");
                        said.push_str(&format!(
                            "Check each, then approve with:\n  krate org approve {slug} {digest} --accept {all}"
                        ));
                        anyhow::bail!(said);
                    }
                }
            }
            let answer = worded(
                client.send(
                    "POST",
                    &format!("/orgs/{}/candidates/{}/approve", seg(&slug)?, seg(&digest)?),
                    &Body::Json(serde_json::json!({ "accept": accept })),
                ),
                &slug,
                &me,
                Action::Approve(&digest),
            )?;
            let release = &answer["release"];
            let released = Some(text(release, "digest"))
                .filter(|d| !d.is_empty())
                .unwrap_or(&digest);
            if release["solo"].as_bool() == Some(true) {
                println!(
                    "released. {} is now a release of {slug}, released by you: {slug} has \
                     nobody else who can approve yet.",
                    short(released)
                );
            } else {
                println!("approved. {} is now a release of {slug}.", short(released));
            }
            let url = text(release, "url");
            if !url.is_empty() {
                println!("Its members and recipients run it with:");
                println!("  krate run {url}");
                println!("The link alone opens nothing: each person's own sign-in is checked.");
            }
            Ok(0)
        }
        OrgCommand::Releases { slug } => {
            let answer = worded(
                client.send(
                    "GET",
                    &format!("/orgs/{}/releases", seg(&slug)?),
                    &Body::Empty,
                ),
                &slug,
                &me,
                Action::Releases,
            )?;
            let releases = answer
                .as_array()
                .cloned()
                .or_else(|| answer["releases"].as_array().cloned())
                .unwrap_or_default();
            if releases.is_empty() {
                println!(
                    "{slug} has no releases yet. A developer uploads one with \
                     `krate publish app.krate --org {slug}`, and a reviewer approves it."
                );
                return Ok(0);
            }
            println!(
                "{slug}: {} release{}",
                releases.len(),
                if releases.len() == 1 { "" } else { "s" }
            );
            for release in &releases {
                println!(
                    "  {} {}  {}",
                    text(release, "name"),
                    text(release, "version"),
                    short(text(release, "digest"))
                );
                let at = when(&release["at"]);
                println!(
                    "    uploaded by {}, approved by {}{}",
                    spoken_principal(&release["uploaded_by"]),
                    spoken_principal(&release["approved_by"]),
                    if at.is_empty() {
                        String::new()
                    } else {
                        format!(", {at}")
                    }
                );
                if release["current"].as_bool() == Some(true) {
                    println!("    opens now for its app ({})", text(release, "app_id"));
                }
                if let Some(groups) = release["audience"].as_array().filter(|g| !g.is_empty()) {
                    let groups: Vec<&str> = groups.iter().filter_map(|g| g.as_str()).collect();
                    println!("    only for: {}", groups.join(", "));
                }
                let url = text(release, "url");
                if !url.is_empty() {
                    println!("    krate run {url}");
                }
            }
            Ok(0)
        }
        OrgCommand::Candidates { slug } => {
            let answer = worded(
                client.send(
                    "GET",
                    &format!("/orgs/{}/candidates", seg(&slug)?),
                    &Body::Empty,
                ),
                &slug,
                &me,
                Action::Candidates,
            )?;
            let candidates = answer
                .as_array()
                .cloned()
                .or_else(|| answer["candidates"].as_array().cloned())
                .unwrap_or_default();
            if candidates.is_empty() {
                println!(
                    "{slug} has no candidates yet. A developer uploads one with \
                     `krate publish app.krate --org {slug}`."
                );
                return Ok(0);
            }
            println!(
                "{slug}: {} candidate{}",
                candidates.len(),
                if candidates.len() == 1 { "" } else { "s" }
            );
            for candidate in &candidates {
                let state = match text(candidate, "state") {
                    "waiting-for-approval" => "waiting for approval",
                    other => other,
                };
                println!(
                    "  {} {}  {}  {state}",
                    text(candidate, "name"),
                    text(candidate, "version"),
                    text(candidate, "digest"),
                );
                // What it can do, and for one that is waiting, what is new
                // against the version that opens now (IC-950).
                if text(candidate, "state") == "waiting-for-approval" {
                    let changes = &candidate["changes"];
                    let against = text(&changes["against"], "version");
                    let new = new_capabilities(candidate);
                    if !changes["against"].is_object() {
                        let asked = asked_capabilities(candidate);
                        if !asked.is_empty() {
                            println!("    can, once someone allows it:");
                            for (cap, words) in asked {
                                println!("      - {words} ({cap})");
                            }
                        }
                    } else if new.is_empty() {
                        println!("    asks for nothing new since {against}");
                    } else {
                        println!("    new since {against}, check each before approving:");
                        for (cap, words) in new {
                            println!("      - {words} ({cap})");
                        }
                    }
                }
                let uploaded = when(&candidate["uploaded_at"]);
                let mut line = format!(
                    "    uploaded by {}",
                    spoken_principal(&candidate["uploaded_by"])
                );
                if !uploaded.is_empty() {
                    line.push_str(&format!(" {uploaded}"));
                }
                let approver = spoken_principal(&candidate["approved_by"]);
                if !approver.is_empty() {
                    line.push_str(&format!(", approved by {approver}"));
                    let approved = when(&candidate["approved_at"]);
                    if !approved.is_empty() {
                        line.push_str(&format!(" {approved}"));
                    }
                }
                println!("{line}");
            }
            Ok(0)
        }
        OrgCommand::Release { command } => release_command(client, &me, command),
        OrgCommand::Group { command } => group_command(client, &me, command),
        OrgCommand::Connection { command } => connection::run(client, &me, command),
        OrgCommand::Apps { .. } => unreachable!("handled before signing in"),
        OrgCommand::Keys { command } => keys_command(client, command),
        OrgCommand::Sync { quiet } => signing::sync_all(client, quiet),
        OrgCommand::Setup { slug } => signing::setup(client, &slug),
        OrgCommand::Audit { slug } => {
            let answer = worded(
                client.send("GET", &format!("/orgs/{}/audit", seg(&slug)?), &Body::Empty),
                &slug,
                &me,
                Action::Audit,
            )?;
            let events = answer["events"].as_array().cloned().unwrap_or_default();
            println!(
                "{slug} audit: {} event{}",
                events.len(),
                if events.len() == 1 { "" } else { "s" }
            );
            for event in &events {
                let result = text(event, "result");
                let reason = text(event, "reason");
                let outcome = if reason.is_empty() {
                    result.to_string()
                } else {
                    format!("{result}: {reason}")
                };
                let target = spoken_target(text(event, "target"));
                println!(
                    "  {:>4}  {}  {:<24} {:<18} {:<24} {outcome}",
                    event["seq"]
                        .as_u64()
                        .map(|n| n.to_string())
                        .unwrap_or_default(),
                    when(&event["at"]),
                    spoken_principal(&event["actor"]),
                    text(event, "action"),
                    target,
                );
            }
            if answer["chain_ok"].as_bool() == Some(true) {
                println!(
                    "chain intact: the hub recomputed every link, and each event follows the \
                     one before it."
                );
                Ok(0)
            } else {
                println!(
                    "THE CHAIN IS BROKEN: an event was changed or removed after it was \
                     written, so this history cannot be trusted. Tell {slug}'s owners."
                );
                Ok(1)
            }
        }
    }
}

/// The public key a root-key file names: 64 hex characters, or the public
/// half of a PKCS#8 key (the flag says it was a private key file).
fn public_key_from(path: &Path) -> Result<(String, bool)> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("could not read the key at {}", path.display()))?;
    if let Ok(text) = std::str::from_utf8(&bytes) {
        let text = text.trim();
        if text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Ok((text.to_ascii_lowercase(), false));
        }
    }
    match krate_bundle::signing::SigningKey::from_pkcs8(&bytes) {
        Ok(key) => Ok((
            key.public_key()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
            true,
        )),
        Err(_) => anyhow::bail!(
            "{} is not a root key: give the root's public key (64 hex characters) or the \
             key file `krate sign --generate-key` wrote",
            path.display()
        ),
    }
}

/// The signed list `krate revoke` wrote, checked here before it is sent:
/// it must be a list, and from the root being registered.
fn revocation_list(path: &Path, root: &str) -> Result<serde_json::Value> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("could not read the list at {}", path.display()))?;
    let list: krate_bundle::delegation::SignedRevocationList = serde_json::from_str(&text)
        .map_err(|_| {
            anyhow::anyhow!("{} is not a list written by `krate revoke`", path.display())
        })?;
    if !list.list.root.eq_ignore_ascii_case(root) {
        anyhow::bail!(
            "{} was signed by a different root ({}...) than the key being registered \
             ({}...): give the list `krate revoke --root` wrote with this root",
            path.display(),
            short(&list.list.root),
            short(root)
        );
    }
    Ok(serde_json::to_value(&list)?)
}

/// Where this machine keeps its release key for `slug`, and the delegation
/// an owner of `slug` wrote for it: `~/.krate/orgs/<slug>/`.
fn release_dir(slug: &str) -> Result<std::path::PathBuf> {
    let valid = slug.len() >= 2
        && slug.len() <= 39
        && slug.as_bytes()[0].is_ascii_lowercase()
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !valid {
        anyhow::bail!("\"{slug}\" is not an organisation's name");
    }
    Ok(crate::krate_home().join("orgs").join(slug))
}

/// The release key and the delegation for `slug`, when both are installed.
pub fn installed_release_key(slug: &str) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    let dir = release_dir(slug).ok()?;
    let key = dir.join("release.pkcs8");
    let delegation = dir.join("delegation.json");
    (key.is_file() && delegation.is_file()).then_some((key, delegation))
}

/// The namespace to sign `app` under for a delegation: its own exact
/// namespace, or `<prefix>/<app>` for a range like `acme/*`.
pub fn namespace_for(delegation: &Path, slug: &str, app: &str) -> String {
    let named = std::fs::read_to_string(delegation)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|v| v["delegation"]["namespace"].as_str().map(str::to_string))
        .unwrap_or_else(|| format!("{slug}/*"));
    match named.strip_suffix("/*") {
        Some(prefix) => format!("{prefix}/{app}"),
        None => named,
    }
}

/// The public half of a PKCS#8 key file, as hex.
fn public_hex_of(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("could not read the key at {}", path.display()))?;
    let key = krate_bundle::signing::SigningKey::from_pkcs8(&bytes)
        .map_err(|err| anyhow::anyhow!("{}: {err}", path.display()))?;
    Ok(key
        .public_key()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// `krate org release-key <slug> [--delegation <file>]`.
fn release_key_command(slug: &str, delegation: Option<&Path>) -> Result<u8> {
    let dir = release_dir(slug)?;
    let key = dir.join("release.pkcs8");
    if !key.is_file() {
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("could not make {}", dir.display()))?;
        let document = krate_bundle::signing::SigningKey::generate_pkcs8()
            .map_err(|err| anyhow::anyhow!("could not make a release key: {err}"))?;
        std::fs::write(&key, &document)
            .with_context(|| format!("could not write {}", key.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600));
        }
        println!("made a release key for {slug}, kept at {}", key.display());
    }
    let public = public_hex_of(&key)?;
    let installed = dir.join("delegation.json");
    if let Some(file) = delegation {
        let text = std::fs::read_to_string(file)
            .with_context(|| format!("could not read {}", file.display()))?;
        let slip: krate_bundle::delegation::SignedDelegation = serde_json::from_str(&text)
            .map_err(|_| {
                anyhow::anyhow!(
                    "{} is not a delegation written by `krate delegate`",
                    file.display()
                )
            })?;
        if !slip.delegation.key.eq_ignore_ascii_case(&public) {
            anyhow::bail!(
                "{} delegates a different key ({}...), not this machine's {slug} release key \
                 ({}...): ask the owner to run `krate delegate` with --key {public}",
                file.display(),
                short(&slip.delegation.key),
                short(&public)
            );
        }
        let namespace = namespace_for(file, slug, "app");
        let public_bytes: Vec<u8> = (0..32)
            .filter_map(|i| u8::from_str_radix(&public[i * 2..i * 2 + 2], 16).ok())
            .collect();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let verdict = krate_bundle::delegation::verify_chain(
            &slip,
            &namespace,
            &public_bytes,
            now,
            &krate_bundle::delegation::RevocationState::Known(Vec::new()),
        );
        if !verdict.is_authorised() {
            anyhow::bail!("{} does not hold: {verdict}", file.display());
        }
        std::fs::write(&installed, &text)
            .with_context(|| format!("could not write {}", installed.display()))?;
        println!(
            "installed. Your {slug} release key {}... is delegated for {} until {}.",
            short(&public),
            slip.delegation.namespace,
            when(&serde_json::json!(slip.delegation.expires))
        );
        println!("`krate publish app.krate --org {slug}` now signs with it.");
        return Ok(0);
    }
    println!("your release key for {slug}: {public}");
    if installed.is_file() {
        println!("It is delegated; `krate publish app.krate --org {slug}` signs with it.");
        return Ok(0);
    }
    println!("An owner of {slug} turns it into a delegation with:");
    println!(
        "  krate delegate --root <{slug}-root.pkcs8> --key {public} --namespace '{slug}/*' -o {slug}-release.json"
    );
    println!("Then install the file they send you:");
    println!("  krate org release-key {slug} --delegation {slug}-release.json");
    Ok(0)
}

/// Before anything is uploaded to an organisation: the bundle must carry a
/// signature that verifies. Unsigned, it says exactly what to run.
pub fn require_signed(opened: &krate_bundle::OpenBundle, file: &Path, slug: &str) -> Result<()> {
    let file = file.display();
    // The one line that fixes both a missing mark and a missing signature.
    let fix = format!(
        "krate sign {file} --org {slug} --key <release.pkcs8> --delegation {slug}-release.json"
    );
    let signed = matches!(opened.signature_verdict(), Ok(Some(ref v)) if v.is_genuinely_signed());
    match opened.org_mark() {
        Ok(Some(marked)) if marked != slug => anyhow::bail!(
            "{file} is marked as {marked}'s private app, not {slug}'s, so nothing was \
             uploaded: publish it with `krate publish {file} --org {marked}`, or mark and sign \
             it for {slug}:\n  {fix}"
        ),
        Ok(Some(_)) if signed => Ok(()),
        Ok(Some(_)) => match opened.signature_verdict() {
            Ok(Some(_)) => anyhow::bail!(
                "{file}'s signature does not match its contents, so nothing was uploaded: it \
                 was changed after it was signed. Mark and sign it again:\n  {fix}"
            ),
            _ => anyhow::bail!(
                "{file} is marked for {slug} but not signed, so nothing was uploaded. Sign \
                 it:\n  {fix}"
            ),
        },
        Ok(None) | Err(_) if signed => anyhow::bail!(
            "{file} is signed but not marked as {slug}'s private app, so nothing was \
             uploaded: {slug} takes only apps it is marked for. Mark and sign it for \
             {slug}:\n  {fix}"
        ),
        Ok(None) | Err(_) => anyhow::bail!(
            "{file} is not signed or marked for {slug}, so nothing was uploaded: {slug} takes \
             only apps marked as its private apps and signed by its root key, or by a release \
             key that root delegated.\n\
             \n  The shortest way: make this machine a release key, and send its public half \
             to an owner of {slug}:\n    \
             krate org release-key {slug}\n\
             \n  With a release key ({slug}'s owner runs the first line once):\n    \
             krate delegate --root <{slug}-root.pkcs8> --key <release.pkcs8> --namespace '{slug}/*' -o {slug}-release.json\n    \
             {fix}\n\
             \n  With {slug}'s root key:\n    \
             krate sign {file} --org {slug} --key <{slug}-root.pkcs8>\n\
             \nThen run this publish again."
        ),
    }
}

/// The app part of a namespace, from an app id: its last dotted label,
/// lower-case. `dev.acme.demo` is `demo`.
pub fn app_label(id: &str) -> String {
    id.rsplit('.').next().unwrap_or("app").to_ascii_lowercase()
}

/// `krate publish app.krate --org <slug>`: upload a signed bundle as a
/// candidate. Over the publish door's 5 MiB it goes through the big-file
/// path, the same as any publish. Nothing is released until someone else
/// with approve authority approves its exact digest.
pub fn publish_candidate(
    hub: &str,
    slug: &str,
    identity: github_auth::Identity,
    bytes: &[u8],
    name: &str,
    description: Option<&str>,
    category: &str,
) -> Result<u8> {
    let local = krate_bundle::sha256_hex(bytes);
    let client = Client::new(hub.trim_end_matches('/').to_string(), identity);
    let me = client.me();
    let answer: serde_json::Value = if bytes.len() > crate::PUBLISH_DOOR_BYTES {
        let fields = serde_json::json!({
            "name": name,
            "description": description.unwrap_or(""),
            "category": category,
            "unlisted": true,
            "org": slug,
        });
        let body = worded(
            crate::publish_big(
                &client.hub,
                &client.identity.token,
                bytes,
                "bundle",
                None,
                fields,
            ),
            slug,
            &me,
            Action::Publish,
        )?;
        serde_json::from_str(&body).unwrap_or_default()
    } else {
        let mut headers = vec![("X-Krate-Encoding", "uri".to_string())];
        if !name.is_empty() {
            headers.push(("X-Krate-Name", crate::percent_encode_text(name)));
        }
        if let Some(description) = description {
            headers.push((
                "X-Krate-Description",
                crate::percent_encode_text(description),
            ));
        }
        headers.push(("X-Krate-Category", category.to_string()));
        worded(
            client.send(
                "POST",
                &format!("/orgs/{}/candidates", seg(slug)?),
                &Body::Bundle(bytes, headers),
            ),
            slug,
            &me,
            Action::Publish,
        )?
    };
    let digest = Some(text(&answer, "digest"))
        .filter(|d| !d.is_empty())
        .unwrap_or(&local)
        .to_string();
    let app = format!("{} {}", text(&answer, "name"), text(&answer, "version"));
    // Nobody else in the organisation may approve yet, and its owner sent
    // it: released now, and said so (IC-958, solo release).
    if answer["solo_release"].as_bool() == Some(true) {
        let approver = Client::new(client.hub.clone(), client.identity.clone());
        let released = worded(
            approver.send(
                "POST",
                &format!("/orgs/{}/candidates/{}/approve", seg(slug)?, seg(&digest)?),
                &Body::Json(serde_json::json!({ "accept": [] })),
            ),
            slug,
            &me,
            Action::Approve(&digest),
        )?;
        let url = text(&released["release"], "url");
        println!(
            "released {} to {slug}. Everyone in {slug} can use it now.",
            Some(app.trim()).filter(|a| !a.is_empty()).unwrap_or("it")
        );
        println!("  digest  {digest}");
        if !url.is_empty() {
            println!("  open it: krate run {url}");
        }
        println!(
            "Released by you: {slug} has nobody else who can approve yet. Once someone who can \
             approve joins, every release waits for a second person."
        );
        return Ok(0);
    }
    println!("uploaded to {slug} as a candidate. It waits for approval and opens for nobody yet.");
    if !app.trim().is_empty() {
        println!("  app     {}", app.trim());
    }
    println!("  digest  {digest}");
    if digest != local {
        println!(
            "  note: the hub reports a different digest than these bytes ({}); what it \
             stored is not byte-for-byte what was sent",
            short(&local)
        );
    }
    let approvers: Vec<String> = answer["approvers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| a.as_str().map(str::to_string))
        .collect();
    match approvers.as_slice() {
        [] => {}
        [one] => println!(
            "Waiting for {one} to approve it: https://krate.tech/org/?o={slug}&tab=waiting"
        ),
        [first, ..] => println!(
            "Waiting for {first} or another reviewer to approve it: \
             https://krate.tech/org/?o={slug}&tab=waiting"
        ),
    }
    println!("A reviewer in {slug} (not you) releases it with:");
    println!("  krate org approve {slug} {digest}");
    Ok(0)
}

/// The session to send with a link to run: only to a Krate hub -- the one
/// configured here, or Krate's own -- so a link to anywhere else never
/// receives the person's GitHub token.
pub fn link_authorization(url: &str) -> Option<String> {
    if !is_hub_link(url) {
        return None;
    }
    github_auth::current().map(|identity| format!("Bearer {}", identity.token))
}

pub(crate) fn origin(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    // userinfo@host is not a hub; refuse rather than guess which part counts.
    if host.contains('@') || host.is_empty() {
        return None;
    }
    Some(format!(
        "{}://{}",
        scheme.to_ascii_lowercase(),
        host.to_ascii_lowercase()
    ))
}

pub(crate) fn is_hub_link(url: &str) -> bool {
    let Some(link) = origin(url) else {
        return false;
    };
    // Under a pinned hub, that hub is the only one there is.
    if let Some(pinned) = crate::fleet::pinned_hub() {
        return link == pinned;
    }
    let configured = hub_url(None).ok().and_then(|hub| origin(&hub));
    configured.as_deref() == Some(link.as_str())
        || link == "https://hub.krate.tech"
        || link == "https://krate.tech"
}

/// The words for a refused download of a private release.
pub fn download_refusal(url: &str, reason: &str, message: &str, org: Option<&str>) -> String {
    let org_name = org.unwrap_or("the organisation that sent it");
    let me = github_auth::current().map(|identity| identity.login.to_ascii_lowercase());
    match reason {
        "not-signed-in" => match (&me, is_hub_link(url)) {
            (Some(_), false) => format!(
                "this app is private, and the link is not on your Krate hub, so Krate did not \
                 send your sign-in and nothing ran: if you trust that hub, run it again with \
                 KRATE_HUB_URL={}",
                origin(url).unwrap_or_default()
            ),
            (Some(login), true) => format!(
                "the hub did not accept your sign-in as {login} (it may have run out), so \
                 nothing ran: sign in again with `krate account login`, then run the same link \
                 again"
            ),
            (None, _) => "this app is private to an organisation and you are not signed in, so \
                  nothing was downloaded or run: sign in with `krate account login` as the \
                  GitHub account it was shared with, then run the same link again"
                .to_string(),
        },
        "not-a-member" => {
            let me = me.unwrap_or_else(|| "this account".to_string());
            format!(
                "this app is private to {org_name}, and {me} is not one of its members or \
                 recipients, so nothing ran: ask an owner or admin of {org_name} to invite {me}"
            )
        }
        "removed" => format!(
            "you were removed from {org_name}, so this app no longer opens for you: ask an \
             owner of {org_name} if that is a mistake"
        ),
        "role-cannot" => format!(
            "your role in {org_name} cannot download its releases: ask an owner or admin of \
             {org_name}"
        ),
        "no-such-org" => {
            "the organisation that released this app is gone, so it no longer opens".to_string()
        }
        "digest-mismatch" => format!(
            "this link is to an app {org_name} has not approved (or no longer has), so nothing \
             ran: ask whoever sent it for the link `krate org approve` printed"
        ),
        "orgs-off" => "organisations are switched off on this hub, so its private apps do \
             not open: ask whoever runs it"
            .to_string(),
        "release-withdrawn" => match Some(message.trim()).filter(|m| !m.is_empty()) {
            Some(said) => {
                format!("this app no longer opens, here or anywhere it next connects: {said}")
            }
            None => format!(
                "{org_name} withdrew this version, so it no longer opens: ask an admin of \
                 {org_name} which version to use now"
            ),
        },
        "not-in-audience" => match Some(message.trim()).filter(|m| !m.is_empty()) {
            Some(said) => format!("this app does not open for you: {said}"),
            None => format!(
                "this app is only for some groups in {org_name}, and you are in none of them: \
                 ask an admin of {org_name} to add you"
            ),
        },
        _ => format!("the hub would not hand this app over: {message}"),
    }
}

/// `krate org release ...` (IC-949 R).
fn release_command(client: &Client, me: &str, command: ReleaseCommand) -> Result<u8> {
    match command {
        ReleaseCommand::Withdraw {
            slug,
            digest,
            reason,
        } => {
            let digest = digest.trim().to_ascii_lowercase();
            let answer = worded(
                client.send(
                    "POST",
                    &format!("/orgs/{}/releases/{}/withdraw", seg(&slug)?, seg(&digest)?),
                    &Body::Json(serde_json::json!({ "reason": reason.trim() })),
                ),
                &slug,
                me,
                Action::Withdraw,
            )?;
            let release = &answer["release"];
            println!(
                "withdrew {} {} ({}). From now on nobody downloads it or renews offline \
                 access to it, and every computer refuses it when it next connects.",
                text(release, "name"),
                text(release, "version"),
                short(&digest)
            );
            let leases = &answer["leases"];
            match leases["active"].as_u64().unwrap_or(0) {
                0 => println!("Nobody holds offline access to it, so it stops everywhere now."),
                n => {
                    println!(
                    "Offline access already given out holds without a connection until it runs \
                     out: {n} {} it, the last until {}.",
                    if n == 1 { "person holds" } else { "people hold" },
                    when(&leases["until"])
                )
                }
            }
            let current = &answer["current"];
            let app = text(current, "app_id");
            match current["digest"].as_str() {
                Some(now) if now != digest => {
                    println!("{} opens {} now.", app, short(now));
                }
                _ if current["digest"].is_null() && !app.is_empty() => println!(
                    "No release of {app} opens now. Approve a new one, or choose an older \
                     one with `krate org release current {slug} {app} <digest>`."
                ),
                _ => {}
            }
            Ok(0)
        }
        ReleaseCommand::Current {
            slug,
            app_id,
            digest,
        } => {
            let path = format!("/orgs/{}/apps/{}/current", seg(&slug)?, seg(&app_id)?);
            let link = format!("{}{path}", client.hub);
            let Some(digest) = digest else {
                let answer = worded(
                    client.send("GET", &path, &Body::Empty),
                    &slug,
                    me,
                    Action::Releases,
                )?;
                println!(
                    "{app_id} opens {} {} ({}).",
                    text(&answer, "name"),
                    text(&answer, "version"),
                    short(text(&answer, "digest"))
                );
                println!("Its link always opens the current release:");
                println!("  krate run {link}");
                return Ok(0);
            };
            let digest = digest.trim().to_ascii_lowercase();
            let answer = worded(
                client.send(
                    "PUT",
                    &path,
                    &Body::Json(serde_json::json!({ "digest": digest })),
                ),
                &slug,
                me,
                Action::SetCurrent,
            )?;
            let current = &answer["current"];
            println!(
                "{app_id} now opens {} {} ({}); the change is in {slug}'s audit.",
                text(current, "name"),
                text(current, "version"),
                short(&digest)
            );
            println!("Its link opens it for everyone it is for:");
            println!("  krate run {link}");
            Ok(0)
        }
        ReleaseCommand::Audience {
            slug,
            digest,
            groups,
            everyone,
        } => {
            let digest = digest.trim().to_ascii_lowercase();
            let body = if everyone {
                serde_json::json!({ "everyone": true })
            } else {
                let groups: Vec<String> = groups
                    .iter()
                    .map(|g| g.trim().to_ascii_lowercase())
                    .filter(|g| !g.is_empty())
                    .collect();
                serde_json::json!({ "groups": groups })
            };
            let answer = worded(
                client.send(
                    "PUT",
                    &format!("/orgs/{}/releases/{}/audience", seg(&slug)?, seg(&digest)?),
                    &Body::Json(body),
                ),
                &slug,
                me,
                Action::Audience,
            )?;
            match answer["audience"].as_array().filter(|g| !g.is_empty()) {
                Some(groups) => {
                    let groups: Vec<&str> = groups.iter().filter_map(|g| g.as_str()).collect();
                    println!(
                        "{} opens only for {} in {slug}, and always for its owners, admins and \
                         reviewers. Anyone else is refused.",
                        short(&digest),
                        groups.join(", ")
                    );
                }
                None => println!("{} opens for everyone in {slug}.", short(&digest)),
            }
            Ok(0)
        }
    }
}

/// `krate org group ...` (IC-949 R).
/// This computer's agent for one organisation's keyring.
fn key_agent(client: &Client, slug: &str) -> Result<krate_orgkeys::agent::Agent> {
    let device = crate::access::org_device_key()?;
    let me = client.my_principal();
    Ok(krate_orgkeys::agent::Agent::new(
        &client.hub,
        slug,
        Some(format!("Bearer {}", client.identity.token)),
        device,
        &crate::access::device_name(Some(&me)),
    ))
}

fn key_words(err: krate_orgkeys::agent::AgentError, slug: &str) -> anyhow::Error {
    use krate_orgkeys::agent::AgentError;
    match err {
        AgentError::Offline => {
            anyhow::anyhow!("could not reach the hub for {slug}'s key: try again when online")
        }
        AgentError::Refused {
            reason, message, ..
        } if reason == "key-exists" => {
            anyhow::anyhow!("{slug} already has its key: {message}")
        }
        other => anyhow::anyhow!("{other}"),
    }
}

/// Make the key and say the recovery code, once.
fn keys_init(client: &Client, slug: &str) -> Result<()> {
    let agent = key_agent(client, slug)?;
    let (held, code) = agent.init().map_err(|err| key_words(err, slug))?;
    println!(
        "made {slug}'s key (version {}) on this computer. Its apps' data is sealed with it; \
         Krate's hub holds only ciphertext.",
        held.keyring.current().0
    );
    println!("recovery code: {}", code.display());
    println!(
        "Keep the recovery code where your company keeps passwords: it is shown once. With it an \
         owner or admin restores the key on a new computer (`krate org keys recover {slug}`). \
         Without a member's computer or this code, the organisation's app data cannot be read by \
         anyone, Krate included."
    );
    Ok(())
}

fn keys_command(client: &Client, command: KeysCommand) -> Result<u8> {
    use krate_orgkeys::agent::Status;
    match command {
        KeysCommand::Init { slug } => {
            keys_init(client, &slug)?;
            Ok(0)
        }
        KeysCommand::Show { slug } => {
            let agent = key_agent(client, &slug)?;
            // Showing is also this device doing its share: registering,
            // taking the key, passing it on.
            let status = agent.refresh(None).map_err(|err| key_words(err, &slug))?;
            let view = agent.view().map_err(|err| key_words(err, &slug))?;
            match status {
                Status::NoKey => println!("{slug} has no key yet: an owner runs `krate org keys init {slug}`"),
                Status::Waiting => println!(
                    "this computer is waiting for {slug}'s key: the next time another member's computer is online with one of {slug}'s apps open, it passes the key on"
                ),
                Status::Holds(held) => {
                    println!("this computer holds {slug}'s key, version {}", held.keyring.current().0)
                }
            }
            for d in view["devices"].as_array().into_iter().flatten() {
                let holds = d["holds"]
                    .as_u64()
                    .map(|v| format!("holds version {v}"))
                    .unwrap_or_else(|| "waiting".to_string());
                println!(
                    "  {}  {}  {}  {}",
                    text(d, "device_id"),
                    text(d, "display"),
                    text(d, "name"),
                    if text(d, "state") == "active" {
                        holds
                    } else {
                        text(d, "state").to_string()
                    }
                );
            }
            Ok(0)
        }
        KeysCommand::Recover { slug, code } => {
            let code = match code {
                Some(code) => code,
                None => {
                    eprint!("recovery code for {slug}: ");
                    let mut line = String::new();
                    std::io::stdin().read_line(&mut line)?;
                    line
                }
            };
            let code = krate_orgkeys::RecoveryCode::parse(&code).context(
                "that is not a recovery code: 26 letters and digits, as it was shown when the key was made",
            )?;
            let agent = key_agent(client, &slug)?;
            let held = agent.recover(&code).map_err(|err| match err {
                krate_orgkeys::agent::AgentError::Key(_) => anyhow::anyhow!(
                    "that recovery code does not open {slug}'s key: check it, character by character"
                ),
                other => key_words(other, &slug),
            })?;
            println!(
                "restored {slug}'s key (version {}) on this computer from the recovery code",
                held.keyring.current().0
            );
            // And, for an owner or admin, signing for the organisation
            // (IC-958): the same code opens it.
            if agent.signing_recover(&code).is_ok() {
                println!("This computer signs {slug}'s apps again.");
            }
            Ok(0)
        }
    }
}

fn group_command(client: &Client, me: &str, command: GroupCommand) -> Result<u8> {
    match command {
        GroupCommand::Create { slug, name } => {
            let name = name.trim().to_ascii_lowercase();
            worded(
                client.send(
                    "POST",
                    &format!("/orgs/{}/groups", seg(&slug)?),
                    &Body::Json(serde_json::json!({ "name": name })),
                ),
                &slug,
                me,
                Action::Groups,
            )?;
            println!("made the group {name} in {slug}. Put people in it with:");
            println!("  krate org group add {slug} {name} <login or email>");
            Ok(0)
        }
        GroupCommand::Add { slug, name, login } => {
            group_member(client, me, &slug, &name, &login, true)
        }
        GroupCommand::Remove { slug, name, login } => {
            group_member(client, me, &slug, &name, &login, false)
        }
        GroupCommand::List { slug } => {
            let answer = worded(
                client.send(
                    "GET",
                    &format!("/orgs/{}/groups", seg(&slug)?),
                    &Body::Empty,
                ),
                &slug,
                me,
                Action::Groups,
            )?;
            let groups = answer["groups"].as_array().cloned().unwrap_or_default();
            if groups.is_empty() {
                println!(
                    "{slug} has no groups yet. An owner or admin makes one with \
                     `krate org group create {slug} <name>`."
                );
                return Ok(0);
            }
            for group in &groups {
                let members: Vec<String> = group["members"]
                    .as_array()
                    .map(|m| m.iter().map(spoken_principal).collect())
                    .unwrap_or_default();
                println!(
                    "  {}  {}",
                    text(group, "name"),
                    if members.is_empty() {
                        "nobody yet".to_string()
                    } else {
                        members.join(", ")
                    }
                );
            }
            Ok(0)
        }
    }
}

/// Put a member in a group, or take them out.
fn group_member(
    client: &Client,
    me: &str,
    slug: &str,
    name: &str,
    login: &str,
    adding: bool,
) -> Result<u8> {
    let name = name.trim().to_ascii_lowercase();
    let principal = principal_of(login);
    let who = spoken_principal(&serde_json::json!(principal));
    worded(
        client.send(
            if adding { "PUT" } else { "DELETE" },
            &format!(
                "/orgs/{}/groups/{}/members/{}",
                seg(slug)?,
                seg(&name)?,
                seg(&principal)?
            ),
            &Body::Empty,
        ),
        slug,
        me,
        Action::Groups,
    )?;
    if adding {
        println!("{who} is in {name}: releases for {name} open for them.");
    } else {
        println!("{who} is out of {name}: releases only for {name} no longer open for them.");
    }
    Ok(0)
}

/// The capabilities a waiting candidate adds that a person is asked for,
/// as (exact name, plain words), from the hub's `changes` (IC-950). Empty
/// for a first release, an unchanged one, or a hub from before.
fn new_capabilities(candidate: &serde_json::Value) -> Vec<(String, String)> {
    if !candidate["changes"]["against"].is_object() {
        return Vec::new();
    }
    capability_pairs(&candidate["changes"]["added"])
}

/// What a candidate can do that a person is asked for, as (name, words).
fn asked_capabilities(candidate: &serde_json::Value) -> Vec<(String, String)> {
    capability_pairs(&candidate["capabilities"])
}

fn capability_pairs(list: &serde_json::Value) -> Vec<(String, String)> {
    list.as_array()
        .map(|all| {
            all.iter()
                .filter(|c| c["asks"].as_bool() != Some(false))
                .map(|c| {
                    let cap = text(c, "cap").to_string();
                    let words = Some(text(c, "words"))
                        .filter(|w| !w.is_empty())
                        .unwrap_or(&cap)
                        .to_string();
                    (cap, words)
                })
                .filter(|(cap, _)| !cap.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Write a private key readable by this person only.
fn write_private_key(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    options
        .open(path)
        .and_then(|mut f| f.write_all(bytes))
        .map_err(|err| anyhow::anyhow!("could not write {}: {err}", path.display()))
}

#[cfg(test)]
mod tests {

    #[test]
    fn new_capabilities_are_the_asked_additions_against_a_release() {
        let update = serde_json::json!({
            "changes": {
                "against": { "digest": "d", "version": "1.0.0" },
                "added": [
                    { "cap": "camera.capture", "words": "use your camera", "asks": true },
                    { "cap": "io.stdout", "words": "print output", "asks": false },
                    { "cap": "mystery.thing" }
                ]
            }
        });
        assert_eq!(
            new_capabilities(&update),
            vec![
                ("camera.capture".to_string(), "use your camera".to_string()),
                ("mystery.thing".to_string(), "mystery.thing".to_string()),
            ],
            "asked-for additions, and anything not marked otherwise counts as asked"
        );
        // A first release, or a hub from before, has nothing to confirm.
        let first = serde_json::json!({ "changes": { "against": null, "added": [{ "cap": "camera.capture", "asks": true }] } });
        assert!(new_capabilities(&first).is_empty());
        assert!(new_capabilities(&serde_json::json!({ "digest": "x" })).is_empty());
    }
    use super::*;

    /// Every stable reason the contract names has its own words, so none
    /// reaches a person as the hub's bare string.
    #[test]
    fn every_contract_reason_has_its_own_words() {
        let reasons = [
            "not-signed-in",
            "not-a-member",
            "removed",
            "role-cannot",
            "last-owner",
            "self-approval",
            "digest-mismatch",
            "not-signed-for-this-org",
            "invite-expired",
            "invite-used",
            "invite-for-someone-else",
            "invite-revoked",
            "slug-taken",
            "slug-invalid",
            "idempotency-key-reused",
            "no-such-org",
            "not-marked-for-this-org",
            "private-app",
            "invalid-bundle",
            "bytes-held-elsewhere",
            "too-large",
            "store-failed",
            "bad-request",
            "orgs-off",
            "release-withdrawn",
            "not-in-audience",
            "reason-required",
            "no-such-group",
            "group-taken",
            "group-invalid",
            "no-such-connection",
            "connection-taken",
            "connection-invalid",
            "credential-required",
            "connections-off",
            "connection-not-in-audience",
            "app-not-allowed",
            "connection-not-requested",
        ];
        for reason in reasons {
            let refusal = HubRefusal {
                reason: reason.to_string(),
                message: "SENTINEL".to_string(),
            };
            let said = words(&refusal, "acme", "bob", Action::Approve("abc"));
            // Some reasons quote the hub's detail (which rule a signature
            // failed, what was wrong with the file); none is ONLY that.
            assert!(
                !said.starts_with("the hub refused"),
                "{reason} fell through: {said}"
            );
            assert!(
                said.replace("SENTINEL", "").trim().len() > 40,
                "{reason} says more than the hub did: {said}"
            );
            assert!(said.contains(':'), "{reason} names a next step: {said}");
        }
    }

    #[test]
    fn a_session_goes_only_to_a_hub() {
        assert_eq!(
            origin("https://Hub.Krate.Tech/a/abc").as_deref(),
            Some("https://hub.krate.tech")
        );
        assert!(origin("https://hub.krate.tech@evil.example/a/x").is_none());
        assert!(is_hub_link("https://hub.krate.tech/a/abc"));
        assert!(!is_hub_link("https://hub.krate.tech.evil.example/a/abc"));
        assert!(!is_hub_link("https://evil.example/a/abc"));
    }

    #[test]
    fn people_are_named_by_principal_and_read_plainly() {
        assert_eq!(principal_of("Alice"), "github:alice");
        assert_eq!(principal_of("Bob@Corp.com"), "email:bob@corp.com");
        assert_eq!(principal_of("github:carol"), "github:carol");
        assert_eq!(
            spoken_principal(&serde_json::json!("github:alice")),
            "alice (GitHub)"
        );
        assert_eq!(
            spoken_principal(&serde_json::json!({
                "principal": "email:bob@corp.com", "display": "bob@corp.com", "kind": "email"
            })),
            "bob@corp.com (email)"
        );
        assert_eq!(
            spoken_principal(&serde_json::json!({ "login": "dan" })),
            "dan"
        );
        assert_eq!(
            seg(&principal_of("bob@corp.com")).unwrap(),
            "email%3Abob%40corp.com"
        );
        let live = "https://hub.krate.tech";
        assert_eq!(
            join_url(&serde_json::json!({}), "acme", "abc+/=", live).as_deref(),
            Some("https://krate.tech/join/?o=acme&c=abc%2B%2F%3D")
        );
    }

    /// K-1105: an invitation on a hub on this machine links to the site on
    /// this machine the hub names, never to krate.tech, whose join page
    /// talks to another hub; a hub that names no site gets no link.
    #[test]
    fn a_local_invitation_never_links_to_krate_tech() {
        let local = "http://127.0.0.1:8787";
        let named = serde_json::json!({ "join_url": "http://127.0.0.1:8000/join/?o=acme&c=x" });
        assert_eq!(
            join_url(&named, "acme", "x", local).as_deref(),
            Some("http://127.0.0.1:8000/join/?o=acme&c=x")
        );
        let unset = serde_json::json!({ "join_url": "https://krate.tech/join/?o=acme&c=x" });
        assert_eq!(join_url(&unset, "acme", "x", local), None);
        assert_eq!(join_url(&serde_json::json!({}), "acme", "x", local), None);
        // A company's own hub with a site of its own keeps its link; plain
        // http off this machine is never offered.
        let own = serde_json::json!({ "join_url": "https://krate.acme.com/join/?o=acme&c=x" });
        assert!(join_url(&own, "acme", "x", "https://hub.acme.com").is_some());
        let plain = serde_json::json!({ "join_url": "http://krate.acme.com/join/?o=acme&c=x" });
        assert_eq!(join_url(&plain, "acme", "x", "https://hub.acme.com"), None);
        assert_eq!(
            join_url(&unset, "acme", "x", "https://hub.krate.tech").as_deref(),
            Some("https://krate.tech/join/?o=acme&c=x")
        );
    }

    #[test]
    fn a_path_segment_cannot_leave_its_route() {
        assert_eq!(seg("../admin").unwrap(), "..%2Fadmin");
        assert_eq!(seg("bob").unwrap(), "bob");
        assert!(seg("..").is_err() && seg("").is_err());
    }

    #[test]
    fn unix_seconds_read_as_a_date() {
        assert_eq!(when(&serde_json::json!(0)), "1970-01-01 00:00 UTC");
        assert_eq!(
            when(&serde_json::json!(1_760_400_000u64)),
            "2025-10-14 00:00 UTC"
        );
        assert_eq!(
            when(&serde_json::json!(1_760_400_000_000u64)),
            "2025-10-14 00:00 UTC"
        );
        assert_eq!(
            when(&serde_json::json!("2026-10-21T00:00:00Z")),
            "2026-10-21T00:00:00Z"
        );
    }
}
