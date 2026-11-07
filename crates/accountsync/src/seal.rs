//! What leaves the device, and how it is opened again.
//!
//! The account's data key is a crates/orgkeys [`Keyring`] made by the first
//! device, labelled with the account, and sealed by HPKE to each device's
//! public key and to the recovery code's key; the hub holds only those
//! envelopes. From it, per app:
//!
//! - **the bucket's name**: an HMAC of (publisher, app id), so the hub keeps
//!   a person's apps apart without learning which apps they are;
//! - **each entry's name**: an HMAC of the key, the same on every device,
//!   so the hub can keep a key's versions together without learning it;
//! - **each entry's value**: the key and the value (or a deletion), sealed
//!   with XChaCha20-Poly1305 under associated data binding it to that name,
//!   to whether it is a value or a deletion, to the key's version and to
//!   its version vector and writer. A hub that moved an entry, changed its
//!   vector or swapped a value for a deletion hands a device something that
//!   does not open, which the device ignores.

use krate_orgkeys::{AppDataKey, KeyError, Keyring, RowKind};

use crate::replica::Version;
use crate::vv::VersionVector;

/// The label an account's keyring carries: an envelope for one account
/// does not open as another's.
pub fn keyring_label(account: &str) -> String {
    format!("krate account {account}")
}

/// A fresh data key for an account: what its first device makes.
pub fn new_account_key(account: &str) -> Keyring {
    Keyring::create(&keyring_label(account))
}

/// One version as the hub keeps it: the vector and writer in the clear
/// (the hub compares vectors), the rest sealed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sealed {
    pub vv: VersionVector,
    pub by: String,
    pub deleted: bool,
    /// The version of the account key it was sealed with.
    pub k: u32,
    /// Base64 of the nonce and ciphertext.
    pub c: String,
}

impl Sealed {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "vv": self.vv.to_json(),
            "by": self.by,
            "kind": if self.deleted { "t" } else { "v" },
            "k": self.k,
            "c": self.c,
        })
    }

    pub fn from_json(value: &serde_json::Value) -> Option<Sealed> {
        let by = value["by"].as_str()?;
        if !crate::vv::valid_device_id(by) {
            return None;
        }
        Some(Sealed {
            vv: VersionVector::from_json(&value["vv"])?,
            by: by.to_string(),
            deleted: match value["kind"].as_str()? {
                "v" => false,
                "t" => true,
                _ => return None,
            },
            k: u32::try_from(value["k"].as_u64()?).ok()?,
            c: value["c"].as_str()?.to_string(),
        })
    }
}

/// One app's keys, for one account.
pub struct AppKey {
    keyring: Keyring,
    scope: String,
    bucket: String,
}

impl std::fmt::Debug for AppKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AppKey({})", self.bucket)
    }
}

impl AppKey {
    /// The keys of the app `app_id` by `publisher` ([`crate::publisher_label`]).
    pub fn derive(keyring: &Keyring, publisher: &str, app_id: &str) -> AppKey {
        let scope = format!("account kv\n{publisher}\n{app_id}");
        // The bucket's name from the first version of the account key, so
        // it stays the same if the key is ever replaced.
        let first = *keyring.versions().first().expect("a keyring has a version");
        let names = AppDataKey::derive(keyring, first, "account buckets")
            .expect("the keyring holds its own first version");
        let bucket = names.record_key("bucket", &format!("{publisher}\n{app_id}"));
        AppKey {
            keyring: keyring.clone(),
            scope,
            bucket,
        }
    }

    /// The bucket's name on the hub: 64 hex.
    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    fn data(&self, version: u32) -> Option<AppDataKey> {
        AppDataKey::derive(&self.keyring, version, &self.scope)
    }

    fn current(&self) -> AppDataKey {
        self.data(self.keyring.current().0)
            .expect("the keyring holds its current version")
    }

    /// The name an entry for `key` goes by on the hub: 64 hex, the same on
    /// every device of the account, under every version of its key.
    pub fn name_of(&self, key: &str) -> String {
        let first = *self.keyring.versions().first().expect("a version");
        self.data(first)
            .expect("the keyring holds its first version")
            .record_key("kv", key)
    }

    fn bound(vv: &VersionVector, by: &str) -> String {
        format!("{}|{by}", vv.canonical())
    }

    /// Seal one version of `key`.
    pub fn seal(&self, key: &str, version: &Version) -> Result<Sealed, KeyError> {
        let data = self.current();
        let name = self.name_of(key);
        let deleted = version.value.is_none();
        let plain = serde_json::json!({
            "k": key,
            "v": version.value.as_deref().map(krate_orgkeys::b64_encode),
        })
        .to_string();
        let c = data.seal(
            &name,
            &Self::bound(&version.vv, &version.by),
            if deleted {
                RowKind::Tombstone
            } else {
                RowKind::Value
            },
            plain.as_bytes(),
        )?;
        Ok(Sealed {
            vv: version.vv.clone(),
            by: version.by.clone(),
            deleted,
            k: data.version(),
            c,
        })
    }

    /// Open one version the hub keeps under `name`: its key and version.
    /// Refuses anything that does not open, or that names a key whose name
    /// is not `name`.
    pub fn open(&self, name: &str, sealed: &Sealed) -> Result<(String, Version), KeyError> {
        let data = self.data(sealed.k).ok_or(KeyError::Open)?;
        let plain = data.open(
            name,
            &Self::bound(&sealed.vv, &sealed.by),
            if sealed.deleted {
                RowKind::Tombstone
            } else {
                RowKind::Value
            },
            &sealed.c,
        )?;
        let value: serde_json::Value =
            serde_json::from_slice(&plain).map_err(|_| KeyError::Malformed("sealed entry"))?;
        let key = value["k"]
            .as_str()
            .ok_or(KeyError::Malformed("sealed entry"))?
            .to_string();
        if self.name_of(&key) != name {
            return Err(KeyError::Open);
        }
        let bytes = match (&value["v"], sealed.deleted) {
            (serde_json::Value::Null, true) => None,
            (serde_json::Value::String(text), false) => {
                Some(krate_orgkeys::b64_decode(text).ok_or(KeyError::Malformed("sealed entry"))?)
            }
            _ => return Err(KeyError::Malformed("sealed entry")),
        };
        Ok((
            key,
            Version {
                value: bytes,
                vv: sealed.vv.clone(),
                by: sealed.by.clone(),
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "aaaaaaaaaaaaaaaa";

    fn version(value: Option<&[u8]>, n: u64) -> Version {
        let mut vv = VersionVector::new();
        for _ in 0..n {
            vv.bump(A);
        }
        Version {
            value: value.map(<[u8]>::to_vec),
            vv,
            by: A.to_string(),
        }
    }

    #[test]
    fn an_entry_opens_with_the_accounts_key_and_names_its_key() {
        let ring = new_account_key("u1");
        let app = AppKey::derive(&ring, "unsigned", "dev.krate.reminders");
        let v = version(Some(b"Buy milk"), 1);
        let sealed = app.seal("reminder:1", &v).unwrap();
        let name = app.name_of("reminder:1");
        assert_eq!(
            app.open(&name, &sealed).unwrap(),
            ("reminder:1".to_string(), v)
        );
        // A deletion carries its key too.
        let gone = version(None, 2);
        let sealed = app.seal("reminder:1", &gone).unwrap();
        assert!(sealed.deleted);
        assert_eq!(app.open(&name, &sealed).unwrap().1, gone);
        // And the wire form round-trips.
        assert_eq!(Sealed::from_json(&sealed.to_json()).unwrap(), sealed);
    }

    #[test]
    fn nothing_the_hub_holds_carries_the_key_or_the_value() {
        let ring = new_account_key("u1");
        let app = AppKey::derive(&ring, "unsigned", "dev.krate.reminders");
        let sealed = app
            .seal("reminder:secret-key", &version(Some(b"Buy milk"), 1))
            .unwrap();
        let wire = serde_json::json!({ "bucket": app.bucket(), "name": app.name_of("reminder:secret-key"), "entry": sealed.to_json() }).to_string();
        for plain in [
            "reminder",
            "secret-key",
            "Buy milk",
            "dev.krate.reminders",
            "unsigned",
        ] {
            assert!(
                !wire.contains(plain),
                "the hub's bytes carry {plain:?}: {wire}"
            );
            let b64 = krate_orgkeys::b64_encode(plain.as_bytes());
            assert!(
                !wire.contains(&b64[..b64.len().saturating_sub(4)]),
                "nor in base64"
            );
        }
    }

    #[test]
    fn another_accounts_key_and_a_changed_entry_do_not_open() {
        let ring = new_account_key("u1");
        let other = new_account_key("u1");
        let app = AppKey::derive(&ring, "unsigned", "dev.krate.reminders");
        let stranger = AppKey::derive(&other, "unsigned", "dev.krate.reminders");
        let sealed = app.seal("r", &version(Some(b"x"), 1)).unwrap();
        let name = app.name_of("r");
        assert_ne!(
            app.bucket(),
            stranger.bucket(),
            "another key, another bucket"
        );
        assert_eq!(stranger.open(&name, &sealed), Err(KeyError::Open));
        // The hub moving it to another name, inflating its vector, or
        // calling it a deletion: none of it opens.
        assert_eq!(app.open(&app.name_of("s"), &sealed), Err(KeyError::Open));
        let mut inflated = sealed.clone();
        inflated.vv.bump(A);
        assert_eq!(app.open(&name, &inflated), Err(KeyError::Open));
        let mut flipped = sealed.clone();
        flipped.deleted = true;
        assert_eq!(app.open(&name, &flipped), Err(KeyError::Open));
        // Another app of the same account is another bucket and key.
        let notes = AppKey::derive(&ring, "unsigned", "dev.krate.notes");
        assert_ne!(notes.bucket(), app.bucket());
        assert_eq!(notes.open(&name, &sealed), Err(KeyError::Open));
        // And another publisher's app claiming the same id.
        let impostor = AppKey::derive(&ring, "ab12", "dev.krate.reminders");
        assert_ne!(impostor.bucket(), app.bucket());
        assert_eq!(impostor.open(&name, &sealed), Err(KeyError::Open));
    }

    #[test]
    fn a_devices_key_opens_the_envelope_and_a_wrong_one_does_not() {
        let ring = new_account_key("u1");
        let phone = krate_orgkeys::KeyPair::generate();
        let laptop = krate_orgkeys::KeyPair::generate();
        let envelope = ring.seal_to(&phone.public()).unwrap();
        let opened = Keyring::open(&envelope, &phone, &keyring_label("u1")).unwrap();
        assert_eq!(opened, ring);
        assert_eq!(
            Keyring::open(&envelope, &laptop, &keyring_label("u1")),
            Err(KeyError::Open),
            "a device it was not sealed to"
        );
        assert!(
            Keyring::open(&envelope, &phone, &keyring_label("u2")).is_err(),
            "another account's label"
        );
    }
}
