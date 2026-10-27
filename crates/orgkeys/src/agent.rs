//! A device's side of the keyring protocol with the hub
//! (`/orgs/<slug>/keys`, cloud/worker/src/org-keys.js).
//!
//! [`Agent::refresh`] is what a member device runs whenever it is online
//! with an organisation's app open: register this device if the hub does
//! not know it, take the keyring from its envelope, and do a holder's two
//! duties -- seal the keyring for every waiting device of an active member,
//! and after a removal make the next version and seal it to every
//! remaining device and the recovery key. Nobody has to act for a new
//! member's device to get the key; every grant is in the audit.
//!
//! [`Agent::init`] makes an organisation's first key on an owner's device
//! and answers the recovery code, shown once. [`Agent::recover`] restores a
//! fresh device from that code. [`Agent::org_public_key`] is what anything
//! sealing a secret for the organisation needs (connections).

use serde_json::Value;

use crate::{
    device_id, hex32, recovery_keypair, KeyError, KeyPair, Keyring, RecoveryCode, RecoveryParams,
};

/// The keyring this device holds, and the envelope it came in (kept on
/// disk by the caller: it opens only with this device's key).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Held {
    pub keyring: Keyring,
    pub envelope: String,
}

/// Where this device stands with the organisation's key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// The organisation has no key yet: an owner runs `krate org keys init`.
    NoKey,
    /// Registered and waiting for another member's device to pass the key.
    Waiting,
    /// This device holds the current version.
    Holds(Held),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentError {
    /// No answer from the hub: offline is a normal state.
    Offline,
    /// The hub refused, with its stable reason and words.
    Refused {
        status: u16,
        reason: String,
        message: String,
    },
    /// A key did not open or did not match.
    Key(KeyError),
    /// The hub answered something that is not a hub's answer.
    Hub(String),
}

impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentError::Offline => f.write_str("the hub could not be reached"),
            AgentError::Refused {
                message, reason, ..
            } if !message.is_empty() => write!(f, "{message} ({reason})"),
            AgentError::Refused { reason, .. } => write!(f, "the hub refused: {reason}"),
            AgentError::Key(error) => write!(f, "{error}"),
            AgentError::Hub(what) => write!(f, "{what}"),
        }
    }
}

impl std::error::Error for AgentError {}

impl From<KeyError> for AgentError {
    fn from(error: KeyError) -> Self {
        AgentError::Key(error)
    }
}

pub use crate::reseal_credentials;

/// One device, one organisation, one sign-in.
pub struct Agent {
    hub: String,
    slug: String,
    authorization: Option<String>,
    device: KeyPair,
    name: String,
    agent: ureq::Agent,
}

fn key_id() -> String {
    let bytes: [u8; 12] = crate::random().unwrap_or([7; 12]);
    crate::hex(&bytes)
}

impl Agent {
    /// `authorization` is `Bearer <token>`; `name` is what the dashboard
    /// lists the device as.
    pub fn new(
        hub: &str,
        slug: &str,
        authorization: Option<String>,
        device: KeyPair,
        name: &str,
    ) -> Agent {
        let hub = hub.trim_end_matches('/').to_string();
        let agent = krate_http::builder(&hub)
            .timeout_connect(std::time::Duration::from_secs(4))
            .timeout_read(std::time::Duration::from_secs(15))
            .build();
        Agent {
            hub,
            slug: slug.to_string(),
            authorization,
            device,
            name: name.to_string(),
            agent,
        }
    }

    pub fn device(&self) -> &KeyPair {
        &self.device
    }

    fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, AgentError> {
        self.call_org(method, &format!("/keys{path}"), body)
    }

    /// One request to `/orgs/<slug><path>`.
    fn call_org(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, AgentError> {
        let url = format!("{}/orgs/{}{path}", self.hub, self.slug);
        let mut request = self
            .agent
            .request(method, &url)
            .set("accept", "application/json");
        if let Some(value) = &self.authorization {
            request = request.set("authorization", value);
        }
        let sent = match body {
            Some(body) => request
                .set("content-type", "application/json")
                .set("idempotency-key", &key_id())
                .send_string(&body.to_string()),
            None => request.call(),
        };
        match sent {
            Ok(response) => {
                let text = response.into_string().map_err(|_| AgentError::Offline)?;
                if text.trim().is_empty() {
                    return Ok(Value::Null);
                }
                serde_json::from_str(&text)
                    .map_err(|_| AgentError::Hub(format!("{url} did not answer as a Krate hub")))
            }
            Err(ureq::Error::Status(status, response)) if status < 500 && status != 429 => {
                let body: Value = serde_json::from_str(&response.into_string().unwrap_or_default())
                    .unwrap_or_default();
                Err(AgentError::Refused {
                    status,
                    reason: body["error"].as_str().unwrap_or("refused").to_string(),
                    message: body["message"].as_str().unwrap_or("").to_string(),
                })
            }
            Err(_) => Err(AgentError::Offline),
        }
    }

    /// The hub's view of the keyring, for this device.
    pub fn view(&self) -> Result<Value, AgentError> {
        self.call("GET", &format!("?device={}", self.device.id()), None)
    }

    /// Register this device under the signed-in member.
    pub fn register(&self) -> Result<(), AgentError> {
        self.call(
            "PUT",
            &format!("/devices/{}", self.device.id()),
            Some(&serde_json::json!({ "public_key": self.device.public_hex(), "name": self.name })),
        )
        .map(|_| ())
    }

    fn registered_view(&self) -> Result<Value, AgentError> {
        let view = self.view()?;
        if view["device"].is_null() {
            self.register()?;
            return self.view();
        }
        Ok(view)
    }

    /// The organisation's current public key, and its version: what a
    /// secret is sealed to. Checked against the keyring this device holds
    /// when it holds one, so a hub cannot hand out a key of its own.
    pub fn org_public_key(
        &self,
        held: Option<&Held>,
    ) -> Result<Option<(u32, [u8; 32])>, AgentError> {
        let view = self.view()?;
        let Some(version) = view["current"]["version"].as_u64() else {
            return Ok(None);
        };
        let public = hex32(view["current"]["public_key"].as_str().unwrap_or(""))?;
        if let Some(held) = held {
            if let Some(mine) = held.keyring.version(version as u32) {
                if mine.public() != public {
                    return Err(AgentError::Key(KeyError::Malformed(
                        "organisation key: the hub's differs from the one this device holds",
                    )));
                }
            }
        }
        Ok(Some((version as u32, public)))
    }

    /// Bring this device's hold up to date and do a holder's duties.
    pub fn refresh(&self, held: Option<&Held>) -> Result<Status, AgentError> {
        let view = self.registered_view()?;
        let Some(current) = view["current"]["version"].as_u64().map(|v| v as u32) else {
            return Ok(Status::NoKey);
        };
        let mut held = held.cloned();
        if let Some(blob) = view["envelope"]["blob"].as_str() {
            let stale = held.as_ref().is_none_or(|h| h.envelope != blob);
            if stale {
                let keyring = Keyring::open(blob, &self.device, &self.slug)?;
                held = Some(Held {
                    keyring,
                    envelope: blob.to_string(),
                });
            }
        }
        let Some(mut held) = held else {
            return Ok(Status::Waiting);
        };
        if held.keyring.current().0 < current {
            return Ok(Status::Waiting);
        }
        // The hub's public key for the current version is the one held here.
        let public = hex32(view["current"]["public_key"].as_str().unwrap_or(""))?;
        if held.keyring.version(current).map(|k| k.public()) != Some(public) {
            return Err(AgentError::Key(KeyError::Malformed(
                "organisation key: the hub's differs from the one this device holds",
            )));
        }
        if view["current"]["state"].as_str() == Some("stale")
            && view["seal_to"].as_array().is_some_and(|d| !d.is_empty())
        {
            if let Ok(rotated) = self.rotate(&held, &view) {
                held = rotated;
            }
        } else {
            for request in view["requests"].as_array().into_iter().flatten() {
                let _ = self.grant(&held, request);
            }
        }
        Ok(Status::Holds(held))
    }

    fn seal_for(&self, keyring: &Keyring, device: &Value) -> Option<(String, String)> {
        let id = device["device_id"].as_str()?;
        let public = hex32(device["public_key"].as_str()?).ok()?;
        // The id is the key's: the hub cannot list one device and hand over
        // another's public key under its name.
        (device_id(&public) == id).then_some(())?;
        Some((id.to_string(), keyring.seal_to(&public).ok()?))
    }

    fn grant(&self, held: &Held, request: &Value) -> Result<(), AgentError> {
        let Some((id, blob)) = self.seal_for(&held.keyring, request) else {
            return Ok(());
        };
        self.call(
            "POST",
            "/grants",
            Some(&serde_json::json!({
                "device_id": id, "version": held.keyring.current().0, "blob": blob, "by_device": self.device.id(),
            })),
        )
        .map(|_| ())
    }

    fn rotate(&self, held: &Held, view: &Value) -> Result<Held, AgentError> {
        let mut next = held.keyring.clone();
        let version = next.rotate();
        let mut envelopes = serde_json::Map::new();
        for device in view["seal_to"].as_array().into_iter().flatten() {
            if let Some((id, blob)) = self.seal_for(&next, device) {
                envelopes.insert(id, Value::String(blob));
            }
        }
        let mine = next.seal_to(&self.device.public())?;
        envelopes.insert(self.device.id(), Value::String(mine.clone()));
        let recovery_blob = match view["recovery"]["public_key"].as_str() {
            Some(public) => Some(next.seal_to(&hex32(public)?)?),
            None => None,
        };
        let credentials = reseal_credentials(&next, &self.slug, &view["credentials"]);
        self.call(
            "POST",
            "/rotate",
            Some(&serde_json::json!({
                "version": version, "public_key": next.current().1.public_hex(), "by_device": self.device.id(),
                "envelopes": envelopes, "recovery_blob": recovery_blob, "credentials": credentials,
            })),
        )?;
        Ok(Held {
            keyring: next,
            envelope: mine,
        })
    }

    /// Make the organisation's first key on this device (an owner's).
    /// Answers what this device now holds and the recovery code, which is
    /// shown once and never stored.
    pub fn init(&self) -> Result<(Held, RecoveryCode), AgentError> {
        self.registered_view()?;
        let keyring = Keyring::create(&self.slug);
        let code = RecoveryCode::generate()?;
        let params = RecoveryParams::generate()?;
        let recovery = recovery_keypair(&code, &params)?;
        let envelope = keyring.seal_to(&self.device.public())?;
        let recovery_blob = keyring.seal_to(&recovery.public())?;
        self.call(
            "POST",
            "/init",
            Some(&serde_json::json!({
                "device_id": self.device.id(), "public_key": keyring.current().1.public_hex(), "envelope": envelope,
                "recovery": { "public_key": recovery.public_hex(), "params": params.to_json(), "blob": recovery_blob },
            })),
        )?;
        Ok((Held { keyring, envelope }, code))
    }

    /// Restore this device from the recovery code (an owner's or admin's).
    pub fn recover(&self, code: &RecoveryCode) -> Result<Held, AgentError> {
        self.registered_view()?;
        let stored = self.call("GET", "/recovery", None)?;
        let params = RecoveryParams::from_json(&stored["params"])?;
        let recovery = recovery_keypair(code, &params)?;
        if Some(recovery.public_hex().as_str()) != stored["public_key"].as_str() {
            return Err(AgentError::Key(KeyError::Open));
        }
        let keyring = Keyring::open(stored["blob"].as_str().unwrap_or(""), &recovery, &self.slug)?;
        let envelope = keyring.seal_to(&self.device.public())?;
        self.call(
            "POST",
            "/grants",
            Some(&serde_json::json!({
                "device_id": self.device.id(), "version": keyring.current().0, "blob": envelope, "by_device": "recovery",
            })),
        )?;
        Ok(Held { keyring, envelope })
    }
}

/// The signing ring's background duties and the publisher's side
/// (IC-958 A), on the same device registration and the same hub client.
mod ring;
pub use ring::{Duties, HeldRing, Publisher, RingStatus};

#[cfg(test)]
mod tests {
    use super::*;

    /// After a removal: what the remaining devices re-seal opens with the
    /// new version, and not with the keyring the removed member kept.
    #[test]
    fn a_rotation_reseals_every_credential_beyond_a_removed_members_reach() {
        let mut ring = Keyring::create("acme");
        let kept_by_removed = ring.clone();
        let base = "https://api.acme.example/finance";
        let context = crate::connection_context("acme", "finance-api", "Authorization", base);
        let sealed =
            crate::seal_for_org(&ring.current().1.public(), &context, b"Bearer s3cret").unwrap();
        let listed = serde_json::json!([
            { "name": "finance-api", "base": base, "header": "Authorization", "v": 1, "sealed": sealed },
            // The hub moved this one to another address: it no longer opens.
            { "name": "payroll", "base": "https://evil.example", "header": "Authorization", "v": 1, "sealed": sealed },
        ]);
        ring.rotate();
        let out = reseal_credentials(&ring, "acme", &listed);
        assert_eq!(out["payroll"], serde_json::json!({ "unopened": true }));
        assert_eq!(out["finance-api"]["v"], 2);
        let resealed = out["finance-api"]["sealed"].as_str().unwrap();
        assert_ne!(resealed, sealed);
        assert_eq!(
            crate::open_for_org(&ring, 2, &context, resealed).unwrap(),
            b"Bearer s3cret"
        );
        for v in [1, 2] {
            assert_eq!(
                crate::open_for_org(&kept_by_removed, v, &context, resealed),
                Err(KeyError::Open),
                "the removed member's keyring, version {v}"
            );
        }
    }
}
