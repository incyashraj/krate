//! Krate's builder, set up on first need (IC-1005).
//!
//! A person who added Krate to Claude Desktop has no Rust. The first check or
//! build used to die on cargo's own error, which tells a model to tell the
//! person to install a compiler -- a terminal step the extension exists to
//! remove. Instead the first call that needs a compiler starts `krate setup
//! rust` in the background and answers at once with what is happening ("one
//! time, about 126 MB"), and every later call reports its progress until it is
//! done. Async-shaped for the same reason builds are: a download that takes a
//! minute must not sit inside one tool call that a client may time out.
//!
//! Everything is asked of the `krate` binary (`setup rust --status --json`,
//! `setup rust --json`), so this crate keeps no copy of where toolchains live.

use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::{json, Value};

#[derive(Debug, Clone)]
enum State {
    /// Not asked yet, or a failed setup that may be tried again.
    Unknown,
    /// This machine builds apps: nothing more to do in this session.
    Ready,
    Running {
        started: Instant,
        bytes: u64,
        last: Option<Value>,
    },
    Failed(String),
}

pub struct Builder {
    krate_bin: PathBuf,
    state: Arc<Mutex<State>>,
}

impl Builder {
    pub fn new(krate_bin: PathBuf) -> Self {
        Self {
            krate_bin,
            state: Arc::new(Mutex::new(State::Unknown)),
        }
    }

    /// `Ok` when a build may run now; otherwise the answer to give instead,
    /// which says what is being set up and when to call again.
    ///
    /// A binary that cannot say (an older krate, a test's fake) is taken as
    /// ready: the build then speaks for itself, as it did before this
    /// existed, rather than a guard inventing a reason to refuse.
    pub fn ensure(&self) -> Result<(), Value> {
        let current = self.state.lock().map(|s| s.clone()).unwrap_or(State::Ready);
        match current {
            State::Ready => Ok(()),
            State::Running {
                started,
                bytes,
                last,
            } => Err(setting_up(bytes, started, last.as_ref(), false)),
            State::Failed(reason) => {
                if let Ok(mut state) = self.state.lock() {
                    *state = State::Unknown;
                }
                Err(json!({
                    "ok": false,
                    "stage": "setup",
                    "status": "setup_failed",
                    "detail": format!("Setting up Krate's builder did not finish: {reason}"),
                    "next": "Tell the person in plain words (usually the internet connection \
                             dropped). Calling this tool again starts the setup again.",
                }))
            }
            State::Unknown => {
                let Some(status) = self.status() else {
                    self.set(State::Ready);
                    return Ok(());
                };
                if status.get("ready").and_then(Value::as_bool) != Some(false) {
                    self.set(State::Ready);
                    return Ok(());
                }
                if status.get("cargo_component").is_some_and(Value::is_null) {
                    // Not something a download fixes: every release ships it
                    // beside krate, so this Krate is incomplete.
                    return Err(json!({
                        "ok": false,
                        "stage": "setup",
                        "status": "incomplete_install",
                        "detail": "This Krate is missing cargo-component, which ships beside \
                                   krate in every release, so it cannot build apps.",
                        "next": "Tell the person to reinstall Krate (or the Krate extension). \
                                 Do not ask them to install developer tools.",
                    }));
                }
                if status.get("can_set_up").and_then(Value::as_bool) == Some(false) {
                    return Err(json!({
                        "ok": false,
                        "stage": "setup",
                        "status": "no_builder",
                        "detail": "Krate has no builder for this kind of computer yet.",
                        "next": "Tell the person plainly; the app can still be made on \
                                 krate.tech in a browser.",
                    }));
                }
                let bytes = status
                    .get("download_bytes")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                self.start(bytes);
                Err(setting_up(bytes, Instant::now(), None, true))
            }
        }
    }

    fn set(&self, state: State) {
        if let Ok(mut current) = self.state.lock() {
            *current = state;
        }
    }

    fn status(&self) -> Option<Value> {
        let output = Command::new(&self.krate_bin)
            .args(["setup", "rust", "--status", "--json"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        text.lines()
            .rev()
            .find_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
    }

    /// `krate setup rust --json` on a thread, its progress lines kept for the
    /// next call to report.
    fn start(&self, bytes: u64) {
        self.set(State::Running {
            started: Instant::now(),
            bytes,
            last: None,
        });
        let state = Arc::clone(&self.state);
        let krate_bin = self.krate_bin.clone();
        std::thread::spawn(move || {
            let child = Command::new(&krate_bin)
                .args(["setup", "rust", "--json"])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn();
            let mut child = match child {
                Ok(child) => child,
                Err(err) => {
                    if let Ok(mut s) = state.lock() {
                        *s = State::Failed(format!("could not start it: {err}"));
                    }
                    return;
                }
            };
            let stderr = child.stderr.take().map(|mut pipe| {
                std::thread::spawn(move || {
                    let mut text = String::new();
                    let _ = std::io::Read::read_to_string(&mut pipe, &mut text);
                    text
                })
            });
            if let Some(stdout) = child.stdout.take() {
                for line in std::io::BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    let Ok(step) = serde_json::from_str::<Value>(line.trim()) else {
                        continue;
                    };
                    if let Ok(mut s) = state.lock() {
                        if let State::Running { last, .. } = &mut *s {
                            *last = Some(step);
                        }
                    }
                }
            }
            let finished = child.wait();
            let complaint = stderr.and_then(|t| t.join().ok()).unwrap_or_default();
            if let Ok(mut s) = state.lock() {
                *s = match finished {
                    Ok(status) if status.success() => State::Ready,
                    Ok(_) => State::Failed(
                        complaint
                            .trim()
                            .trim_start_matches("Error: ")
                            .lines()
                            .next()
                            .unwrap_or("it stopped without saying why")
                            .to_string(),
                    ),
                    Err(err) => State::Failed(err.to_string()),
                };
            }
        });
    }
}

/// The answer while the builder is being set up: not a failure, and never
/// mistakable for a pass (`ok` is false).
fn setting_up(bytes: u64, started: Instant, last: Option<&Value>, first: bool) -> Value {
    let megabytes = bytes.div_ceil(1_000_000);
    let progress = match last {
        Some(step) if step["step"] == "download" => format!(
            "downloading {} ({} of {} MB)",
            step["part"].as_str().unwrap_or("Rust"),
            step["done"].as_u64().unwrap_or(0) / 1_000_000,
            step["total"].as_u64().unwrap_or(0) / 1_000_000
        ),
        Some(step) if step["step"] == "unpack" => {
            format!("unpacking {}", step["part"].as_str().unwrap_or("Rust"))
        }
        _ => "starting".to_string(),
    };
    json!({
        "ok": false,
        "stage": "setup",
        "status": "setting_up",
        "detail": format!(
            "Setting up Krate's builder, one time, about {megabytes} MB. It is the Rust \
             compiler apps are built with, kept in Krate's own folder."
        ),
        "progress": progress,
        "elapsed_seconds": started.elapsed().as_secs(),
        "next": if first {
            "Tell the person exactly that: Krate is setting up its builder, one time, and it \
             takes a minute or two. Then call this same tool again with the same arguments in \
             about 30 seconds, and keep calling until it answers with the check itself."
        } else {
            "Still setting up. Tell the person the progress, wait about 30 seconds, and call \
             this same tool again with the same arguments."
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_binary_that_cannot_answer_does_not_block_a_build() {
        let builder = Builder::new(PathBuf::from("/no/such/krate"));
        assert!(builder.ensure().is_ok());
        assert!(builder.ensure().is_ok(), "and stays out of the way");
    }

    #[test]
    fn the_first_answer_says_one_time_and_the_size() {
        let said = setting_up(126_451_384, Instant::now(), None, true);
        assert_eq!(said["ok"], false);
        assert_eq!(said["status"], "setting_up");
        let detail = said["detail"].as_str().unwrap();
        assert!(detail.contains("one time"), "{detail}");
        assert!(detail.contains("127 MB"), "{detail}");
        let progress = setting_up(
            1,
            Instant::now(),
            Some(
                &json!({"step": "download", "part": "rustc", "done": 40_000_000, "total": 67_652_240}),
            ),
            false,
        );
        assert_eq!(progress["progress"], "downloading rustc (40 of 67 MB)");
    }

    /// The whole road with a stand-in krate: not ready, set up in the
    /// background with progress, then ready -- and a failure says why and
    /// lets the next call start again.
    #[cfg(unix)]
    #[test]
    fn setup_runs_once_in_the_background_then_gets_out_of_the_way() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let done = dir.path().join("done");
        let bin = dir.path().join("krate");
        std::fs::write(
            &bin,
            format!(
                "#!/bin/sh\n\
                 if [ \"$3\" = --status ]; then\n\
                   if [ -f '{done}' ]; then echo '{{\"ready\":true}}'; else echo '{{\"ready\":false,\"cargo_component\":\"/x\",\"can_set_up\":true,\"download_bytes\":126451384}}'; fi\n\
                   exit 0\n\
                 fi\n\
                 echo '{{\"step\":\"download\",\"part\":\"rustc\",\"done\":0,\"total\":67652240}}'\n\
                 sleep 1\n\
                 touch '{done}'\n\
                 echo '{{\"step\":\"done\"}}'\n",
                done = done.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();

        let builder = Builder::new(bin);
        let first = builder.ensure().expect_err("not ready on a fresh machine");
        assert_eq!(first["status"], "setting_up");
        assert!(first["detail"].as_str().unwrap().contains("127 MB"));
        let second = builder.ensure().expect_err("still running");
        assert_eq!(second["status"], "setting_up");
        let deadline = Instant::now() + std::time::Duration::from_secs(20);
        loop {
            if builder.ensure().is_ok() {
                break;
            }
            assert!(Instant::now() < deadline, "setup never finished");
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(done.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_setup_says_why_and_the_next_call_tries_again() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("krate");
        std::fs::write(
            &bin,
            "#!/bin/sh\n\
             if [ \"$3\" = --status ]; then echo '{\"ready\":false,\"cargo_component\":\"/x\",\"can_set_up\":true,\"download_bytes\":5}'; exit 0; fi\n\
             echo 'Error: download https://static.rust-lang.org/x: connection reset' >&2\n\
             exit 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let builder = Builder::new(bin);
        assert!(builder.ensure().is_err());
        let deadline = Instant::now() + std::time::Duration::from_secs(20);
        let failed = loop {
            let answer = builder.ensure().expect_err("never ready");
            if answer["status"] == "setup_failed" {
                break answer;
            }
            assert!(Instant::now() < deadline, "the failure was never reported");
            std::thread::sleep(std::time::Duration::from_millis(100));
        };
        assert!(
            failed["detail"]
                .as_str()
                .unwrap()
                .contains("connection reset"),
            "{failed}"
        );
        let again = builder.ensure().expect_err("starts again");
        assert_eq!(again["status"], "setting_up");
    }

    #[cfg(unix)]
    #[test]
    fn a_krate_without_cargo_component_is_named_not_downloaded_around() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("krate");
        std::fs::write(
            &bin,
            "#!/bin/sh\necho '{\"ready\":false,\"cargo_component\":null,\"can_set_up\":true}'\n",
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let answer = Builder::new(bin).ensure().expect_err("cannot build");
        assert_eq!(answer["status"], "incomplete_install");
        assert!(answer["next"].as_str().unwrap().contains("reinstall"));
    }
}
