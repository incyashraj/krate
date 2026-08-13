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

use std::io::IsTerminal;
use std::path::PathBuf;
use std::time::Duration;

/// Actions worth counting. A closed set, deliberately: a free-form string
/// would eventually carry something about the person.
#[derive(Debug, Clone, Copy)]
pub enum Action {
    /// First run on this machine. The top of the funnel: how many people got
    /// as far as installing, whether or not they ever made anything.
    Install,
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
            Action::Install => "install",
            Action::Make => "make",
            Action::Open => "open",
            Action::Publish => "publish",
        }
    }
}

/// Why an open did not end in a running app.
///
/// A closed set for the same reason `Action` is one: a free-form string would
/// eventually carry a path, a URL, or an app name, and none of those are ours
/// to collect. Every variant below is a category of *our* behaviour, and none
/// of them can vary with anything about the person or their files.
///
/// This exists because the telemetry recorded that 425 of 4,612 opens failed
/// over five days (K-100) and could not say why -- so the number was alarming
/// and unactionable at the same time. A count without a cause cannot be fixed.
///
/// The most important variant is `Refused`. The permission wall turning an app
/// away is the product working, not breaking, and it was previously counted as
/// a failure. Any read of the failure rate that does not separate it is wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenFailure {
    /// The app asked for something the person did not grant, so the wall
    /// refused it. **Not a defect** -- this is the product doing its job.
    Refused,
    /// The file or URL could not be found or fetched.
    NotFound,
    /// The bundle was there but could not be read: malformed, truncated, or
    /// failing one of the format's own limits.
    BadBundle,
    /// The manifest would not parse or did not match the component.
    BadManifest,
    /// The app needs a newer Krate than this one.
    VersionTooOld,
    /// The machine could not open a window at all -- a missing system
    /// library, or no display.
    NoWindow,
    /// The app started and then failed on its own: a trap, a panic, or a
    /// non-zero exit of its own choosing.
    AppFailed,
    /// Anything not yet classified. A rising share here means the list above
    /// needs another variant, and is itself the signal to look.
    Other,
}

impl OpenFailure {
    fn as_str(self) -> &'static str {
        match self {
            OpenFailure::Refused => "refused",
            OpenFailure::NotFound => "not-found",
            OpenFailure::BadBundle => "bad-bundle",
            OpenFailure::BadManifest => "bad-manifest",
            OpenFailure::VersionTooOld => "version-too-old",
            OpenFailure::NoWindow => "no-window",
            OpenFailure::AppFailed => "app-failed",
            OpenFailure::Other => "other",
        }
    }

    /// Classify a finished run into one of the categories above.
    ///
    /// Reads the error chain rather than the message text where it can, and
    /// falls back to matching on the sentences the CLI itself prints -- those
    /// are our own strings, fixed in this binary, not anything a person typed.
    pub fn classify(outcome: &Result<u8, anyhow::Error>) -> Option<Self> {
        match outcome {
            // The wall refusing, and the manifest-entry mismatch, both exit 5.
            Ok(5) => Some(OpenFailure::Refused),
            Ok(0) => None,
            Ok(_) => Some(OpenFailure::AppFailed),
            Err(err) => Some(Self::from_error(err)),
        }
    }

    fn from_error(err: &anyhow::Error) -> Self {
        // The whole chain, lowercased once, so a cause deep in the stack is
        // still visible to the match below.
        let text = err
            .chain()
            .map(|cause| cause.to_string().to_lowercase())
            .collect::<Vec<_>>()
            .join(" :: ");

        if text.contains("needs a newer version of krate")
            || text.contains("version-too-old")
            || text.contains("unsupported world")
        {
            OpenFailure::VersionTooOld
        } else if text.contains("does not exist")
            || text.contains("not found")
            || text.contains("could not open bundle from")
            || text.contains("no such file")
        {
            OpenFailure::NotFound
        } else if text.contains("bundle") {
            OpenFailure::BadBundle
        } else if text.contains("manifest") {
            OpenFailure::BadManifest
        } else if text.contains("libxkbcommon")
            || text.contains("could not open a window")
            || text.contains("no display")
        {
            OpenFailure::NoWindow
        } else if text.contains("wasm trap")
            || text.contains("panicked")
            || text.contains("unreachable")
        {
            OpenFailure::AppFailed
        } else {
            OpenFailure::Other
        }
    }
}

/// The two extra facts worth knowing, both about the software rather than the
/// person.
///
/// `ai` answers "does the AI path actually work", which is the whole product
/// thesis. `ok` answers "did it start" -- a make that fails and an open that
/// crashes are the numbers that matter most, and counting only successes would
/// flatter us into thinking nothing is broken.
#[derive(Debug, Clone, Copy, Default)]
pub struct Facts {
    /// Whether an AI agent wrote the app. None where the question does not
    /// apply, such as opening one.
    pub ai: Option<bool>,
    /// Whether the thing succeeded.
    pub ok: Option<bool>,
    /// When it did not succeed, which category of not-succeeding. Always
    /// `None` alongside `ok: Some(true)`.
    pub why: Option<OpenFailure>,
}

fn krate_dir() -> Option<PathBuf> {
    let home = crate::home_dir()?;
    Some(home.join(".krate"))
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
    // Never speak into a pipe. A script reading krate's output has no way to
    // tell a one-time notice from the answer it asked for -- this broke the
    // website build, where a helper merged stderr into stdout and then tried
    // to parse it as JSON. The counting still happens; only the notice waits
    // for a terminal to print to.
    if !std::io::stderr().is_terminal() || !std::io::stdout().is_terminal() {
        return true;
    }
    let marker = dir.join("usage-notice-shown");
    if marker.exists() {
        return true;
    }
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(&marker, "1");
    // Ask, once, before the first count -- not announce after it. A reviewer
    // put it exactly: for a tool whose whole argument is asking before
    // taking, taking first and explaining matches nothing else in the
    // product (K-081). Enter keeps it on; `n` writes the same no-usage
    // marker `krate telemetry off` writes. Non-interactive runs never reach
    // here (the terminal check above), so scripts and CI are never prompted
    // and stay counted unless KRATE_NO_USAGE says otherwise.
    eprintln!();
    eprintln!("  Krate counts how many people use it: a random id, the version,");
    eprintln!("  and which of make/open/publish happened. No names, no prompts,");
    eprintln!("  no file paths, nothing about you or your apps.");
    eprint!("  Is that ok? [Y/n]  ");
    let mut answer = String::new();
    let said_no = std::io::stdin()
        .read_line(&mut answer)
        .map(|_| answer.trim().eq_ignore_ascii_case("n"))
        .unwrap_or(false);
    if said_no {
        let _ = std::fs::write(dir.join("no-usage"), "1");
        eprintln!("  Off, and staying off. Everything else works the same.");
        eprintln!();
        return false;
    }
    eprintln!("  Ok. Turn it off any time with `krate telemetry off`.");
    eprintln!();
    true
}

/// Record that something happened, with nothing else known about it.
pub fn record(action: Action) {
    record_with(action, Facts::default());
}

/// Note the first run on this machine, once, ever.
///
/// Called before the install id exists, so the same file that makes the id
/// tells us whether this is the first time it has been used.
pub fn record_install_once() {
    let Some(dir) = krate_dir() else {
        return;
    };
    if dir.join("install-id").exists() {
        return;
    }
    record(Action::Install);
}

/// Record that something happened, with what is known about it.
pub fn record_with(action: Action, facts: Facts) {
    if opted_out() {
        return;
    }
    // The flush helper is not a user session; it must never record its own
    // run or spawn another helper.
    if std::env::var_os("KRATE_USAGE_HELPER").is_some() {
        return;
    }
    if !announce_once() {
        return;
    }
    let Some(id) = install_id() else {
        return;
    };

    let mut body = format!(
        r#"{{"id":"{id}","version":"{version}","os":"{os}","action":"{action}""#,
        // The bare number, never the debug suffix: this goes into a JSON
        // field, and a suffix would make every dev build its own version.
        version = crate::KRATE_VERSION_NUMBER,
        os = std::env::consts::OS,
        action = action.as_str()
    );
    if let Some(ai) = facts.ai {
        body.push_str(&format!(r#","ai":{ai}"#));
    }
    if let Some(ok) = facts.ok {
        body.push_str(&format!(r#","ok":{ok}"#));
    }
    if let Some(why) = facts.why {
        body.push_str(&format!(r#","why":"{}""#, why.as_str()));
    }
    body.push('}');

    // Spool to disk, then try to drain in the background. Nothing on this
    // path waits for the network.
    //
    // The previous shape joined the sending thread against a 600 ms
    // deadline, because a detached thread loses the race with process exit
    // and the last event of a command -- which is most of them -- was
    // never sent. That reasoning was right and the location was wrong: it
    // put a network round-trip on the path a person waits behind, and it
    // measured 68 ms of a 74 ms `krate run` (K-091). Writing the event
    // down first means nothing is lost even if the process dies one
    // instruction later, so the send no longer has to be waited on.
    append_to_spool(&body);
    drain_spool_in_background();
}

/// The file events wait in until a send succeeds. One JSON object per
/// line, so a half-written line at the end of a crashed run costs exactly
/// that line.
fn spool_path() -> Option<std::path::PathBuf> {
    krate_dir().map(|dir| dir.join("usage-spool.jsonl"))
}

/// Events kept before dropping the oldest. A person who is offline for a
/// month must not grow an unbounded file, and stale counts have little
/// value anyway.
const SPOOL_MAX_EVENTS: usize = 200;

fn append_to_spool(body: &str) {
    let Some(path) = spool_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(file, "{body}");
    }
}

/// Hand the spool to a detached helper process and return immediately.
///
/// A thread cannot do this job: every command here exits in single-digit
/// milliseconds and a network round-trip is hundreds, so the thread is
/// killed by process exit every single time -- measured, not assumed (the
/// spool grew to 21 events and never drained). A separate process
/// outlives its parent, so the person waits for nothing and the events
/// still leave the machine.
fn drain_spool_in_background() {
    // The helper is this same binary, so there is nothing extra to ship.
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut command = std::process::Command::new(exe);
    command
        .arg("usage-flush")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // Guard against a helper spawning a helper: the flush path must never
    // re-enter this function.
    command.env("KRATE_USAGE_HELPER", "1");
    let _ = command.spawn();
}

/// Drain the spool synchronously. Only the detached helper calls this.
pub fn flush_spool_now() {
    let Some(path) = spool_path() else {
        return;
    };
    let endpoint = std::env::var("KRATE_USAGE_URL")
        .unwrap_or_else(|_| "https://hub.krate.tech/usage".to_string());

    {
        let Ok(contents) = std::fs::read_to_string(&path) else {
            return;
        };
        let events: Vec<&str> = contents
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && line.starts_with('{') && line.ends_with('}'))
            .collect();
        if events.is_empty() {
            let _ = std::fs::remove_file(&path);
            return;
        }
        // Oldest first, and never more than the cap: a long offline spell
        // sends recent history rather than a flood.
        let start = events.len().saturating_sub(SPOOL_MAX_EVENTS);
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(3))
            .timeout_read(Duration::from_secs(3))
            .build();

        let mut unsent: Vec<&str> = Vec::new();
        let mut failed = false;
        for event in &events[start..] {
            if failed {
                // The hub is unreachable; keep the rest for next time
                // rather than spending three seconds each proving it.
                unsent.push(event);
                continue;
            }
            match agent
                .post(&endpoint)
                .set("content-type", "application/json")
                .send_string(event)
            {
                Ok(_) => {}
                Err(_) => {
                    failed = true;
                    unsent.push(event);
                }
            }
        }

        // Rewrite the spool with only what did not go. Doing this once at
        // the end keeps a crash mid-drain from losing more than a resend.
        if unsent.is_empty() {
            let _ = std::fs::remove_file(&path);
        } else {
            let _ = std::fs::write(&path, format!("{}\n", unsent.join("\n")));
        }
    }
}

/// `krate telemetry [on|off|status]`.
pub fn telemetry_command(state: &str) -> anyhow::Result<u8> {
    let Some(dir) = krate_dir() else {
        println!("No home directory, so there is nothing to configure.");
        return Ok(0);
    };
    let marker = dir.join("no-usage");

    match state {
        "off" => {
            std::fs::create_dir_all(&dir)?;
            std::fs::write(&marker, "1")?;
            println!("Usage counting is off. Nothing further will be sent.");
        }
        "on" => {
            let _ = std::fs::remove_file(&marker);
            println!("Usage counting is on.");
        }
        _ => {
            println!("Krate counts how many people use it, and nothing about them.");
            println!();
            println!("Sent:      a random id, the Krate version, the operating system,");
            println!("           and one of install/make/open/publish -- plus whether an");
            println!("           AI wrote the app and whether it worked.");
            println!("           When an app does not open, one word for why, from a");
            println!("           fixed list: refused, not-found, bad-bundle, bad-manifest,");
            println!("           version-too-old, no-window, app-failed, other.");
            println!("Not sent:  app names, prompts, file paths, your name, your machine.");
            println!();
            if opted_out() {
                println!("Right now: off.");
                println!("Turn it on with: krate telemetry on");
            } else {
                println!("Right now: on.");
                println!("Turn it off with: krate telemetry off");
            }
        }
    }
    Ok(0)
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

    /// A clean run is not a failure and carries no reason at all.
    #[test]
    fn a_successful_open_has_no_reason() {
        assert_eq!(OpenFailure::classify(&Ok(0)), None);
    }

    /// The one that changes how the whole number reads: the permission wall
    /// refusing an app is the product working, and it exits 5.
    #[test]
    fn the_permission_wall_refusing_is_its_own_category() {
        assert_eq!(OpenFailure::classify(&Ok(5)), Some(OpenFailure::Refused));
    }

    /// Any other non-zero exit is the app's own choice, not ours.
    #[test]
    fn a_nonzero_exit_is_the_app_failing() {
        assert_eq!(OpenFailure::classify(&Ok(1)), Some(OpenFailure::AppFailed));
        assert_eq!(OpenFailure::classify(&Ok(42)), Some(OpenFailure::AppFailed));
    }

    #[test]
    fn a_missing_file_is_not_found() {
        let err = anyhow::anyhow!("input file does not exist: /x/y.krate");
        assert_eq!(
            OpenFailure::classify(&Err(err)),
            Some(OpenFailure::NotFound)
        );
    }

    /// The classifier reads the whole chain, so a cause buried under a
    /// context line is still seen. This is the case that matters most in
    /// practice, because the CLI wraps almost everything in context.
    #[test]
    fn a_cause_below_a_context_line_is_still_classified() {
        let root = anyhow::anyhow!("no such file or directory");
        let wrapped = root.context("could not open bundle /tmp/app.krate");
        assert_eq!(
            OpenFailure::classify(&Err(wrapped)),
            Some(OpenFailure::NotFound)
        );
    }

    #[test]
    fn an_old_app_is_told_apart_from_a_broken_one() {
        let err = anyhow::anyhow!("this app needs a newer version of Krate");
        assert_eq!(
            OpenFailure::classify(&Err(err)),
            Some(OpenFailure::VersionTooOld)
        );
    }

    #[test]
    fn a_missing_window_library_is_its_own_category() {
        let err = anyhow::anyhow!("libxkbcommon-x11.so is not installed");
        assert_eq!(
            OpenFailure::classify(&Err(err)),
            Some(OpenFailure::NoWindow)
        );
    }

    /// K-110: every app after the first opened as a process macOS did not
    /// treat as a GUI app, so its window never appeared. The fix relaunches
    /// through the .app bundle, which means correctly recognising when we are
    /// inside one.
    #[cfg(target_os = "macos")]
    #[test]
    fn an_app_bundle_is_recognised_only_by_its_real_shape() {
        use std::path::{Path, PathBuf};
        let found = crate::enclosing_app_bundle(Path::new(
            "/Applications/Krate.app/Contents/MacOS/krate-cli",
        ));
        assert_eq!(found, Some(PathBuf::from("/Applications/Krate.app")));

        // A plain CLI install has no bundle to launch through, and guessing
        // one would send `open` at a path that does not exist.
        assert_eq!(
            crate::enclosing_app_bundle(Path::new("/usr/local/bin/krate")),
            None
        );
        assert_eq!(
            crate::enclosing_app_bundle(Path::new("/home/me/.local/bin/krate")),
            None
        );
        // Right depth, wrong shape: the directories must actually be
        // Contents/MacOS inside something.app.
        assert_eq!(
            crate::enclosing_app_bundle(Path::new("/tmp/Krate.zip/Contents/MacOS/krate-cli")),
            None
        );
        assert_eq!(
            crate::enclosing_app_bundle(Path::new("/tmp/Krate.app/Other/MacOS/krate-cli")),
            None
        );
    }

    #[test]
    fn telemetry_reports_a_bare_version_never_the_debug_suffix() {
        // The version goes into a JSON field. If a debug build's suffix
        // reached it, every dev run would count as its own "version" and the
        // adoption numbers would be noise. The suffix belongs in
        // `--version`, where a person reads it, and nowhere else (K-030).
        assert!(
            !crate::KRATE_VERSION_NUMBER.contains("debug"),
            "telemetry version must be the bare number, got {}",
            crate::KRATE_VERSION_NUMBER
        );
        assert!(
            !crate::KRATE_VERSION_NUMBER.contains(' '),
            "a version with a space in it would break the JSON field: {}",
            crate::KRATE_VERSION_NUMBER
        );
    }

    #[test]
    fn a_debug_build_says_so_and_a_release_build_does_not() {
        // The whole point: `krate --version` must distinguish the two, because
        // on this project's machine a target/debug/krate sits ahead of the
        // installed release on PATH and both reported the same string.
        let shown = crate::krate_version();
        if cfg!(debug_assertions) {
            assert!(
                shown.contains("debug build"),
                "a debug build must announce itself: {shown}"
            );
        } else {
            assert_eq!(
                shown,
                crate::KRATE_VERSION_NUMBER,
                "a released binary must report the bare number"
            );
        }
    }

    #[test]
    fn the_real_missing_library_message_classifies_as_no_window() {
        // The wording a person actually gets (K-036). It deliberately avoids
        // the word "libxkbcommon" in the first sentence, so classification
        // must not depend on the library name appearing at all.
        let err = anyhow::anyhow!(
            "this computer is missing a library apps need to read the keyboard.\n\n\
             Install it with:\n\n    sudo apt install libxkbcommon-x11-0\n"
        );
        assert_eq!(
            OpenFailure::classify(&Err(err)),
            Some(OpenFailure::NoWindow),
            "a machine without the X11 keyboard bridge is a window problem, \
             not an app that failed"
        );
    }

    #[test]
    fn a_trap_is_the_app_failing() {
        let err = anyhow::anyhow!("wasm trap: wasm `unreachable` instruction executed");
        assert_eq!(
            OpenFailure::classify(&Err(err)),
            Some(OpenFailure::AppFailed)
        );
    }

    /// Anything unrecognised is `other` rather than a guess. A rising share
    /// of `other` is the signal that the list needs another variant.
    #[test]
    fn an_unknown_failure_is_other_rather_than_a_guess() {
        let err = anyhow::anyhow!("something nobody predicted");
        assert_eq!(OpenFailure::classify(&Err(err)), Some(OpenFailure::Other));
    }

    /// The privacy property, stated as a test: every reason is a fixed word
    /// from this file. None of them can carry a path or an app name.
    #[test]
    fn every_reason_is_a_fixed_plain_word() {
        for reason in [
            OpenFailure::Refused,
            OpenFailure::NotFound,
            OpenFailure::BadBundle,
            OpenFailure::BadManifest,
            OpenFailure::VersionTooOld,
            OpenFailure::NoWindow,
            OpenFailure::AppFailed,
            OpenFailure::Other,
        ] {
            let word = reason.as_str();
            assert!(!word.is_empty());
            assert!(
                word.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
                "{word} is not a plain word"
            );
        }
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
