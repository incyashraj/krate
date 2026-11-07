//! A signed-in person's own app data on every device they have (IC-981,
//! founder decision F-321): the same reminders on the Mac and the phone.
//!
//! Stage S1 is `store.kv`; stage S3 the app's own files ([`files`]: whole
//! files under the same rule, their bodies apart in the hub's object
//! store). The parts:
//!
//! - [`vv`]: version vectors -- one counter per device -- which say whether
//!   two writes are one after the other or were made apart. Never a wall
//!   clock: a phone whose clock is an hour out must not win or lose a write
//!   for that.
//! - [`replica`]: one app's data on one device -- what the app wrote here,
//!   what came from the other devices, and the conflicts the person has not
//!   answered yet -- with the merge rule and the four resolutions (keep this
//!   device's, keep the other's, keep both, merge).
//! - [`seal`]: what leaves the device. The account's data key (a
//!   crates/orgkeys keyring, made by the first device) derives one key per
//!   app; every entry's name is an HMAC and its value is sealed, so the hub
//!   learns neither the app nor the key names nor the values.
//! - `hub` and `device` (feature `agent`): the device's side of the hub's
//!   `/account/sync` routes (cloud/worker/src/account-sync.js) and its files
//!   on disk -- its key pair, the data key it holds, the recovery code to
//!   show once -- and approving a new device or recovering with the code.
//! - `overview` (feature `agent`): what Krate's home and `krate account
//!   sync` show -- this device, the others, who waits for a yes -- kept for
//!   when the hub is out of reach.
//!
//! The hub keeps, per (account, app), every version no other version has
//! replaced (cloud/worker/src/account-sync.js, the same "a newer vector
//! replaces an older one, two made apart are both kept" rule as
//! [`vv::VersionVector::covers`]), so a conflict is never decided on the
//! hub, and nothing is lost there while a person decides.

pub mod files;
pub mod json_merge;
pub mod readable;
pub mod replica;
pub mod seal;
pub mod toggles;
pub mod vv;

#[cfg(feature = "agent")]
pub mod device;
#[cfg(feature = "agent")]
pub mod hub;
#[cfg(feature = "agent")]
pub mod overview;

pub use replica::{Conflict, Replica, Resolution, ResolveError, Version};
pub use vv::{Order, VersionVector};

/// The SHA-256 of a value, lower-case hex: how the replica remembers what
/// the store holds without keeping a second copy of it.
pub fn value_hash(value: &[u8]) -> String {
    use sha2::Digest;
    krate_orgkeys::hex(&sha2::Sha256::digest(value))
}

/// What a sync bucket is keyed by besides the account: the app's verified
/// publisher (the signing root a signature that held names), or `unsigned`.
/// The same on every device, because it comes from the bundle: an app that
/// claims another's id but is not signed by its publisher lands in a
/// different bucket (IC-736's storage principal).
pub fn publisher_label(verified_root: Option<&str>) -> String {
    match verified_root {
        Some(root) if !root.trim().is_empty() => root.trim().to_ascii_lowercase(),
        _ => "unsigned".to_string(),
    }
}
