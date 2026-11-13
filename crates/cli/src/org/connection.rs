//! `krate org connection ...`: an organisation's company connections
//! (IC-952).
//!
//! A connection is an API's address and credential, set up once by an owner
//! or admin, that the organisation's private apps use without ever holding
//! the credential: an app declares `net.connection:<name>`, and Krate adds
//! the credential to that app's requests to the address.
//!
//! The credential is never an argument, because an argument stays in the
//! shell's history and shows in the list of running processes. It is read
//! from a file (`--value-file`) or typed at a prompt that does not show it,
//! encrypted here to the organisation's key (F-316), sent once, and never
//! printed back: the hub keeps only what it cannot read, and shows who set
//! it and when.

use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Subcommand;

use super::{seg, spoken_principal, text, when, worded, Action, Body, Client};

#[derive(Debug, Subcommand)]
pub enum ConnectionCommand {
    /// The organisation's connections: each one's address, what it allows,
    /// who and which apps may use it, and when its credential was set. Never
    /// the credential itself.
    List { slug: String },
    /// Set up a connection. The credential's value is read from
    /// --value-file, or asked for at a prompt that does not show it.
    Create {
        slug: String,
        /// Its name, which apps ask for as `net.connection:<name>`: 2 to 39
        /// lower-case letters, digits and dashes, like finance-api.
        name: String,
        /// The address it covers: https, a host and a path, like
        /// https://api.acme.com/finance. Requests under it get the credential.
        #[arg(long)]
        base: String,
        /// The credential is this header, like Authorization or X-Api-Key;
        /// its value is read from --value-file or the prompt.
        #[arg(long, value_name = "NAME", conflicts_with = "basic")]
        header: Option<String>,
        /// The credential is a user and password, sent as Basic; this is the
        /// user, and the password is read from --value-file or the prompt.
        #[arg(long, value_name = "USER")]
        basic: Option<String>,
        /// A file holding the header's value or the password, on one line.
        #[arg(long, value_name = "FILE")]
        value_file: Option<PathBuf>,
        /// The methods apps may use, comma-separated. GET alone unless given.
        #[arg(long, value_delimiter = ',', default_value = "GET")]
        allow: Vec<String>,
        /// The paths under the address apps may reach, comma-separated, like
        /// /invoices,/rates. All of it unless given.
        #[arg(long, value_delimiter = ',')]
        paths: Vec<String>,
        /// Only members of these groups may use it, comma-separated.
        /// Everyone in the organisation unless given.
        #[arg(long, value_delimiter = ',')]
        groups: Vec<String>,
        /// Only these apps may use it, by the app id in their manifest,
        /// comma-separated. Any of the organisation's apps unless given.
        #[arg(long, value_delimiter = ',')]
        apps: Vec<String>,
        /// Do not tell the API who is calling. By default each request also
        /// carries a short-lived Krate-Identity token, signed by Krate,
        /// naming the person, the app and its version, which the API can log
        /// and check against the hub's /.well-known/jwks.json.
        #[arg(long)]
        no_identity: bool,
    },
    /// Give a connection a new credential. Every app that uses it gets the
    /// new one the next time it opens.
    Replace {
        slug: String,
        name: String,
        /// The credential is this header; its value comes from --value-file
        /// or the prompt.
        #[arg(long, value_name = "NAME", conflicts_with = "basic")]
        header: Option<String>,
        /// The credential is a user and password, sent as Basic.
        #[arg(long, value_name = "USER")]
        basic: Option<String>,
        #[arg(long, value_name = "FILE")]
        value_file: Option<PathBuf>,
    },
    /// Remove a connection and its credential. Apps that use it are refused
    /// it from their next open.
    Remove { slug: String, name: String },
}

/// The value of a credential: one line from a file, or typed at a prompt
/// that does not echo. Never from an argument.
fn read_value(file: Option<&PathBuf>, what: &str) -> Result<String> {
    let line = match file {
        Some(path) => std::fs::read_to_string(path)
            .with_context(|| format!("could not read {}", path.display()))?,
        None => {
            let stdin = std::io::stdin();
            if stdin.is_terminal() {
                eprint!("{what} (it is not shown as you type, and never printed): ");
                let _ = std::io::stderr().flush();
            }
            let _quiet = Quiet::enter();
            let mut line = String::new();
            stdin
                .lock()
                .read_line(&mut line)
                .context("could not read the value")?;
            if stdin.is_terminal() {
                eprintln!();
            }
            line
        }
    };
    let value = line.trim_end_matches(['\r', '\n']).to_string();
    if value.trim().is_empty() {
        anyhow::bail!("nothing was sent: the {what} was empty");
    }
    Ok(value)
}

/// Echo off for as long as this lives, when stdin is a terminal.
struct Quiet {
    #[cfg(unix)]
    previous: Option<libc::termios>,
}

impl Quiet {
    fn enter() -> Self {
        #[cfg(unix)]
        {
            if !std::io::stdin().is_terminal() {
                return Self { previous: None };
            }
            // SAFETY: tcgetattr fills a termios we own; the fd is stdin.
            unsafe {
                let mut previous: libc::termios = std::mem::zeroed();
                if libc::tcgetattr(libc::STDIN_FILENO, &mut previous) != 0 {
                    return Self { previous: None };
                }
                let mut quiet = previous;
                quiet.c_lflag &= !libc::ECHO;
                if libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &quiet) != 0 {
                    return Self { previous: None };
                }
                Self {
                    previous: Some(previous),
                }
            }
        }
        #[cfg(not(unix))]
        {
            Self {}
        }
    }
}

#[cfg(unix)]
impl Drop for Quiet {
    fn drop(&mut self) {
        if let Some(previous) = self.previous {
            // SAFETY: restoring the termios captured in `enter`.
            unsafe {
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &previous);
            }
        }
    }
}

/// The longest header value the runtime sends (adapter-common net.rs).
const MAX_VALUE: usize = 4096;

/// Headers the transport owns: a credential in one would be rewritten or
/// refused on the way out.
const TRANSPORT_HEADERS: &[&str] = &[
    "host",
    "content-length",
    "transfer-encoding",
    "connection",
    "upgrade",
    "te",
    "trailer",
    "keep-alive",
    "expect",
];

/// A credential as typed, checked here because the hub cannot look inside
/// the seal: the kind, the header it goes in, and the header's value.
struct Typed {
    kind: &'static str,
    header: String,
    value: String,
}

/// What the credential is, read and checked. Never from an argument.
fn credential(
    header: Option<&str>,
    basic: Option<&str>,
    value_file: Option<&PathBuf>,
) -> Result<Typed> {
    let typed = match (header, basic) {
        (Some(name), None) => {
            let name = name.trim();
            if name.is_empty()
                || name.len() > 64
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+.^_`|~-".contains(&b))
            {
                anyhow::bail!("the header name is one word, such as Authorization or X-Api-Key");
            }
            let lower = name.to_ascii_lowercase();
            if TRANSPORT_HEADERS.contains(&lower.as_str()) || lower.starts_with("proxy-") {
                anyhow::bail!(
                    "{name} is a header the connection itself sets, so it cannot carry a \
                     credential: use the one your API names, like Authorization"
                );
            }
            let value = read_value(value_file, &format!("{name} value"))?;
            Typed {
                kind: "header",
                header: name.to_string(),
                value,
            }
        }
        (None, Some(user)) => {
            if user.is_empty() || user.contains(':') || user.len() > 256 {
                anyhow::bail!("the user is one line without a colon");
            }
            let password = read_value(value_file, &format!("password for {user}"))?;
            if password.len() > 1024 {
                anyhow::bail!("nothing was sent: the password is longer than 1024 characters");
            }
            Typed {
                kind: "basic",
                header: "Authorization".to_string(),
                value: format!(
                    "Basic {}",
                    krate_orgkeys::b64_encode(format!("{user}:{password}").as_bytes())
                ),
            }
        }
        _ => anyhow::bail!(
            "say what the credential is: --header <name> (like --header Authorization), or \
             --basic <user>; its value is read from --value-file or asked for"
        ),
    };
    if typed.value.len() > MAX_VALUE || typed.value.contains(['\r', '\n', '\0']) {
        anyhow::bail!(
            "nothing was sent: the credential is one line of up to {MAX_VALUE} characters"
        );
    }
    Ok(typed)
}

/// A connection's address as the hub keeps it (cloud/worker
/// org-connections.js normalBase): scheme and host in lower case, no
/// default port, no trailing slash. The credential is sealed for exactly
/// this text, so it is what is sent; the hub refuses one that differs.
fn normal_base(raw: &str) -> String {
    let raw = raw.trim();
    let Some((scheme, rest)) = raw.split_once("://") else {
        return raw.to_string();
    };
    let scheme = scheme.to_ascii_lowercase();
    let (authority, path) = match rest.find('/') {
        Some(at) => rest.split_at(at),
        None => (rest, ""),
    };
    let authority = authority.to_ascii_lowercase();
    let authority = match (scheme.as_str(), authority.rsplit_once(':')) {
        ("https", Some((host, "443"))) | ("http", Some((host, "80"))) => host.to_string(),
        _ => authority,
    };
    format!("{scheme}://{authority}{}", path.trim_end_matches('/'))
}

/// The entry side of the credential seam (F-316): the one place a typed
/// credential becomes what leaves this machine. It is sealed here to the
/// organisation's current public key -- checked against the copy this
/// computer holds, when it holds one -- for the connection's name, header
/// and address; only the sealed blob and its key version are sent, so
/// Krate's hub cannot read it.
fn seal_for_hub(
    client: &super::Client,
    slug: &str,
    name: &str,
    base: &str,
    typed: Typed,
) -> Result<serde_json::Value> {
    use krate_orgkeys::agent::Status;
    let agent = super::key_agent(client, slug)?;
    let held = match agent
        .refresh(None)
        .map_err(|err| super::key_words(err, slug))?
    {
        Status::Holds(held) => Some(held),
        Status::Waiting => None,
        Status::NoKey => anyhow::bail!(
            "nothing was sent: {slug} has not turned on encryption yet, and a credential is \
             always encrypted so that only {slug}'s computers can read it. An owner turns it on \
             once: `krate org keys init {slug}`"
        ),
    };
    let Some((version, public)) = agent
        .org_public_key(held.as_ref())
        .map_err(|err| super::key_words(err, slug))?
    else {
        anyhow::bail!(
            "nothing was sent: {slug} has not turned on encryption yet. An owner turns it on \
             once: `krate org keys init {slug}`"
        );
    };
    let context = krate_orgkeys::connection_context(slug, name, &typed.header, base);
    let sealed = krate_orgkeys::seal_for_org(&public, &context, typed.value.as_bytes())
        .map_err(|err| anyhow::anyhow!("could not encrypt the credential: {err}"))?;
    Ok(serde_json::json!({
        "credential": { "kind": typed.kind, "header": typed.header, "v": version, "sealed": sealed },
    }))
}

fn spoken_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn strings(value: &serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// One connection, as `list` and `create` print it.
fn describe(c: &serde_json::Value) -> Vec<String> {
    let methods = strings(&c["allow"]["methods"]);
    let paths = strings(&c["allow"]["paths"]);
    let audience = strings(&c["audience"]);
    let apps = strings(&c["apps"]);
    let reach = if paths == ["/"] {
        "anything under it".to_string()
    } else {
        spoken_list(&paths)
    };
    vec![
        format!("  {}  {}", text(c, "name"), text(c, "base")),
        format!("    allows {} on {reach}", spoken_list(&methods)),
        format!(
            "    for {}, in {}",
            if audience.is_empty() {
                "everyone".to_string()
            } else {
                format!(
                    "the {} group{}",
                    spoken_list(&audience),
                    if audience.len() == 1 { "" } else { "s" }
                )
            },
            if apps.is_empty() {
                "any of the organisation's apps".to_string()
            } else {
                spoken_list(&apps)
            }
        ),
        format!(
            "    credential: the {} header, set by {} {}; used {} times today, {} in 30 days",
            text(&c["credential"], "header"),
            spoken_principal(&c["credential"]["set_by"]),
            when(&c["credential"]["set_at"]),
            c["uses"]["today"].as_u64().unwrap_or(0),
            c["uses"]["last_30_days"].as_u64().unwrap_or(0),
        ),
        // A hub from before the setting says nothing: such a connection
        // tells no API anything, so nothing is claimed.
        match c["identity"].as_bool() {
            Some(true) => "    tells the API who is calling: each request carries a signed \
                           Krate-Identity header naming the person"
                .to_string(),
            Some(false) => "    does not tell the API who is calling".to_string(),
            None => String::new(),
        },
        if c["credential"]["state"] == "needs-entry" {
            format!(
                "    enter this credential again: it was saved before Krate encrypted \
                 credentials, so Krate deleted it rather than keep a copy it could read. Apps \
                 cannot use it until an owner or admin runs `krate org connection replace {} \
                 --header {}`",
                text(c, "name"),
                text(&c["credential"], "header"),
            )
        } else {
            "    encrypted: Krate cannot read this credential".to_string()
        },
    ]
    .into_iter()
    .filter(|line| !line.is_empty())
    .collect()
}

pub(super) fn run(client: &Client, me: &str, command: ConnectionCommand) -> Result<u8> {
    match command {
        ConnectionCommand::List { slug } => {
            let answer = worded(
                client.send(
                    "GET",
                    &format!("/orgs/{}/connections", seg(&slug)?),
                    &Body::Empty,
                ),
                &slug,
                me,
                Action::Connection,
            )?;
            let all = answer["connections"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            if all.is_empty() {
                println!(
                    "{slug} has no connections yet. An owner or admin sets one up with \
                     `krate org connection create {slug} <name> --base <https address> \
                     --header <name>`."
                );
                return Ok(0);
            }
            for c in &all {
                for line in describe(c) {
                    println!("{line}");
                }
            }
            Ok(0)
        }
        ConnectionCommand::Create {
            slug,
            name,
            base,
            header,
            basic,
            value_file,
            allow,
            paths,
            groups,
            apps,
            no_identity,
        } => {
            let name = name.trim().to_ascii_lowercase();
            let base = normal_base(&base);
            let typed = credential(header.as_deref(), basic.as_deref(), value_file.as_ref())?;
            let mut body = seal_for_hub(client, &slug, &name, &base, typed)?;
            body["name"] = serde_json::json!(name);
            body["base"] = serde_json::json!(base);
            body["allow"] = serde_json::json!({
                "methods": allow,
                "paths": if paths.is_empty() { vec!["/".to_string()] } else { paths },
            });
            if !groups.is_empty() {
                body["audience"] = serde_json::json!({ "groups": groups });
            }
            if !apps.is_empty() {
                body["apps"] = serde_json::json!({ "ids": apps });
            }
            if no_identity {
                body["identity"] = serde_json::json!(false);
            }
            let answer = worded(
                client.send(
                    "POST",
                    &format!("/orgs/{}/connections", seg(&slug)?),
                    &Body::Json(body),
                ),
                &slug,
                me,
                Action::Connection,
            )?;
            println!("set up the {name} connection in {slug}:");
            for line in describe(&answer["connection"]) {
                println!("{line}");
            }
            println!("An app uses it by declaring this in its manifest.toml:");
            println!("  [[capabilities]]");
            println!("  cap = \"net.connection:{name}\"");
            println!("  rationale = \"<what it reads or sends>\"");
            println!("The app never sees the credential: Krate adds it to the app's requests.");
            println!(
                "It was encrypted on this computer before it was sent: Krate cannot read it, \
                 only {slug}'s members' computers can."
            );
            Ok(0)
        }
        ConnectionCommand::Replace {
            slug,
            name,
            header,
            basic,
            value_file,
        } => {
            let name = name.trim().to_ascii_lowercase();
            let typed = credential(header.as_deref(), basic.as_deref(), value_file.as_ref())?;
            // Sealed for the address the connection has now.
            let listed = worded(
                client.send(
                    "GET",
                    &format!("/orgs/{}/connections", seg(&slug)?),
                    &Body::Empty,
                ),
                &slug,
                me,
                Action::Connection,
            )?;
            let base = listed["connections"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|c| c["name"] == name.as_str())
                .and_then(|c| c["base"].as_str())
                .map(str::to_string)
                .with_context(|| {
                    format!(
                        "{slug} has no connection called {name}: `krate org connection list \
                         {slug}` shows the ones there are"
                    )
                })?;
            let body = seal_for_hub(client, &slug, &name, &base, typed)?;
            worded(
                client.send(
                    "PUT",
                    &format!(
                        "/orgs/{}/connections/{}/credential",
                        seg(&slug)?,
                        seg(&name)?
                    ),
                    &Body::Json(body),
                ),
                &slug,
                me,
                Action::Connection,
            )?;
            println!(
                "{name} has a new credential. Every app that uses it gets the new one the next \
                 time it opens; the old one is gone from the hub."
            );
            Ok(0)
        }
        ConnectionCommand::Remove { slug, name } => {
            let name = name.trim().to_ascii_lowercase();
            worded(
                client.send(
                    "DELETE",
                    &format!("/orgs/{}/connections/{}", seg(&slug)?, seg(&name)?),
                    &Body::Empty,
                ),
                &slug,
                me,
                Action::Connection,
            )?;
            println!(
                "removed the {name} connection from {slug}, with its credential. Apps that \
                 used it are refused it from their next open."
            );
            Ok(0)
        }
    }
}
