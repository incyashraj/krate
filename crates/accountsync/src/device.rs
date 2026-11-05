//! This device and the account's data key: its key pair on disk, the key it
//! holds, the recovery code waiting to be shown once, and the two ways a
//! new device gets the key -- another device approves it, or the recovery
//! code opens it.
//!
//! The data key passes between devices only sealed to the receiving
//! device's public key (HPKE, crates/orgkeys). Unlike an organisation's
//! keyring, nothing hands it over by itself: a new device waits until the
//! person, on a device that holds the key, says yes to it ("Let Galaxy S22
//! Ultra read your apps' data?") -- S2 puts that question in Krate's home;
//! [`approve`] is what answering it does.
//!
//! On disk, under the player's account-sync folder, one folder per account
//! (named by a hash of the hub's account id): `device.json` (the device's
//! private key, sealed with the machine key Krate keeps for store.secret),
//! `held.json` (the envelope, which opens only with that key) and, until it
//! is shown, `recovery-code` (sealed the same way).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use krate_orgkeys::{
    device_id, recovery_keypair, KeyError, KeyPair, Keyring, RecoveryCode, RecoveryParams,
};
use serde_json::Value;

use crate::hub::{Hub, HubError};
use crate::seal::{keyring_label, new_account_key};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncError {
    Hub(HubError),
    Key(KeyError),
    Io(String),
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncError::Hub(error) => write!(f, "{error}"),
            SyncError::Key(error) => write!(f, "{error}"),
            SyncError::Io(error) => f.write_str(error),
        }
    }
}

impl std::error::Error for SyncError {}

impl From<HubError> for SyncError {
    fn from(error: HubError) -> Self {
        SyncError::Hub(error)
    }
}

impl From<KeyError> for SyncError {
    fn from(error: KeyError) -> Self {
        SyncError::Key(error)
    }
}

fn io(error: std::io::Error) -> SyncError {
    SyncError::Io(error.to_string())
}

const DEVICE_CONTEXT: &str = "krate account sync device key";
const CODE_CONTEXT: &str = "krate account sync recovery code";

/// This device, signed in to one account.
pub struct Account {
    /// The hub's id for the account.
    pub id: String,
    /// This device's key pair for this account.
    pub device: KeyPair,
    /// What the account's device list calls this device.
    pub name: String,
    dir: PathBuf,
    machine_key: Vec<u8>,
}

impl std::fmt::Debug for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Account({}, device {})", self.id, self.device.id())
    }
}

/// Where this device stands with the account's data key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyStatus {
    /// It holds the key: its apps' data syncs.
    Holds(Keyring),
    /// Registered, and waiting for another device to approve it (or for the
    /// recovery code).
    Waiting,
}

/// A device of the account that does not hold the key yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingDevice {
    pub device_id: String,
    pub name: String,
    pub added_at: u64,
}

/// The folder one account's files live in: a hash of its id, so an id is
/// never a path.
pub use crate::toggles::slot;

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn write_private(path: &Path, text: &str) -> Result<(), SyncError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io)?;
    }
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&temp, text).map_err(io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&temp, path).map_err(io)
}

fn seal_at_rest(machine_key: &[u8], context: &str, plain: &[u8]) -> Result<Value, SyncError> {
    if machine_key.is_empty() {
        return Ok(serde_json::json!({ "plain": krate_orgkeys::hex(plain) }));
    }
    Ok(serde_json::json!({ "sealed": krate_orgkeys::seal_local(machine_key, context, plain)? }))
}

fn open_at_rest(machine_key: &[u8], context: &str, kept: &Value) -> Result<Vec<u8>, SyncError> {
    if let Some(sealed) = kept["sealed"].as_str() {
        return Ok(krate_orgkeys::open_local(machine_key, context, sealed)?);
    }
    kept["plain"]
        .as_str()
        .and_then(krate_orgkeys::unhex)
        .ok_or(SyncError::Key(KeyError::Malformed("device key file")))
}

impl Account {
    fn dir(&self) -> &Path {
        &self.dir
    }

    /// This device's files for `account`, its key pair made the first time.
    pub fn load(
        device_dir: &Path,
        account: &str,
        machine_key: &[u8],
        name: &str,
    ) -> Result<Account, SyncError> {
        let dir = device_dir.join(slot(account));
        let path = dir.join("device.json");
        let device = match read_json(&path) {
            Some(kept) => {
                let secret = open_at_rest(machine_key, DEVICE_CONTEXT, &kept)?;
                KeyPair::from_secret(&secret)?
            }
            None => {
                let pair = KeyPair::generate();
                let kept = seal_at_rest(machine_key, DEVICE_CONTEXT, pair.secret())?;
                write_private(&path, &kept.to_string())?;
                pair
            }
        };
        Ok(Account {
            id: account.to_string(),
            device,
            name: name.to_string(),
            dir,
            machine_key: machine_key.to_vec(),
        })
    }

    /// The key this device held last time, without asking the hub.
    pub fn held(&self) -> Option<Keyring> {
        let kept = read_json(&self.dir().join("held.json"))?;
        Keyring::open(
            kept["envelope"].as_str()?,
            &self.device,
            &keyring_label(&self.id),
        )
        .ok()
    }

    fn keep_held(&self, envelope: &str) -> Result<(), SyncError> {
        write_private(
            &self.dir().join("held.json"),
            &serde_json::json!({ "envelope": envelope }).to_string(),
        )
    }

    fn keep_code(&self, code: &RecoveryCode) -> Result<(), SyncError> {
        let kept = seal_at_rest(&self.machine_key, CODE_CONTEXT, code.display().as_bytes())?;
        write_private(&self.dir().join("recovery-code"), &kept.to_string())
    }

    /// Is a recovery code waiting to be shown?
    pub fn recovery_code_waiting(&self) -> bool {
        self.dir().join("recovery-code").exists()
    }

    /// The recovery code made at first set-up, once: it is gone from this
    /// device after this answers it. Krate shows it and asks the person to
    /// write it down.
    pub fn take_recovery_code(&self) -> Option<String> {
        let path = self.dir().join("recovery-code");
        let kept = read_json(&path)?;
        let code = open_at_rest(&self.machine_key, CODE_CONTEXT, &kept).ok()?;
        let _ = std::fs::remove_file(&path);
        String::from_utf8(code).ok()
    }

    /// Register this device if the hub does not know it, take the key if a
    /// device approved it, and make the account's first key if there is
    /// none -- this is then the first device, and a recovery code waits to
    /// be shown.
    pub fn refresh(&self, hub: &Hub) -> Result<KeyStatus, SyncError> {
        self.refresh_with_view(hub).map(|(status, _)| status)
    }

    /// [`Account::refresh`], with the hub's view it ended on: the device
    /// list, for the names "keep both" puts on a key.
    pub fn refresh_with_view(&self, hub: &Hub) -> Result<(KeyStatus, Value), SyncError> {
        let id = self.device.id();
        let mut view = hub.view(Some(&id))?;
        if view["device"].is_null() {
            hub.register(&id, &self.device.public_hex(), &self.name)?;
            view = hub.view(Some(&id))?;
        }
        if view["key"].is_null() {
            match self.first_key(hub) {
                Ok(ring) => return Ok((KeyStatus::Holds(ring), view)),
                // Another device made it a moment ago: take the answer.
                Err(SyncError::Hub(HubError::Refused { reason, .. })) if reason == "key-exists" => {
                    view = hub.view(Some(&id))?;
                }
                Err(other) => return Err(other),
            }
        }
        let Some(blob) = view["envelope"]["blob"].as_str() else {
            return Ok((KeyStatus::Waiting, view));
        };
        let ring = Keyring::open(blob, &self.device, &keyring_label(&self.id))?;
        check_public(&ring, &view)?;
        self.keep_held(blob)?;
        Ok((KeyStatus::Holds(ring), view))
    }

    fn first_key(&self, hub: &Hub) -> Result<Keyring, SyncError> {
        let ring = new_account_key(&self.id);
        let code = RecoveryCode::generate()?;
        let params = RecoveryParams::generate()?;
        let recovery = recovery_keypair(&code, &params)?;
        let envelope = ring.seal_to(&self.device.public())?;
        let recovery_blob = ring.seal_to(&recovery.public())?;
        hub.init(&serde_json::json!({
            "device_id": self.device.id(),
            "version": ring.current().0,
            "public_key": ring.current().1.public_hex(),
            "envelope": envelope,
            "recovery": { "public_key": recovery.public_hex(), "params": params.to_json(), "blob": recovery_blob },
        }))?;
        self.keep_held(&envelope)?;
        self.keep_code(&code)?;
        Ok(ring)
    }

    /// The devices waiting for the key, for Krate to ask about.
    pub fn pending(&self, hub: &Hub) -> Result<Vec<PendingDevice>, SyncError> {
        let view = hub.view(Some(&self.device.id()))?;
        Ok(pending_in(&view, &self.device.id()))
    }

    /// Yes to a waiting device: seal the key to its public key. The public
    /// key is checked against its id, so the hub cannot list one device and
    /// hand over another's key under its name.
    pub fn approve(&self, hub: &Hub, ring: &Keyring, device: &str) -> Result<(), SyncError> {
        let view = hub.view(Some(&self.device.id()))?;
        check_public(ring, &view)?;
        let listed = view["devices"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|d| d["device_id"].as_str() == Some(device))
            .ok_or(SyncError::Hub(HubError::Refused {
                status: 404,
                reason: "device-unknown".into(),
                message: "that device is not one of this account's".into(),
            }))?;
        let public = krate_orgkeys::unhex(listed["public_key"].as_str().unwrap_or(""))
            .and_then(|b| <[u8; 32]>::try_from(b.as_slice()).ok())
            .ok_or(SyncError::Key(KeyError::Malformed("device public key")))?;
        if device_id(&public) != device {
            return Err(SyncError::Key(KeyError::Malformed(
                "the hub listed a public key that is not that device's",
            )));
        }
        let blob = ring.seal_to(&public)?;
        hub.grant(device, ring.current().0, &blob, &self.device.id())?;
        Ok(())
    }

    /// No other device at hand: the recovery code opens the key for this
    /// one.
    pub fn recover(&self, hub: &Hub, code: &str) -> Result<Keyring, SyncError> {
        let code = RecoveryCode::parse(code).ok_or(SyncError::Key(KeyError::Malformed(
            "recovery code: 26 letters and digits",
        )))?;
        let id = self.device.id();
        if hub.view(Some(&id))?["device"].is_null() {
            hub.register(&id, &self.device.public_hex(), &self.name)?;
        }
        let stored = hub.recovery()?;
        let params = RecoveryParams::from_json(&stored["params"])?;
        let recovery = recovery_keypair(&code, &params)?;
        if Some(recovery.public_hex().as_str()) != stored["public_key"].as_str() {
            return Err(SyncError::Key(KeyError::Open));
        }
        let ring = Keyring::open(
            stored["blob"].as_str().unwrap_or(""),
            &recovery,
            &keyring_label(&self.id),
        )?;
        let envelope = ring.seal_to(&self.device.public())?;
        hub.grant(&id, ring.current().0, &envelope, "recovery")?;
        self.keep_held(&envelope)?;
        Ok(ring)
    }
}

/// The hub's public key for the current version is the one this device
/// holds: a hub cannot slip in a key of its own.
fn check_public(ring: &Keyring, view: &Value) -> Result<(), SyncError> {
    let version = view["key"]["version"].as_u64().unwrap_or(0) as u32;
    let public = view["key"]["public_key"].as_str().unwrap_or("");
    match ring.version(version) {
        Some(mine) if mine.public_hex() == public => Ok(()),
        _ => Err(SyncError::Key(KeyError::Malformed(
            "account key: the hub's differs from the one this device holds",
        ))),
    }
}

/// The devices in `view` that do not hold the current key, except `me`.
pub fn pending_in(view: &Value, me: &str) -> Vec<PendingDevice> {
    let current = view["key"]["version"].as_u64();
    view["devices"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|d| d["device_id"].as_str() != Some(me))
        .filter(|d| current.is_some() && d["holds"].as_u64() != current)
        .filter_map(|d| {
            Some(PendingDevice {
                device_id: d["device_id"].as_str()?.to_string(),
                name: d["name"].as_str().unwrap_or("").to_string(),
                added_at: d["added_at"].as_u64().unwrap_or(0),
            })
        })
        .collect()
}

/// The account's device names by id, for "keep both".
pub fn device_names(view: &Value) -> BTreeMap<String, String> {
    view["devices"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| {
            Some((
                d["device_id"].as_str()?.to_string(),
                d["name"].as_str().unwrap_or("").to_string(),
            ))
        })
        .collect()
}

/// Which account a sign-in is: the hub's id for it.
pub fn account_of(hub: &Hub) -> Result<String, SyncError> {
    let view = hub.view(None)?;
    view["account"]
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .ok_or_else(|| SyncError::Hub(HubError::Hub("the hub named no account".into())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_device_key_is_kept_sealed_and_comes_back_the_same() {
        let dir = tempfile::tempdir().unwrap();
        let key = vec![7u8; 32];
        let first = Account::load(dir.path(), "u1", &key, "Mac").unwrap();
        let again = Account::load(dir.path(), "u1", &key, "Mac").unwrap();
        assert_eq!(first.device, again.device);
        let text =
            std::fs::read_to_string(dir.path().join(slot("u1")).join("device.json")).unwrap();
        assert!(!text.contains(&krate_orgkeys::hex(first.device.secret())));
        assert!(
            Account::load(dir.path(), "u1", &[9u8; 32], "Mac").is_err(),
            "another machine key"
        );
        let other = Account::load(dir.path(), "u2", &key, "Mac").unwrap();
        assert_ne!(other.device, first.device, "one key pair per account");
    }

    #[test]
    fn the_recovery_code_is_shown_once() {
        let dir = tempfile::tempdir().unwrap();
        let account = Account::load(dir.path(), "u1", &[7u8; 32], "Mac").unwrap();
        let code = RecoveryCode::generate().unwrap();
        account.keep_code(&code).unwrap();
        assert!(account.recovery_code_waiting());
        assert_eq!(account.take_recovery_code(), Some(code.display()));
        assert_eq!(account.take_recovery_code(), None);
        assert!(!account.recovery_code_waiting());
    }

    #[test]
    fn pending_lists_only_devices_without_the_current_key() {
        let view = serde_json::json!({
            "key": { "version": 1 },
            "devices": [
                { "device_id": "aaaaaaaaaaaaaaaa", "name": "Mac", "holds": 1 },
                { "device_id": "bbbbbbbbbbbbbbbb", "name": "Galaxy S22", "holds": null, "added_at": 5 },
                { "device_id": "cccccccccccccccc", "name": "iPhone", "holds": null },
            ]
        });
        let pending = pending_in(&view, "cccccccccccccccc");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].name, "Galaxy S22");
        assert_eq!(device_names(&view)["aaaaaaaaaaaaaaaa"], "Mac");
    }
}
