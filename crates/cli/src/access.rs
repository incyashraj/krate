//! Opening an organisation's private app on this computer (IC-948 G and H,
//! Addendum 5).
//!
//! The rules -- the check-in, the offline lease, the terms, this device's
//! organisation key, where the team data lives -- are crates/orgaccess's,
//! the same code the phone Players run (K-1171). What is this computer's is
//! here: `~/.krate`, the GitHub sign-in, the hub a policy pins, the words a
//! refusal is put in, and the native alert a double-click shows.
//!
//! Every door that runs an app -- `krate run`, `krate launch`, a double-click
//! through the Player, Studio's Open, the MCP tools -- reaches [`gate_opened`]
//! once the bytes are in hand. Leases live in `~/.krate/leases/<digest>.json`.

use std::path::PathBuf;

use anyhow::Result;

use crate::{github_auth, org};

pub use krate_orgaccess::{mark_of, Bytes, Mark, Opened};

/// How a door is being used: from a terminal, or with nobody at one (a
/// double-click), where the words also go in a native alert.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Door {
    Terminal,
    Window,
}

/// This computer, as the organisation rules see it: its `~/.krate`, its
/// sign-in, its hub, and the door the app is being opened through.
pub struct Computer {
    pub door: Door,
}

/// The computer as seen from a door nobody watches with a native alert:
/// what every caller that only reads or keeps something uses.
pub const HERE: Computer = Computer {
    door: Door::Terminal,
};

impl krate_orgaccess::Device for Computer {
    fn home(&self) -> PathBuf {
        crate::krate_home()
    }
    fn machine_key(&self) -> Vec<u8> {
        crate::machine_key()
    }
    fn signed_in(&self) -> Option<krate_orgaccess::SignedIn> {
        github_auth::current().map(|identity| krate_orgaccess::SignedIn {
            login: identity.login,
            token: identity.token,
        })
    }
    fn hub(&self) -> std::result::Result<String, krate_orgaccess::GateError> {
        org::hub_url(None).map_err(|err| match err.downcast_ref::<crate::audit::Refused>() {
            Some(refused) => krate_orgaccess::GateError::Refused(krate_orgaccess::Refusal {
                reason: refused.reason.clone(),
                words: refused.words.clone(),
            }),
            None => krate_orgaccess::GateError::Failed(format!("{err:#}")),
        })
    }
    fn pinned_hub(&self) -> Option<String> {
        crate::fleet::pinned_hub().map(str::to_string)
    }
    fn kind(&self) -> &str {
        "computer"
    }
    fn name(&self, principal: Option<&str>) -> String {
        device_name(principal)
    }
    fn sign_in_words(&self) -> String {
        "sign in with `krate account login`".to_string()
    }
    fn tell(&self, title: &str, words: &str) {
        tell(self.door, title, words);
    }
    fn note(&self, words: &str) {
        eprintln!("{words}");
    }
    fn is_hub_link(&self, url: &str) -> bool {
        org::is_hub_link(url)
    }
    fn refusal_words(&self, url: &str, reason: &str, message: &str, org: Option<&str>) -> String {
        org::download_refusal(url, reason, message, org)
    }
}

/// Offline access for `digest`, made ready ahead of the first open (IC-953
/// background readiness). See [`krate_orgaccess::prefetch_lease`].
pub fn prefetch_lease(
    hub: &str,
    org: (&str, &str),
    digest: &str,
    authorization: &str,
    me: &str,
) -> std::result::Result<Option<u64>, String> {
    krate_orgaccess::prefetch_lease(&HERE, hub, org, digest, authorization, me)
}

/// Delete this app's team data in `slug` on this computer (G17). Answers
/// how many files went.
pub fn wipe_team_data(slug: &str, app_id: &str) -> usize {
    krate_orgaccess::wipe_team_data(&HERE, slug, app_id)
}

/// This computer's key for organisations' keyrings (F-316), kept in
/// `~/.krate/org-keys/device.json` sealed with this computer's machine key.
pub fn org_device_key() -> Result<krate_orgkeys::KeyPair> {
    krate_orgaccess::org_device_key(&HERE).map_err(|words| anyhow::anyhow!(words))
}

/// What an organisation's device list calls this computer.
pub fn device_name(principal: Option<&str>) -> String {
    let who = principal
        .map(|p| p.split_once(':').map_or(p, |(_, v)| v))
        .unwrap_or("someone");
    format!("{who}'s {} computer", std::env::consts::OS)
}

/// May these bytes open? `Ok` at once, with no request anywhere, for any
/// bundle without an organisation's mark; for a marked one, `Ok` only when
/// this person may open it now, with what is known about the private app
/// it let through (`None` for an unmarked app). A refusal is an
/// [`crate::audit::Refused`], shown in an alert at a window door.
pub fn gate_opened(bytes: Bytes, mark: Mark, door: Door) -> Result<Option<Opened>> {
    krate_orgaccess::gate_opened(&Computer { door }, bytes, mark).map_err(|err| match err {
        krate_orgaccess::GateError::Refused(refusal) => {
            alert_refusal(door, &refusal.words);
            anyhow::Error::new(crate::audit::Refused::new(&refusal.reason, refusal.words))
        }
        krate_orgaccess::GateError::Failed(words) => anyhow::anyhow!(words),
    })
}

/// Show `words` where the person is looking: always on stderr, and in a
/// native alert when nobody is at a terminal to read it.
pub fn tell(door: Door, title: &str, words: &str) {
    eprintln!("{words}");
    alert(door, title, words);
}

/// The native alert alone, for a window door nobody is watching from a
/// terminal.
fn alert(door: Door, title: &str, words: &str) {
    alert_where(door, title, words, true);
}

/// `on_windows` is false for a refusal: on Windows `main` already puts any
/// refusal in a dialog when nobody can read stderr, and two would be one
/// too many.
fn alert_where(door: Door, title: &str, words: &str, on_windows: bool) {
    if door != Door::Window {
        return;
    }
    #[cfg(target_os = "macos")]
    if !stderr_is_watched() {
        krate_adapter_macos::present_message(title, words);
    }
    #[cfg(windows)]
    if on_windows {
        // The same test as `main`'s dialog: no console and nobody reading
        // stderr is a double-click (K-178, K-240).
        let unwatched =
            unsafe { windows_sys::Win32::System::Console::GetConsoleWindow().is_null() }
                && !crate::stderr_has_a_reader();
        if unwatched {
            let _ = rfd::MessageDialog::new()
                .set_title(title)
                .set_description(words)
                .set_buttons(rfd::MessageButtons::Ok)
                .show();
        }
    }
    let _ = (title, words, on_windows);
}

/// Is somebody reading stderr -- a terminal, a pipe, a file? Then a modal
/// alert would only block a process nobody can see (K-240). A double-click
/// sends stderr to /dev/null, which is none of them.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn stderr_is_watched() -> bool {
    use std::io::IsTerminal;
    if std::io::stderr().is_terminal() {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(std::io::stderr().as_raw_fd(), &mut stat) } == 0 {
            let kind = stat.st_mode & libc::S_IFMT;
            return kind == libc::S_IFIFO || kind == libc::S_IFREG || kind == libc::S_IFSOCK;
        }
        true
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// The refusal alert alone, for the machine policy's gate (IC-949), which
/// refuses through the same door machinery as this one.
pub fn alert_refusal(door: Door, words: &str) {
    alert_where(door, "This app did not open", words, false);
}

/// The words for a refused download: one place for every door that
/// fetches (launch, MCP), as `krate run` already does. A refusal that ends
/// access also ends the lease for the bytes the link names.
pub fn refused_words(err: &krate_bundle::BundleError, url: &str) -> Option<String> {
    match err {
        krate_bundle::BundleError::HubRefused {
            reason,
            message,
            org,
            ..
        } => {
            refused_link(url, reason);
            Some(org::download_refusal(url, reason, message, org.as_deref()))
        }
        _ => None,
    }
}

/// A link's download was refused: when the refusal ends access, the lease
/// for the bytes it names goes too, exactly as a refused check-in does.
pub fn refused_link(url: &str, reason: &str) {
    krate_orgaccess::refused_link(&HERE, url, reason);
}
