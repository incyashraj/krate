//! Company connections at open (IC-952).
//!
//! An organisation's private app that declares `net.connection:<name>`, and
//! was granted it, gets that connection's credential from the hub when it
//! opens: once per run, for this person and these exact bytes, after the
//! access gate has let them open. The answer goes to the runtime as a
//! [`krate_runtime::connections::Connections`] and stays in this process's
//! memory for the run; the app never sees it, and nothing here prints or
//! writes it. What is kept on disk is the connection's address and allow
//! (never its credential), so that offline, or once the hub has refused, a
//! request to the address is refused in words rather than sent.
//!
//! A refusal does not stop the app opening: the person may still use what
//! does not need the connection. The app's request to the address is refused
//! with the hub's words, and the run says once, at the start, which
//! connection is not available and why.
//!
//! When the connection is set to tell the API who is calling, the hub's
//! answer also carries a short-lived identity token. The runtime adds it to
//! the app's requests to the address as `Krate-Identity`, and asks the hub
//! for a new one (GET .../connections/<name>/identity/<digest>, the same
//! checks as the credential) on a thread of its own before it runs out.

use krate_manifest::Manifest;
use krate_policy::SessionPolicy;
use krate_runtime::connections::{
    open_credential, Connection, ConnectionState, Connections, Delivered, Identity,
    IdentityRefresh, IdentityToken,
};
use serde_json::Value;

use crate::access;

/// What the hub said about one connection.
enum Answer {
    Ready(Value),
    /// A refusal: the hub's words, and the address when it gave one.
    Refused {
        words: String,
        base: Option<String>,
    },
    /// No answer that means anything: offline, or the hub is broken.
    Unreachable,
}

fn agent(url: &str) -> ureq::Agent {
    krate_http::builder(url)
        .timeout_connect(std::time::Duration::from_secs(5))
        .timeout_read(std::time::Duration::from_secs(15))
        .build()
}

fn ask(session: &access::OrgSession, name: &str, digest: &str) -> Answer {
    let url = format!(
        "{}/orgs/{}/connections/{name}/credential/{digest}",
        session.hub, session.slug
    );
    let mut request = agent(&url).get(&url).set("Accept", "application/json");
    if let Some(authorization) = &session.authorization {
        request = request.set("Authorization", authorization);
    }
    match request.call() {
        Ok(response) => match response
            .into_string()
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        {
            Some(answer)
                // The credential itself is read by `delivered` alone.
                if answer["base"].is_string() =>
            {
                Answer::Ready(answer)
            }
            _ => Answer::Unreachable,
        },
        Err(ureq::Error::Status(status, response)) if (400..500).contains(&status) => {
            let answer: Value = response
                .into_string()
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or_default();
            if answer["error"].as_str() == Some("rate-limited") {
                return Answer::Unreachable;
            }
            let message = answer["message"]
                .as_str()
                .filter(|m| !m.is_empty())
                .unwrap_or("the organisation's hub refused it");
            Answer::Refused {
                words: format!("the {name} connection is not available to this app: {message}"),
                base: answer["base"].as_str().map(str::to_string),
            }
        }
        Err(_) => Answer::Unreachable,
    }
}

fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The identity token in a hub answer (`{"identity": {"token",
/// "expires_in"}}`), timed by this computer's clock from now. `None` when
/// the connection does not tell the API who is calling, or the hub has no
/// key to sign one with.
fn identity_token(answer: &Value) -> Option<IdentityToken> {
    let identity = &answer["identity"];
    let token = identity["token"].as_str()?;
    // A compact JWT is three base64url parts: anything else is not sent.
    let compact = token.split('.').count() == 3
        && token.len() <= 4096
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.');
    let life = identity["expires_in"].as_u64()?.min(60 * 60);
    compact.then(|| IdentityToken::new(token, now_seconds() + life))
}

/// Ask the hub for a fresh identity token for this run: what the runtime
/// calls, off the app's path, when the one it holds is nearly done.
fn identity_refresh(session: &access::OrgSession, name: &str, digest: &str) -> IdentityRefresh {
    let url = format!(
        "{}/orgs/{}/connections/{name}/identity/{digest}",
        session.hub, session.slug
    );
    let authorization = session.authorization.clone();
    Box::new(move || {
        let mut request = agent(&url).get(&url).set("Accept", "application/json");
        if let Some(authorization) = &authorization {
            request = request.set("Authorization", authorization);
        }
        let answer: Value = request
            .call()
            .ok()?
            .into_string()
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())?;
        identity_token(&answer)
    })
}

/// The credential part of the hub's answer, as the runtime's seam takes it:
/// the blob sealed to the organisation's key where it was typed (F-316),
/// and what it is sealed for. The hub cannot open it; neither does this.
fn delivered(org: &str, name: &str, answer: &Value) -> Delivered {
    let c = &answer["credential"];
    Delivered {
        org: org.to_string(),
        name: name.to_string(),
        base: answer["base"].as_str().unwrap_or_default().to_string(),
        header_name: c["header"].as_str().unwrap_or_default().to_string(),
        version: c["v"]
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(0),
        sealed: c["sealed"].as_str().unwrap_or_default().to_string(),
    }
}

/// This computer's copy of the organisation's key, taken from the hub's
/// envelope for this device (and this device doing its share of the
/// keyring, as any member device online with an organisation's app open
/// does), or the plain words of why it has none yet.
fn this_computers_key(session: &access::OrgSession) -> Result<krate_orgkeys::Keyring, String> {
    use krate_orgkeys::agent::{Agent, AgentError, Status};
    let org = &session.slug;
    let device = access::org_device_key().map_err(|err| format!("{err:#}"))?;
    let agent = Agent::new(
        &session.hub,
        org,
        session.authorization.clone(),
        device,
        &access::device_name(
            crate::github_auth::current()
                .map(|identity| crate::org::principal_of(&identity.login))
                .as_deref(),
        ),
    );
    match agent.refresh(None) {
        Ok(Status::Holds(held)) => Ok(held.keyring),
        Ok(Status::Waiting) => Err(format!(
            "this computer does not have {org}'s encryption key yet, and the credential is \
             encrypted with it. It gets it by itself the next time another member's computer \
             is online with one of {org}'s apps open; then open this app again"
        )),
        Ok(Status::NoKey) => Err(format!(
            "{org} has not turned on encryption yet: an owner runs `krate org keys init {org}`"
        )),
        Err(AgentError::Offline) => Err(format!(
            "{} could not be reached for {org}'s encryption key: connect to the internet and \
             open the app again",
            session.hub
        )),
        Err(err) => Err(format!(
            "this computer could not take {org}'s encryption key: {err}"
        )),
    }
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The connections declared, granted, and asked for by name.
fn asked_for(manifest: &Manifest, policy: &SessionPolicy) -> Vec<String> {
    manifest
        .declared_capabilities()
        .unwrap_or_default()
        .into_iter()
        .filter(|cap| cap.module() == "net" && cap.action() == "connection")
        .filter(|cap| policy.allows(cap))
        .filter_map(|cap| cap.resource().map(str::to_string))
        .collect()
}

/// Resolve every connection this run may use. `private` answers, only when
/// asked, the organisation whose signed mark the bundle carries and the
/// bundle's digest: `None` for any app that is not an organisation's
/// private app, which never gets a connection. The access gate has already
/// let these bytes open.
pub fn resolve(
    manifest: Option<&Manifest>,
    policy: &SessionPolicy,
    private: impl FnOnce() -> Option<(String, String)>,
) -> Connections {
    let Some(manifest) = manifest else {
        return Connections::default();
    };
    let names = asked_for(manifest, policy);
    if names.is_empty() {
        return Connections::default();
    }
    let session = private().and_then(|(slug, digest)| {
        access::org_session(&digest)
            .filter(|s| s.slug == slug)
            .map(|session| (digest, session))
    });
    let Some((digest, session)) = session else {
        for name in &names {
            eprintln!(
                "this app asks for the company connection {name}, which only an \
                 organisation's private app can use, so it opens without it"
            );
        }
        return Connections::default();
    };
    let mut out = Vec::new();
    // Taken once, the first time the hub hands over a credential.
    let mut keyring: Option<Result<krate_orgkeys::Keyring, String>> = None;
    for name in names {
        let known = access::known_connection(&digest, &name);
        let known_base = known
            .as_ref()
            .and_then(|k| k["base"].as_str().map(str::to_string));
        let known_allow = |k: &Option<Value>| {
            k.as_ref()
                .map(|k| {
                    (
                        strings(&k["allow"]["methods"]),
                        strings(&k["allow"]["paths"]),
                    )
                })
                .unwrap_or_default()
        };
        match ask(&session, &name, &digest) {
            Answer::Ready(answer) => {
                let base = answer["base"].as_str().unwrap_or_default().to_string();
                access::remember_connection(&digest, &name, &base, &answer["allow"]);
                let identity = identity_token(&answer).map(|token| {
                    Identity::new(token, Some(identity_refresh(&session, &name, &digest)))
                });
                out.push(Connection {
                    name: name.clone(),
                    base,
                    methods: strings(&answer["allow"]["methods"]),
                    paths: strings(&answer["allow"]["paths"]),
                    // Opened in exactly one place, the runtime's side of
                    // the credential seam (F-316).
                    state: open_credential(
                        delivered(&session.slug, &name, &answer),
                        keyring
                            .get_or_insert_with(|| this_computers_key(&session))
                            .as_ref()
                            .map_err(Clone::clone),
                    )
                    .unwrap_or_else(|why| {
                        let words = format!(
                            "the {name} connection is not available to this app, which opens \
                             without it: {why}"
                        );
                        eprintln!("{words}");
                        ConnectionState::Unavailable(words)
                    }),
                    identity,
                });
            }
            Answer::Refused { words, base } => {
                eprintln!("{words}");
                if let Some(base) = base.or(known_base) {
                    let (methods, paths) = known_allow(&known);
                    out.push(Connection {
                        name: name.clone(),
                        base,
                        // Refused whatever the request: an unavailable
                        // connection refuses everything under its address.
                        methods,
                        paths,
                        state: ConnectionState::Unavailable(words),
                        identity: None,
                    });
                }
            }
            Answer::Unreachable => {
                let words = format!(
                    "the {name} connection needs {} to hand over its credential, and it could \
                     not be reached, so nothing is sent with it: connect to the internet and \
                     open the app again",
                    session.hub
                );
                eprintln!("{words}");
                if let Some(base) = known_base {
                    let (methods, paths) = known_allow(&known);
                    out.push(Connection {
                        name: name.clone(),
                        base,
                        methods,
                        paths,
                        state: ConnectionState::Unavailable(words),
                        identity: None,
                    });
                }
            }
        }
    }
    Connections::new(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_identity_token_is_taken_only_in_its_compact_shape() {
        let good =
            serde_json::json!({ "identity": { "token": "eyJh.eyJz.c2ln", "expires_in": 300 } });
        assert!(identity_token(&good).is_some());
        for bad in [
            serde_json::json!({}),
            serde_json::json!({ "identity": null }),
            serde_json::json!({ "identity": { "token": "eyJh.eyJz", "expires_in": 300 } }),
            serde_json::json!({ "identity": { "token": "a.b.c\r\nX-Evil: 1", "expires_in": 300 } }),
            serde_json::json!({ "identity": { "token": "eyJh.eyJz.c2ln" } }),
        ] {
            assert!(identity_token(&bad).is_none(), "{bad}");
        }
    }
}
