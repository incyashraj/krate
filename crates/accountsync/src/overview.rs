//! What a person sees about sync on this device, and what they do there
//! besides the toggles (IC-981 S2): say yes to a device that is waiting,
//! type the recovery code when no other device can, and put the code away
//! once it is written down.
//!
//! Krate's home on a phone and `krate account sync` on a computer both come
//! here, so a choice made on either does the same thing.
//!
//! Offline is normal. [`look`] keeps what the hub last said in
//! `<device dir>/overview.json` -- the account, this device and the others,
//! never a key or a code -- and answers that, saying the hub was out of
//! reach, when it cannot ask. The account's id is what finds an app's
//! conflicts on disk, so they can be shown and answered with no network.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::device::{account_of, Account, KeyStatus, SyncError};
use crate::hub::{Hub, HubError};
use krate_orgkeys::KeyError;

/// How the last look at the hub went.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Reach {
    /// The hub answered just now.
    #[default]
    Answered,
    /// No answer: what is shown is what it last said.
    Offline,
    /// It answered no, or this device could not do its part: in words.
    Failed(String),
}

/// One of the account's other devices.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Known {
    pub id: String,
    pub name: String,
    /// It holds the account's key: it reads the apps' data. A device that
    /// does not is waiting for a yes.
    pub holds: bool,
}

/// This device and the account, as the hub last described them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Overview {
    /// The hub's id for the account; `None` before the hub ever answered.
    pub account: Option<String>,
    pub device_id: String,
    pub device_name: String,
    /// This device holds the account's key.
    pub holds_key: bool,
    /// The account's other devices, oldest first.
    pub devices: Vec<Known>,
    /// When the hub last answered, in seconds since 1970.
    pub checked_at: u64,
    #[serde(skip)]
    pub reach: Reach,
}

impl Overview {
    /// The names every device id goes by, this one's included: for "keep
    /// both" and for saying where a change came from.
    pub fn names(&self) -> std::collections::BTreeMap<String, String> {
        let mut names: std::collections::BTreeMap<String, String> = self
            .devices
            .iter()
            .map(|d| (d.id.clone(), d.name.clone()))
            .collect();
        if !self.device_id.is_empty() {
            names.insert(self.device_id.clone(), self.device_name.clone());
        }
        names
    }

    /// The devices waiting for a yes.
    pub fn waiting(&self) -> impl Iterator<Item = &Known> {
        self.devices.iter().filter(|d| !d.holds)
    }
}

/// Where the last answer is kept.
pub fn cache_path(device_dir: &Path) -> PathBuf {
    device_dir.join("overview.json")
}

/// What the hub last said, with no network.
pub fn cached(device_dir: &Path) -> Option<Overview> {
    serde_json::from_str(&std::fs::read_to_string(cache_path(device_dir)).ok()?).ok()
}

fn keep(device_dir: &Path, overview: &Overview) {
    let Ok(text) = serde_json::to_string(overview) else {
        return;
    };
    let _ = std::fs::create_dir_all(device_dir);
    let path = cache_path(device_dir);
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    if std::fs::write(&temp, text).is_ok() {
        let _ = std::fs::rename(&temp, &path);
    }
}

/// This device's side of the sync: who it is, signed in with `token`.
pub struct Me<'a> {
    pub device_dir: &'a Path,
    pub hub: &'a str,
    pub token: &'a str,
    /// The key the device's private key is sealed with at rest (empty:
    /// kept plain, as on an iPhone today).
    pub machine_key: &'a [u8],
    /// What the account's device list calls this device.
    pub name: &'a str,
}

impl Me<'_> {
    fn connect(&self) -> Result<(Hub, Account), SyncError> {
        let hub = Hub::new(self.hub, self.token);
        let id = account_of(&hub)?;
        let account = Account::load(self.device_dir, &id, self.machine_key, self.name)?;
        Ok((hub, account))
    }

    fn account_offline(&self) -> Option<Account> {
        let id = cached(self.device_dir)?.account?;
        Account::load(self.device_dir, &id, self.machine_key, self.name).ok()
    }
}

/// Ask the hub how this device stands -- registering it the first time,
/// taking the key if another device approved it, making the account's
/// first key if there is none -- and keep the answer. With no answer, the
/// last one, marked [`Reach::Offline`] (or [`Reach::Failed`]).
pub fn look(me: &Me<'_>, now: u64) -> Overview {
    match look_now(me, now) {
        Ok(overview) => {
            keep(me.device_dir, &overview);
            overview
        }
        Err(error) => {
            let mut overview = cached(me.device_dir).unwrap_or_default();
            overview.reach = match error {
                SyncError::Hub(HubError::Offline) => Reach::Offline,
                other => Reach::Failed(in_words(&other)),
            };
            overview
        }
    }
}

fn look_now(me: &Me<'_>, now: u64) -> Result<Overview, SyncError> {
    let (hub, account) = me.connect()?;
    let (status, view) = account.refresh_with_view(&hub)?;
    let mine = account.device.id();
    let current = view["key"]["version"].as_u64();
    let devices = view["devices"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|d| d["device_id"].as_str() != Some(mine.as_str()))
        .filter_map(|d| {
            Some(Known {
                id: d["device_id"].as_str()?.to_string(),
                name: d["name"].as_str().unwrap_or("").trim().to_string(),
                holds: current.is_some() && d["holds"].as_u64() == current,
            })
        })
        .collect();
    Ok(Overview {
        account: Some(account.id.clone()),
        device_id: mine,
        device_name: account.name.clone(),
        holds_key: matches!(status, KeyStatus::Holds(_)),
        devices,
        checked_at: now,
        reach: Reach::Answered,
    })
}

/// Yes to a waiting device: from now on it reads the apps' data.
pub fn approve(me: &Me<'_>, device: &str) -> Result<(), String> {
    let run = || -> Result<(), SyncError> {
        let (hub, account) = me.connect()?;
        let KeyStatus::Holds(ring) = account.refresh(&hub)? else {
            return Err(SyncError::Io(
                "this device cannot say yes for another yet: it is waiting for a yes itself".into(),
            ));
        };
        account.approve(&hub, &ring, device.trim())
    };
    run().map_err(|e| in_words(&e))
}

/// No other device at hand: the recovery code opens the key here.
pub fn recover(me: &Me<'_>, code: &str) -> Result<(), String> {
    let run = || -> Result<(), SyncError> {
        let (hub, account) = me.connect()?;
        account.recover(&hub, code).map(|_| ())
    };
    run().map_err(|e| match e {
        SyncError::Key(KeyError::Open) => {
            "That code did not open your data. Check it against what you wrote down and try again."
                .to_string()
        }
        SyncError::Key(KeyError::Malformed(_)) => {
            "A recovery code is 26 letters and numbers, like ABCDE-FGHJK-...".to_string()
        }
        other => in_words(&other),
    })
}

/// The recovery code waiting to be shown on this device, if this is the
/// device that made the account's key and the person has not put it away.
/// Read with no network, from the account the hub last named.
pub fn recovery_code(me: &Me<'_>) -> Option<String> {
    me.account_offline()?.recovery_code()
}

/// The person wrote the code down: it is gone from this device.
pub fn forget_recovery_code(me: &Me<'_>) {
    if let Some(account) = me.account_offline() {
        account.forget_recovery_code();
    }
}

/// A failure as a person reads it, with what to do about it.
pub fn in_words(error: &SyncError) -> String {
    match error {
        SyncError::Hub(HubError::Offline) => {
            "Krate can't reach the internet right now. Your apps keep working here, and sync when it's back."
                .to_string()
        }
        SyncError::Hub(HubError::Refused { status, .. }) if *status == 401 => {
            "Your sign-in has run out on this device. Sign out and in again to keep syncing."
                .to_string()
        }
        SyncError::Hub(HubError::Refused { reason, .. }) if reason == "device-unknown" => {
            "That device is no longer one of yours.".to_string()
        }
        SyncError::Hub(HubError::Refused { reason, .. }) if reason == "too-many-devices" => {
            "Your account already syncs to as many devices as it can. Sign out on one you no longer use."
                .to_string()
        }
        SyncError::Hub(HubError::Refused {
            message, reason, ..
        }) => {
            if message.is_empty() {
                format!("Krate's server said no ({reason}). Try again in a while.")
            } else {
                let mut words = message.clone();
                if let Some(first) = words.get_mut(0..1) {
                    first.make_ascii_uppercase();
                }
                format!("{words}.")
            }
        }
        SyncError::Hub(HubError::Hub(_)) => {
            "Krate's server gave an answer this Krate does not understand. Try again in a while, or update Krate."
                .to_string()
        }
        SyncError::Key(KeyError::Open) | SyncError::Key(KeyError::Malformed(_)) => {
            "This device's sync keys do not open. Sign out and in again; your apps' data on this device stays."
                .to_string()
        }
        SyncError::Key(KeyError::Random) => {
            "This device gave Krate no randomness to make a key with. Restart it and try again."
                .to_string()
        }
        SyncError::Io(what) => {
            let mut words = what.clone();
            if let Some(first) = words.get_mut(0..1) {
                first.make_ascii_uppercase();
            }
            format!("{words}.")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_the_hub_away_the_last_answer_is_shown_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let me = Me {
            device_dir: dir.path(),
            hub: "http://127.0.0.1:9",
            token: "gho_x",
            machine_key: &[1; 32],
            name: "Phone",
        };
        let first = look(&me, 5);
        assert_eq!(first.reach, Reach::Offline);
        assert_eq!(first.account, None, "never answered: nothing to show");
        let kept = Overview {
            account: Some("u1".into()),
            device_id: "aaaaaaaaaaaaaaaa".into(),
            device_name: "Phone".into(),
            holds_key: true,
            devices: vec![
                Known {
                    id: "bbbbbbbbbbbbbbbb".into(),
                    name: "Mac".into(),
                    holds: true,
                },
                Known {
                    id: "cccccccccccccccc".into(),
                    name: "iPad".into(),
                    holds: false,
                },
            ],
            checked_at: 4,
            reach: Reach::Answered,
        };
        keep(dir.path(), &kept);
        let again = look(&me, 9);
        assert_eq!(again.reach, Reach::Offline);
        assert_eq!(again.account.as_deref(), Some("u1"));
        assert_eq!(again.checked_at, 4, "when it last answered");
        assert_eq!(again.waiting().count(), 1);
        assert_eq!(again.names()["aaaaaaaaaaaaaaaa"], "Phone");
        assert_eq!(again.names()["bbbbbbbbbbbbbbbb"], "Mac");
        assert!(!std::fs::read_to_string(cache_path(dir.path()))
            .unwrap()
            .contains("reach"));
    }

    #[test]
    fn every_failure_reads_as_words_with_what_to_do() {
        let offline = in_words(&SyncError::Hub(HubError::Offline));
        assert!(offline.contains("keep working"));
        let expired = in_words(&SyncError::Hub(HubError::Refused {
            status: 401,
            reason: "sign-in".into(),
            message: "sign in".into(),
        }));
        assert!(expired.contains("Sign out and in again"));
        let said = in_words(&SyncError::Hub(HubError::Refused {
            status: 403,
            reason: "key-not-held".into(),
            message: "only a device that holds the key can pass it on".into(),
        }));
        assert_eq!(said, "Only a device that holds the key can pass it on.");
        for error in [
            SyncError::Key(KeyError::Open),
            SyncError::Key(KeyError::Random),
            SyncError::Hub(HubError::Hub("x".into())),
            SyncError::Io("disk full".into()),
        ] {
            let words = in_words(&error);
            assert!(words.ends_with('.'), "{words}");
            assert!(!words.contains("envelope") && !words.contains("bucket"));
        }
    }
}
