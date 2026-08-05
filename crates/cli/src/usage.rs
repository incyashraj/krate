//! Counting how many people use Krate, without learning anything about them.
//!
//! The question worth answering is "are people making apps, and is that number
//! growing". That needs a count of distinct installs and nothing else, so this
//! sends nothing else.
//!
//! **What is sent:** a random id generated on this machine, the Krate version,
//! the operating system name, and which of a fixed set of actions happened
//! (`make`, `open`, `publish`). That is the whole payload.
//!
//! **What is never sent:** no app names, no prompts, no file paths, no
//! usernames, no email, no IP-derived location, no machine identifiers of any
//! kind. The id is random bytes written to a file -- it is not derived
//! from hardware, a MAC address, or a hostname, so it cannot be traced back to
//! a person or correlated with anything outside Krate. Deleting the file makes
//! this install a new anonymous one.
//!
//! **It is opt-out and says so.** The first run prints one line explaining what
//! is counted and how to turn it off, and `KRATE_NO_USAGE=1` or a `no-usage`
//! file stops it permanently. It is fire-and-forget on a background thread with
//! a short timeout, so it can never slow down or break a command -- a metric
//! that costs the user something is not worth having.

use std::path::PathBuf;
use std::time::Duration;

/// Actions worth counting. A closed set, deliberately: a free-form string
/// would eventually carry something about the person.
#[derive(Debug, Clone, Copy)]
pub enum Action {
    /// Someone made an app.
    Make,
    /// Someone opened an app.
    Open,
    /// Someone published an app.
    Publish,
}

impl Action {
    fn as_str(self) -> &'static str {
        match self {
            Action::Make => "make",
            Action::Open => "open",
            Action::Publish => "publish",
        }
    }
}

fn krate_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".krate"))
}

fn opted_out() -> bool {
    if std::env::var_os("KRATE_NO_USAGE").is_some() {
        return true;
    }
    // Also honour the convention every other tool respects, so someone who has
    // already said "no telemetry" system-wide does not have to say it again.
    if std::env::var_os("DO_NOT_TRACK").is_some() {
        return true;
    }
    krate_dir()
        .map(|dir| dir.join("no-usage").exists())
        .unwrap_or(false)
}

/// The random id for this install, made on first use.
///
/// Deliberately random rather than derived from anything about the machine.
/// A hash of a hostname or MAC address would be stable across reinstalls and
/// linkable to a person; this is neither.
fn install_id() -> Option<String> {
    let dir = krate_dir()?;
    let path = dir.join("install-id");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let trimmed = existing.trim().to_string();
        if !trimmed.is_empty() {
            return Some(trimmed);
        }
    }
    let id = random_hex();
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::write(&path, &id).ok()?;
    Some(id)
}

/// Sixteen random bytes as hex.
///
/// Reads /dev/urandom where there is one, falling back to mixing the clock
/// with the process id. This value only has to be unlikely to collide with
/// another install: it authorises nothing and protects nothing.
fn random_hex() -> String {
    if let Ok(file) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let mut bytes = [0u8; 16];
        let mut handle = file;
        if handle.read_exact(&mut bytes).is_ok() {
            return bytes.iter().map(|b| format!("{b:02x}")).collect();
        }
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}{:x}", std::process::id())
        .chars()
        .cycle()
        .take(32)
        .collect()
}

/// Tell the person, once, before anything is ever sent.
///
/// Printed on first use rather than buried in documentation: someone who would
/// object should find out at the moment it starts, not later.
fn announce_once() -> bool {
    let Some(dir) = krate_dir() else {
        return false;
    };
    let marker = dir.join("usage-notice-shown");
    if marker.exists() {
        return true;
    }
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(&marker, "1");
    eprintln!();
    eprintln!("  Krate counts how many people use it: a random id, the version,");
    eprintln!("  and which of make/open/publish happened. No names, no prompts,");
    eprintln!("  no file paths, nothing about you or your apps.");
    eprintln!("  Turn it off with KRATE_NO_USAGE=1 -- everything still works.");
    eprintln!();
    true
}

/// Record that something happened. Returns immediately.
pub fn record(action: Action) {
    if opted_out() {
        return;
    }
    if !announce_once() {
        return;
    }
    let Some(id) = install_id() else {
        return;
    };

    let endpoint = std::env::var("KRATE_USAGE_URL")
        .unwrap_or_else(|_| "https://hub.krate.tech/usage".to_string());
    let version = crate::KRATE_VERSION;
    let os = std::env::consts::OS;
    let action = action.as_str();

    // Detached, with a short timeout. A count is never worth making someone
    // wait, and a hub that is down or slow must not be able to hang a command
    // or change its exit code.
    std::thread::spawn(move || {
        let body = format!(
            r#"{{"id":"{id}","version":"{version}","os":"{os}","action":"{action}"}}"#
        );
        let _ = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(3))
            .timeout_read(Duration::from_secs(3))
            .build()
            .post(&endpoint)
            .set("content-type", "application/json")
            .send_string(&body);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_are_a_closed_set_of_plain_words() {
        // Free-form strings are how identifying detail leaks into a metric.
        assert_eq!(Action::Make.as_str(), "make");
        assert_eq!(Action::Open.as_str(), "open");
        assert_eq!(Action::Publish.as_str(), "publish");
    }

    #[test]
    fn a_random_id_is_the_right_shape_and_not_a_constant() {
        let a = random_hex();
        assert_eq!(a.len(), 32, "got {a}");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        // Two installs sharing an id would make the count meaningless.
        assert_ne!(a, random_hex());
    }
}
