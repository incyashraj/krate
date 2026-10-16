use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

fn krate() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_krate"));
    // A hung child must name itself rather than eat the run.
    //
    // This suite makes 200-odd unbounded `.output()` calls. On Windows one
    // of them hangs: the test starts, prints no result, and the job burns to
    // GitHub's two-hour ceiling and reports "cancelled" with no cause. Three
    // separate investigations each named a different culprit, because which
    // test holds the bag is decided by position in the run, not by anything
    // about the test (K-240).
    //
    // A watchdog inside the child turns that into a failure with a name.
    // KRATE_TEST_WATCHDOG_SECS is read by the binary under test, which exits
    // rather than waiting for ever -- so `.output()` returns, the assertion
    // fails with the command that hung, and the other 200 tests still run.
    //
    // Generous on purpose: a real create compiles a component, which is
    // minutes on a cold cache. This is a ceiling, not a budget.
    if std::env::var_os("KRATE_TEST_WATCHDOG_SECS").is_none() {
        command.env("KRATE_TEST_WATCHDOG_SECS", "600");
    }
    // A test that needs a real window (the GPU presenter's teardown, K-881)
    // gets one that is transparent, behind everything and never focused:
    // the founder saw a test's "Rate card" window jump in front of his work
    // every few minutes while the gate ran.
    command.env("KRATE_QUIET_WINDOW", "1");
    // And no voice out loud from a speaking app (K-1077).
    command.env("KRATE_SILENT_SPEECH", "1");
    // Never the live hub. A signed app is checked with the hub when it opens
    // (IC-948 G), and a test that names no hub must not reach
    // hub.krate.tech with whatever sign-in this machine holds: a closed
    // local port answers "unreachable" at once. A test that needs a hub
    // sets its own, which replaces this.
    command.env("KRATE_HUB_URL", "http://127.0.0.1:9");
    command
}

/// True when `cargo-component` is on PATH.
///
/// The port pipeline compiles a real component, so the tests that run it need
/// the same build tools `krate create` asks for. Lanes that only run the
/// workspace tests do not install them, and there the honest outcome is to skip
/// rather than to fail on a missing tool or, worse, to weaken the assertions so
/// the test passes without building anything. The lanes that do install the
/// toolchain still run these tests in full.
/// Run a child and give up on it, instead of waiting for ever.
///
/// `.output()` and `.status()` have no timeout: a child that never returns
/// blocks its test, and with `--test-threads=1` that blocks the whole suite.
/// The Windows lane has burned to GitHub's ceiling on exactly this three
/// times (K-240), and each time the log showed the suite going silent rather
/// than failing, because a blocked wait is indistinguishable from slow work.
///
/// `arm_test_watchdog` in the binary covers a hung `krate`. It cannot cover
/// `cargo-component`, which this file spawns directly four times -- including
/// `has_cargo_component`, the probe every build test calls first. Those were
/// the unbounded waits left.
///
/// Kills the child and returns None on expiry, so the caller fails with its
/// own message naming the command rather than the suite stopping dead.
fn output_bounded(mut command: Command, secs: u64) -> Option<std::process::Output> {
    use std::io::Read;
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // Drain what it wrote. Both pipes, or a chatty child that
                // filled one is reported as empty.
                let mut out = Vec::new();
                let mut err = Vec::new();
                if let Some(mut s) = child.stdout.take() {
                    let _ = s.read_to_end(&mut out);
                }
                if let Some(mut s) = child.stderr.take() {
                    let _ = s.read_to_end(&mut err);
                }
                return Some(std::process::Output {
                    status,
                    stdout: out,
                    stderr: err,
                });
            }
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(_) => return None,
        }
    }
}

fn has_cargo_component() -> bool {
    // 60s: a version probe that takes longer than a minute is the hang, not
    // a slow machine.
    let mut command = Command::new("cargo-component");
    command.arg("--version");
    output_bounded(command, 60)
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Serializes the tests that invoke a real cargo build.
///
/// Cargo takes an exclusive lock on the package cache, so two of these running
/// at once leave one blocked on "waiting for file lock". The port pipeline
/// gives its build a bounded number of attempts, and on a cold CI cache that
/// wait outlasts them, which surfaces as a build failure that has nothing to do
/// with the code under test. Holding this lock keeps them one at a time.
static CARGO_BUILD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn cargo_build_guard() -> std::sync::MutexGuard<'static, ()> {
    // A previous test panicking must not poison the run for the rest.
    CARGO_BUILD_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[test]
fn a_tool_next_to_the_krate_binary_is_found() {
    // The installer places cargo-component beside krate, and that directory is
    // not always on PATH. Someone who ran the installer and then invoked krate
    // by its full path would otherwise be told to spend minutes compiling a
    // tool already sitting next to it.
    let dir = tempfile::tempdir().expect("temp dir");
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).expect("create bin");

    let krate_exe = bin.join(if cfg!(windows) { "krate.exe" } else { "krate" });
    std::fs::copy(env!("CARGO_BIN_EXE_krate"), &krate_exe).expect("copy krate");

    // A stand-in that answers --version the way the real tool does.
    let tool = bin.join(if cfg!(windows) {
        "cargo-component.exe"
    } else {
        "cargo-component"
    });
    if cfg!(windows) {
        // A .exe cannot be faked with a script, so only assert the Unix path.
        eprintln!("skipping the tool-beside-binary check on Windows: a .exe cannot be faked with a script");
        return;
    }
    std::fs::write(&tool, "#!/bin/sh\necho 'cargo-component 0.21.1'\n").expect("write tool");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    // An empty HOME so the cargo-home fallback cannot find it instead, and a
    // PATH without the install directory.
    let empty_home = dir.path().join("home");
    std::fs::create_dir_all(&empty_home).expect("create home");
    let output = Command::new(&krate_exe)
        .arg("doctor")
        .env_clear()
        .env("HOME", &empty_home)
        .env("PATH", "/usr/bin:/bin")
        .output()
        .expect("run krate doctor");

    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        text.contains("0.21.1"),
        "doctor should have found the sibling tool:\n{text}"
    );
}

#[test]
fn help_lists_phase_1_commands() {
    let output = krate().arg("--help").output().expect("run krate help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Commands:"));
    assert!(stdout.contains("run"));
    assert!(stdout.contains("version"));
    assert!(stdout.contains("doctor"));
    assert!(stdout.contains("manifest"));
    assert!(stdout.contains("port"));
}

#[test]
fn port_plan_is_read_only_and_reports_source_evidence() {
    let dir = tempfile::tempdir().expect("create temp dir");
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"existing-app\"\nversion = \"0.1.0\"\n",
    )
    .expect("write Cargo.toml");
    std::fs::create_dir(dir.path().join("src")).expect("create src");
    std::fs::write(
        dir.path().join("src/main.rs"),
        "fn main() { let _ = std::fs::read(\"notes.txt\"); }",
    )
    .expect("write source");

    let output = krate()
        .arg("port")
        .arg(dir.path())
        .arg("--plan")
        .output()
        .expect("run port plan");

    assert!(
        output.status.success(),
        "port plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Verdict: needs changes"));
    assert!(stdout.contains("Profile: krate-cli-v1-candidate"));
    assert!(stdout.contains("Local filesystem use"));
    assert!(stdout.contains("src/main.rs:1"));
    assert!(!dir.path().join("manifest.toml").exists());
}

#[test]
fn port_plan_can_write_machine_readable_json() {
    let dir = tempfile::tempdir().expect("create temp dir");
    std::fs::write(
        dir.path().join("go.mod"),
        "module example.com/existing\n\ngo 1.24\n",
    )
    .expect("write go.mod");
    let report_path = dir.path().join("port-plan.json");

    let output = krate()
        .arg("port")
        .arg(dir.path())
        .args(["--plan", "--format", "json", "--output"])
        .arg(&report_path)
        .output()
        .expect("run JSON port plan");

    assert!(
        output.status.success(),
        "JSON port plan failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(report_path).expect("read port plan"))
            .expect("parse port plan");
    assert_eq!(report["schema"], "krate.port.plan.v1");
    assert_eq!(report["profile"], "krate-cli-v1-candidate");
    // A small Go project is a rewrite the port does (ccb788451): work, said
    // as work, the same verdict the analyzer's own test gives Go. This test
    // is about the JSON being machine readable at all; the verdict just has
    // to be the honest one (K-1000).
    assert_eq!(report["verdict"], "needs-changes");
}

#[test]
fn the_prepared_workspace_tells_the_agent_what_it_needs_to_know() {
    // Every port failure today traced back to one of these files being wrong
    // or incomplete rather than missing: the contract listed no functions, so
    // an agent invented `stdio::write`; it never said how the verification
    // argument arrives, so a duplicate finder implemented `--quick`; it said
    // HTTPS did not work months after it did.
    //
    // The existing prepare test checks these files exist. Existing is not the
    // failure mode. This checks they say the things an agent cannot work out
    // for itself, which is the half of Gate 3 that can be covered without an
    // AI agent on the runner.
    let root = tempfile::tempdir().expect("tempdir");
    let source = root.path().join("src-project");
    std::fs::create_dir_all(source.join("src")).expect("mkdir");
    std::fs::write(
        source.join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("write Cargo.toml");
    std::fs::write(
        source.join("src/main.rs"),
        "fn main() { let d = std::fs::read(\"in.bin\").unwrap(); println!(\"{}\", d.len()); }\n",
    )
    .expect("write main.rs");

    let workspace = root.path().join("port-work");
    let output = krate()
        .arg("port")
        .arg(&source)
        .arg("--prepare")
        .arg(&workspace)
        .output()
        .expect("run port --prepare");
    assert!(output.status.success(), "prepare failed");

    let contract =
        std::fs::read_to_string(workspace.join("candidate/CONTRACT.md")).expect("read CONTRACT.md");

    // The API list. Without it an agent guesses names, which is exactly what
    // happened: `stdio::write` was invented three times in one port.
    let listed = contract.lines().filter(|l| l.starts_with("- `")).count();
    assert!(
        listed > 40,
        "the contract lists only {listed} functions; an agent would be guessing"
    );

    // How the verification argument arrives. A file-reading CLI gets a path,
    // everything else gets the bare word, and an app that handles only one of
    // them fails after building and packing correctly.
    assert!(
        contract.contains("`quick`"),
        "the contract must name the verification argument"
    );
    assert!(
        contract.contains("accept **both**"),
        "the contract must say a file-reading app is handed a path, not `quick`"
    );

    // The import rule, which is the one hard constraint on a Krate guest.
    assert!(
        contract.contains("krate:*"),
        "the contract must state the import rule"
    );

    // And the task file has to name the source it is porting, or the agent is
    // working from the candidate alone.
    let task =
        std::fs::read_to_string(workspace.join("AGENT_TASK.md")).expect("read AGENT_TASK.md");
    assert!(
        task.contains("probe") || task.contains("src-project"),
        "the agent task should name the project being ported"
    );
}

#[test]
fn port_prepare_creates_an_agent_workspace_without_changing_source() {
    let root = tempfile::tempdir().expect("create temp dir");
    let source = root.path().join("Existing Notes");
    std::fs::create_dir_all(source.join("src")).expect("create source");
    std::fs::write(
        source.join("Cargo.toml"),
        "[package]\nname = \"existing-notes\"\nversion = \"0.1.0\"\n",
    )
    .expect("write Cargo.toml");
    let original = "fn main() { let _ = std::fs::read(\"notes.txt\"); }\n";
    std::fs::write(source.join("src/main.rs"), original).expect("write source");
    let workspace = root.path().join("port-work");

    let output = krate()
        .arg("port")
        .arg(&source)
        .arg("--prepare")
        .arg(&workspace)
        .output()
        .expect("prepare port workspace");

    assert!(
        output.status.success(),
        "port prepare failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(source.join("src/main.rs")).expect("read original"),
        original
    );
    assert!(workspace.join("port-plan.json").is_file());
    assert!(workspace.join("PORTING.md").is_file());
    assert!(workspace.join("AGENT_TASK.md").is_file());
    assert!(workspace.join("journeys.json").is_file());
    assert!(workspace.join("JOURNEYS.md").is_file());
    assert!(workspace.join("snapshot-summary.json").is_file());
    assert!(workspace.join("reference-source/src/main.rs").is_file());
    assert!(workspace.join("candidate/Cargo.toml").is_file());
    assert!(workspace.join("candidate/src/lib.rs").is_file());
    assert!(workspace.join("candidate/manifest.toml").is_file());
    assert!(workspace.join("candidate/CONTRACT.md").is_file());

    let task =
        std::fs::read_to_string(workspace.join("AGENT_TASK.md")).expect("read generated task");
    assert!(task.contains("Edit files only inside `candidate/`"));
    assert!(task.contains("Do not change `reference-source/`"));
    assert!(task.contains("Local filesystem use"));
    assert!(task.contains("existing-notes"));
    assert!(task.contains("journeys.json"));

    let journeys: serde_json::Value = serde_json::from_slice(
        &std::fs::read(workspace.join("journeys.json")).expect("read journeys"),
    )
    .expect("parse journeys");
    assert_eq!(journeys["schema"], "krate.port.journeys.v1");
    assert!(journeys["journeys"]
        .as_array()
        .expect("journeys array")
        .iter()
        .any(|journey| journey["id"] == "primary-task"));

    let candidate_manifest = std::fs::read_to_string(workspace.join("candidate/manifest.toml"))
        .expect("read candidate manifest");
    assert!(candidate_manifest.contains("existing-notes"));
}

#[test]
fn port_prepare_refuses_to_overwrite_an_existing_workspace() {
    let root = tempfile::tempdir().expect("create temp dir");
    let source = root.path().join("source");
    let workspace = root.path().join("port-work");
    std::fs::create_dir_all(&source).expect("create source");
    std::fs::create_dir_all(&workspace).expect("create workspace");
    std::fs::write(workspace.join("keep.txt"), "keep me").expect("write sentinel");
    std::fs::write(source.join("go.mod"), "module example.com/source\n").expect("write go.mod");

    let output = krate()
        .arg("port")
        .arg(&source)
        .arg("--prepare")
        .arg(&workspace)
        .output()
        .expect("run port prepare");

    assert!(!output.status.success());
    assert_eq!(
        std::fs::read_to_string(workspace.join("keep.txt")).expect("read sentinel"),
        "keep me"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("already exists"));
}

#[test]
fn port_author_command_builds_packages_and_permission_tests_a_candidate() {
    if !has_cargo_component() {
        eprintln!("skipping: cargo-component is not installed");
        return;
    }
    let _build_lock = cargo_build_guard();
    let root = tempfile::tempdir().expect("create temp dir");
    let source = root.path().join("tiny-reader");
    std::fs::create_dir_all(source.join("src")).expect("create source");
    std::fs::write(
        source.join("Cargo.toml"),
        "[package]\nname = \"tiny-reader\"\nversion = \"0.1.0\"\n",
    )
    .expect("write Cargo.toml");
    let original = "fn main() { println!(\"reader\"); }\n";
    std::fs::write(source.join("src/main.rs"), original).expect("write source");
    let workspace = root.path().join("port-work");
    let bundle = root.path().join("tiny-reader.krate");
    let transcript = root.path().join("port-result.json");

    let output = krate()
        .arg("port")
        .arg(&source)
        .arg("--prepare")
        .arg(&workspace)
        .arg("--author-cmd")
        .arg(
            "grep -v 'starting point' \"$KRATE_PORT_CANDIDATE/src/lib.rs\" \
             > \"$KRATE_PORT_CANDIDATE/src/lib.rs.ported\" \
             && mv \"$KRATE_PORT_CANDIDATE/src/lib.rs.ported\" \"$KRATE_PORT_CANDIDATE/src/lib.rs\" \
             && printf 'Preserved the file-reading journey.\\n' > PORT_RESULT.md",
        )
        .arg("--to")
        .arg(&bundle)
        .arg("--transcript")
        .arg(&transcript)
        .arg("--no-install")
        .env("CARGO_NET_OFFLINE", "true")
        .output()
        .expect("run port pipeline");

    // Exit 7, not 0. Everything mechanical passed -- it built, it imports only
    // krate:*, it runs, it refuses without its capability -- and nobody has
    // compared it against the original, which this test does not do either.
    // The pipeline says so instead of calling four green checks a port.
    assert_eq!(
        output.status.code(),
        Some(7),
        "a port whose primary task was never compared is not finished\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("not a finished port"),
        "the person must be told what is missing: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(bundle.is_file(), "the bundle is still written -- it built");
    assert_eq!(
        std::fs::read_to_string(source.join("src/main.rs")).expect("read original"),
        original
    );
    let result: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&transcript).expect("read transcript"))
            .expect("parse transcript");
    assert_eq!(result["schema"], "krate.port.result.v1");
    assert_eq!(result["source_unchanged"], true);
    assert_eq!(result["author"], "external-command");
    assert_eq!(result["repair_attempts_used"], 0);
    assert_eq!(
        result["bundle_sha256"]
            .as_str()
            .expect("bundle sha256")
            .len(),
        64
    );
    assert!(workspace.join("artifact.json").is_file());
    assert!(workspace.join("journey-results.json").is_file());
    let journey_results: serde_json::Value = serde_json::from_slice(
        &std::fs::read(workspace.join("journey-results.json")).expect("read journey results"),
    )
    .expect("parse journey results");
    assert!(journey_results["results"]
        .as_array()
        .expect("results array")
        .iter()
        .any(|result| result["id"] == "launch" && result["status"] == "passed"));
    assert!(result["agent_result"]
        .as_str()
        .expect("agent result text")
        .contains("file-reading journey"));
    assert!(result["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .any(|check| check == "component imports only krate:* interfaces"));
}

#[test]
fn port_repairs_a_failed_candidate_with_the_exact_build_error() {
    if !has_cargo_component() {
        eprintln!("skipping: cargo-component is not installed");
        return;
    }
    let _build_lock = cargo_build_guard();
    let root = tempfile::tempdir().expect("create temp dir");
    let source = root.path().join("repair-reader");
    std::fs::create_dir_all(source.join("src")).expect("create source");
    std::fs::write(
        source.join("Cargo.toml"),
        "[package]\nname = \"repair-reader\"\nversion = \"0.1.0\"\n",
    )
    .expect("write Cargo.toml");
    std::fs::write(source.join("src/main.rs"), "fn main() {}\n").expect("write source");
    let workspace = root.path().join("port-work");
    let bundle = root.path().join("repair-reader.krate");
    let transcript = root.path().join("port-result.json");
    let repair_command = "if [ -n \"$KRATE_PORT_REPAIR_LOG\" ]; then \
        grep -q 'cargo-component build failed' \"$KRATE_PORT_REPAIR_LOG\" && \
        cp \"$KRATE_PORT_CANDIDATE/src/lib.rs.good\" \"$KRATE_PORT_CANDIDATE/src/lib.rs\"; \
        else grep -v 'starting point' \"$KRATE_PORT_CANDIDATE/src/lib.rs\" \
        > \"$KRATE_PORT_CANDIDATE/src/lib.rs.good\" && \
        cp \"$KRATE_PORT_CANDIDATE/src/lib.rs.good\" \"$KRATE_PORT_CANDIDATE/src/lib.rs\" && \
        printf '\\nthis is not valid rust\\n' >> \"$KRATE_PORT_CANDIDATE/src/lib.rs\"; fi";

    let output = krate()
        .arg("port")
        .arg(&source)
        .arg("--prepare")
        .arg(&workspace)
        .arg("--author-cmd")
        .arg(repair_command)
        .arg("--repair-attempts")
        .arg("2")
        .arg("--to")
        .arg(&bundle)
        .arg("--transcript")
        .arg(&transcript)
        .arg("--no-install")
        .env("CARGO_NET_OFFLINE", "true")
        .output()
        .expect("run repairing port pipeline");

    // Exit 7: the repair worked and the candidate builds, which is what this
    // test is about. The port is still not established, because nothing here
    // compared it against the original -- so the pipeline says so, exactly as
    // it does for a candidate that needed no repair.
    assert_eq!(
        output.status.code(),
        Some(7),
        "the repair should succeed and the port should still be unverified\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(bundle.is_file());
    assert!(workspace.join("repair/attempt-1.txt").is_file());
    let repair_log =
        std::fs::read_to_string(workspace.join("repair/attempt-1.txt")).expect("read repair log");
    assert!(repair_log.contains("cargo-component build failed"));
    let result: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&transcript).expect("read transcript"))
            .expect("parse transcript");
    assert_eq!(result["repair_attempts_allowed"], 2);
    assert_eq!(result["repair_attempts_used"], 1);
}

#[test]
fn port_to_requires_an_agent() {
    let root = tempfile::tempdir().expect("create temp dir");
    let source = root.path().join("source");
    std::fs::create_dir_all(&source).expect("create source");
    std::fs::write(source.join("go.mod"), "module example.com/source\n").expect("write go.mod");

    let output = krate()
        .arg("port")
        .arg(&source)
        .arg("--to")
        .arg(root.path().join("app.krate"))
        .output()
        .expect("run invalid port");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("--to requires --agent <agent> or --author-cmd <command>"));
}

#[test]
fn port_agent_cannot_write_through_the_read_only_source_snapshot() {
    if !has_cargo_component() {
        eprintln!("skipping: cargo-component is not installed");
        return;
    }
    let _build_lock = cargo_build_guard();
    let root = tempfile::tempdir().expect("create temp dir");
    let source = root.path().join("source");
    std::fs::create_dir_all(source.join("src")).expect("create source");
    std::fs::write(
        source.join("Cargo.toml"),
        "[package]\nname='source'\nversion='0.1.0'\n",
    )
    .expect("write Cargo.toml");
    std::fs::write(source.join("src/main.rs"), "fn main() {}\n").expect("write source");
    let bundle = root.path().join("must-not-exist.krate");

    let output = krate()
        .arg("port")
        .arg(&source)
        .arg("--author-cmd")
        .arg("printf 'fn changed() {}\\n' > \"$KRATE_PORT_SOURCE/src/main.rs\"")
        .arg("--to")
        .arg(&bundle)
        .arg("--no-install")
        .output()
        .expect("run source integrity test");

    assert!(!output.status.success());
    assert!(!bundle.exists());
    assert_eq!(
        std::fs::read_to_string(source.join("src/main.rs")).expect("read original"),
        "fn main() {}\n"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("port author command failed"),
        "unexpected stderr:\n{stderr}\nstdout:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn version_prints_runtime_metadata() {
    let output = krate().arg("version").output().expect("run krate version");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("krate"));
    assert!(stdout.contains("wasmtime  43.0.2"));
    assert!(stdout.contains("rustc"));
    assert!(stdout.contains("commit"));
}

#[test]
fn doctor_lists_phase_1_tooling() {
    let output = krate().arg("doctor").output().expect("run krate doctor");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Krate doctor"));
    assert!(stdout.contains("Core tools"));
    assert!(stdout.contains("cargo-component"));
    assert!(stdout.contains("wasm32-wasip1"));
    assert!(stdout.contains("wasm32-wasip2"));
    assert!(stdout.contains("Phase 2 language tools"));
    assert!(stdout.contains("wasm-tools"));
    assert!(stdout.contains("tinygo"));
    assert!(stdout.contains("go"));
    assert!(stdout.contains("node"));
    assert!(stdout.contains("npm"));
    assert!(stdout.contains("jco"));
    assert!(stdout.contains("state dir"));
}

#[test]
fn manifest_check_validates_phase_2_manifest() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let manifest_path = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest_path,
        r#"
            [app]
            id = "com.example.hello"
            name = "Hello"
            version = "1.0.0"
            entry = "hello.wasm"
            world = "krate:app/cli@0.1.0"

            [[capabilities]]
            cap = "io.stdout"
            rationale = "Print output"
            required = true
        "#,
    )
    .expect("write manifest");

    let output = krate()
        .args(["manifest", "check"])
        .arg(&manifest_path)
        .output()
        .expect("run manifest check");

    assert!(
        output.status.success(),
        "manifest check failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Manifest OK"));
    assert!(stdout.contains("app id          com.example.hello"));
    assert!(stdout.contains("capabilities    1"));
}

#[test]
fn manifest_check_json_reports_summary() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let manifest_path = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest_path,
        r#"
            [app]
            id = "com.example.hello"
            name = "Hello"
            version = "1.0.0"
            entry = "hello.wasm"
            world = "krate:app/cli@0.1.0"

            [[capabilities]]
            cap = "io.stdout"
            rationale = "Print output"
            required = true
        "#,
    )
    .expect("write manifest");

    let output = krate()
        .args(["manifest", "check", "--format", "json"])
        .arg(&manifest_path)
        .output()
        .expect("run manifest check json");

    assert!(
        output.status.success(),
        "manifest check json failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(r#""ok": true"#));
    assert!(stdout.contains(r#""id": "com.example.hello""#));
    assert!(stdout.contains(r#""capabilities": 1"#));
    assert!(stdout.contains(r#""required_capabilities": 1"#));
    assert!(stdout.contains(r#""world_kind": "Phase 2 CLI""#));
}

#[test]
fn manifest_check_accepts_phase_3_gui_draft_world() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let manifest_path = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest_path,
        r#"
            [app]
            id = "com.example.notes"
            name = "Notes"
            version = "1.0.0"
            entry = "notes.wasm"
            world = "krate:app/gui@0.2.0"

            [[capabilities]]
            cap = "io.stdout"
            rationale = "Debug output while the GUI runtime is in draft"
            required = true
        "#,
    )
    .expect("write manifest");

    let output = krate()
        .args(["manifest", "check"])
        .arg(&manifest_path)
        .output()
        .expect("run Phase 3 manifest check");

    assert!(
        output.status.success(),
        "manifest check failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Manifest OK"));
    assert!(stdout.contains("world           krate:app/gui@0.2.0"));
    assert!(stdout.contains("app type       Graphical app"));
}

#[test]
fn manifest_check_rejects_bad_capability() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let manifest_path = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest_path,
        r#"
            [app]
            id = "com.example.hello"
            name = "Hello"
            version = "1.0.0"
            entry = "hello.wasm"
            world = "krate:app/cli@0.1.0"

            [[capabilities]]
            cap = "FS.read:./data/**"
            rationale = "Read data"
            required = true
        "#,
    )
    .expect("write manifest");

    let output = krate()
        .args(["manifest", "check"])
        .arg(&manifest_path)
        .output()
        .expect("run manifest check");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid capability"));
}

#[test]
fn manifest_explain_shows_default_and_launch_grants() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let manifest_path = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest_path,
        r#"
            [app]
            id = "com.example.notes"
            name = "Notes"
            version = "1.0.0"
            entry = "notes.wasm"
            world = "krate:app/cli@0.1.0"

            [[capabilities]]
            cap = "io.stdout"
            rationale = "Print output"
            required = true

            [[capabilities]]
            cap = "fs.read:./notes/**"
            rationale = "Read notes"
            required = true
        "#,
    )
    .expect("write manifest");

    let output = krate()
        .args(["manifest", "explain"])
        .arg(&manifest_path)
        .output()
        .expect("run manifest explain");

    assert!(
        output.status.success(),
        "manifest explain failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Manifest"));
    assert!(stdout.contains("app id          com.example.notes"));
    assert!(stdout.contains("Capabilities"));
    assert!(stdout.contains("- io.stdout"));
    assert!(stdout.contains("default grant        yes"));
    assert!(stdout.contains("launch grant needed  no"));
    assert!(stdout.contains("- fs.read:notes/**"));
    assert!(stdout.contains("default grant        no"));
    assert!(stdout.contains("launch grant needed  yes"));
    assert!(stdout.contains("resource             notes/**"));
    assert!(stdout.contains("rationale            Read notes"));
}

#[test]
fn manifest_explain_json_reports_structured_grants() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let manifest_path = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest_path,
        r#"
            [app]
            id = "com.example.notes"
            name = "Notes"
            version = "1.0.0"
            entry = "notes.wasm"
            world = "krate:app/cli@0.1.0"

            [[capabilities]]
            cap = "io.stdout"
            rationale = "Print output"
            required = true

            [[capabilities]]
            cap = "fs.read:./notes/**"
            rationale = "Read notes"
            required = true
        "#,
    )
    .expect("write manifest");

    let output = krate()
        .args(["manifest", "explain", "--format", "json"])
        .arg(&manifest_path)
        .output()
        .expect("run manifest explain json");

    assert!(
        output.status.success(),
        "manifest explain json failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(r#""id": "com.example.notes""#));
    assert!(stdout.contains(r#""entry": "notes.wasm""#));
    assert!(stdout.contains(r#""world_kind": "Phase 2 CLI""#));
    assert!(stdout.contains(r#""capability": "io.stdout""#));
    assert!(stdout.contains(r#""default_grant": true"#));
    assert!(stdout.contains(r#""launch_grant_needed": false"#));
    assert!(stdout.contains(r#""capability": "fs.read:notes/**""#));
    assert!(stdout.contains(r#""module": "fs""#));
    assert!(stdout.contains(r#""action": "read""#));
    assert!(stdout.contains(r#""resource": "notes/**""#));
    assert!(stdout.contains(r#""launch_grant_needed": true"#));
}

#[test]
fn manifest_capabilities_lists_phase_2_cap_table() {
    let output = krate()
        .args(["manifest", "capabilities"])
        .output()
        .expect("run manifest capabilities");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Krate capabilities"));
    assert!(stdout.contains("io.args"));
    assert!(stdout.contains("fs.read:<path-glob>"));
    assert!(stdout.contains("net.connect:<host>:<port>"));
    assert!(stdout.contains("locale.format"));
    assert!(stdout.contains("ui.window:create"));
    assert!(stdout.contains("ui.clipboard:read"));
    assert!(stdout.contains("gfx.gpu:basic"));
    assert!(stdout.contains("audio.capture"));
}

#[test]
fn run_accepts_phase_3_gui_manifest_and_reaches_the_runtime() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let wasm_path = dir.path().join("notes.wasm");
    let manifest_path = dir.path().join("manifest.toml");
    std::fs::write(&wasm_path, b"not actually wasm").expect("write wasm placeholder");
    std::fs::write(
        &manifest_path,
        r#"
            [app]
            id = "com.example.notes"
            name = "Notes"
            version = "1.0.0"
            entry = "notes.wasm"
            world = "krate:app/gui@0.2.0"
        "#,
    )
    .expect("write manifest");

    let output = krate()
        .args(["run", "--manifest"])
        .arg(&manifest_path)
        .arg(&wasm_path)
        .output()
        .expect("run Phase 3 GUI draft manifest");

    // The gui world is now runnable: the manifest gate lets the run proceed,
    // so the placeholder bytes reach the runtime and fail as an invalid
    // component instead of being rejected at the world gate.
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("unsupported app world"));
    assert!(stderr.contains("invalid wasm component"));
}

#[test]
fn run_json_reports_denied_capabilities_before_running() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let wasm_path = dir.path().join("app.wasm");
    let manifest_path = dir.path().join("manifest.toml");
    std::fs::write(&wasm_path, b"not actually wasm").expect("write wasm placeholder");
    std::fs::write(
        &manifest_path,
        r#"
            [app]
            id = "com.example.jsonapp"
            name = "JsonApp"
            version = "1.0.0"
            entry = "app.wasm"
            world = "krate:app/cli@0.1.0"

            [[capabilities]]
            cap = "fs.read:data/**"
            rationale = "read data"
            required = true
        "#,
    )
    .expect("write manifest");

    let output = krate()
        .args(["run", "--json", "--manifest"])
        .arg(&manifest_path)
        .arg(&wasm_path)
        .env_remove("KRATE_TEST_PROMPT")
        .output()
        .expect("run json denied");

    assert_eq!(output.status.code(), Some(5));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let payload: serde_json::Value = serde_json::from_str(stdout.trim()).expect("parse run json");
    assert_eq!(payload["schema"], "krate.run.v1");
    assert_eq!(payload["exit"]["class"], "permission-denied");
    assert_eq!(payload["exit"]["code"], 5);
    assert_eq!(payload["capabilities"]["denied"][0], "fs.read:data/**");
    assert_eq!(payload["app"]["id"], "com.example.jsonapp");
}

#[test]
fn run_json_reports_invalid_components_as_machine_readable_failure() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let wasm_path = dir.path().join("app.wasm");
    std::fs::write(&wasm_path, b"not actually wasm").expect("write wasm placeholder");

    let output = krate()
        .args(["run", "--json"])
        .arg(&wasm_path)
        .output()
        .expect("run json invalid");

    assert_eq!(output.status.code(), Some(2));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let payload: serde_json::Value = serde_json::from_str(stdout.trim()).expect("parse run json");
    assert_eq!(payload["schema"], "krate.run.v1");
    assert_eq!(payload["exit"]["class"], "invalid-component");
    assert!(payload["exit"]["message"].as_str().is_some());
    assert_eq!(payload["stdout"], "");
}

#[test]
fn manifest_capabilities_json_lists_phase_2_cap_table() {
    let output = krate()
        .args(["manifest", "capabilities", "--format", "json"])
        .output()
        .expect("run manifest capabilities json");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(r#""capability": "io.args""#));
    assert!(stdout.contains(r#""module": "fs""#));
    assert!(stdout.contains(r#""action": "read""#));
    assert!(stdout.contains(r#""resource": "<path-glob>""#));
    assert!(stdout.contains(r#""capability": "net.connect:<host>:<port>""#));
    assert!(stdout.contains(r#""capability": "ui.window:create""#));
    assert!(stdout.contains(r#""capability": "gfx.gpu:basic""#));
    assert!(stdout.contains(r#""capability": "audio.capture""#));
    assert!(stdout.contains(r#""default_grant": true"#));
}

#[test]
fn manifest_init_prints_valid_phase_2_manifest() {
    let output = krate()
        .args([
            "manifest",
            "init",
            "--id",
            "com.example.notes",
            "--name",
            "Notes",
            "--entry",
            "notes.wasm",
            "--cap",
            "io.stdout",
            "--cap",
            "fs.read:./notes/**",
        ])
        .output()
        .expect("run manifest init");

    assert!(
        output.status.success(),
        "manifest init failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[app]"));
    assert!(stdout.contains("id = \"com.example.notes\""));
    assert!(stdout.contains("entry = \"notes.wasm\""));
    assert!(stdout.contains("cap = \"io.stdout\""));
    assert!(stdout.contains("cap = \"fs.read:notes/**\""));
    assert!(output.stderr.is_empty());

    let dir = tempfile::tempdir().expect("create temp dir");
    let manifest_path = dir.path().join("manifest.toml");
    std::fs::write(&manifest_path, stdout.as_bytes()).expect("write generated manifest");

    let check = krate()
        .args(["manifest", "check"])
        .arg(&manifest_path)
        .output()
        .expect("check generated manifest");
    assert!(
        check.status.success(),
        "generated manifest check failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    );
}

#[test]
fn manifest_init_writes_output_and_refuses_overwrite() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let manifest_path = dir.path().join("manifest.toml");

    let output = krate()
        .args([
            "manifest",
            "init",
            "--id",
            "com.example.clock",
            "--name",
            "Clock",
            "--entry",
            "clock.wasm",
            "--cap",
            "time.clock",
            "--output",
        ])
        .arg(&manifest_path)
        .output()
        .expect("write manifest");

    assert!(
        output.status.success(),
        "manifest init output failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(manifest_path.exists());

    let second = krate()
        .args([
            "manifest",
            "init",
            "--id",
            "com.example.clock",
            "--name",
            "Clock",
            "--entry",
            "clock.wasm",
            "--output",
        ])
        .arg(&manifest_path)
        .output()
        .expect("refuse overwrite");

    assert!(!second.status.success());
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(stderr.contains("refusing to overwrite existing manifest"));
}

#[test]
fn sample_manifests_validate() {
    for manifest in [
        "apps/krate-clock/manifest.toml",
        "apps/krate-cat/manifest.toml",
        "apps/krate-curl/manifest.toml",
    ] {
        let manifest = workspace_path(PathBuf::from(manifest));
        let output = krate()
            .args(["manifest", "check"])
            .arg(&manifest)
            .output()
            .expect("check sample manifest");

        assert!(
            output.status.success(),
            "manifest check failed for {}\nstdout:\n{}\nstderr:\n{}",
            manifest.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn missing_input_returns_clear_error() {
    let output = krate()
        .args(["run", "/definitely/not/a/component.wasm"])
        .output()
        .expect("run krate with missing input");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("input file does not exist"));
}

#[test]
fn run_rejects_empty_app_argument_before_runtime() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let wasm_path = dir.path().join("app.wasm");
    std::fs::write(&wasm_path, b"not actually wasm").expect("write wasm placeholder");

    let output = krate()
        .arg("run")
        .arg(&wasm_path)
        .arg("--")
        .arg("")
        .output()
        .expect("run krate with empty app arg");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("app arguments cannot contain empty values"));
    assert!(
        !stderr.contains("invalid wasm component"),
        "runtime should not run when app args are invalid"
    );
}

#[test]
fn run_rejects_newline_app_argument_before_runtime() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let wasm_path = dir.path().join("app.wasm");
    std::fs::write(&wasm_path, b"not actually wasm").expect("write wasm placeholder");

    let output = krate()
        .arg("run")
        .arg(&wasm_path)
        .arg("--")
        .arg("bad\narg")
        .output()
        .expect("run krate with newline app arg");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cannot contain newline or NUL characters"));
    assert!(
        !stderr.contains("invalid wasm component"),
        "runtime should not run when app args are invalid"
    );
}

#[test]
fn run_rejects_oversized_raw_args_payload_before_runtime() {
    if cfg!(windows) {
        eprintln!(
            "skipping oversized raw-args spawn on Windows: the OS command-line limit is lower than Krate's 64 KiB raw-args guard"
        );
        return;
    }

    let dir = tempfile::tempdir().expect("create temp dir");
    let wasm_path = dir.path().join("app.wasm");
    std::fs::write(&wasm_path, b"not actually wasm").expect("write wasm placeholder");
    let oversized = "x".repeat((64 * 1024) + 1);

    let output = krate()
        .arg("run")
        .arg(&wasm_path)
        .arg("--")
        .arg(oversized)
        .output()
        .expect("run krate with oversized app arg payload");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("app arguments exceed raw args limit"));
    assert!(
        !stderr.contains("invalid wasm component"),
        "runtime should not run when app args are invalid"
    );
}

#[test]
fn run_rejects_too_many_app_arguments_before_runtime() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let wasm_path = dir.path().join("app.wasm");
    std::fs::write(&wasm_path, b"not actually wasm").expect("write wasm placeholder");

    let mut cmd = krate();
    cmd.arg("run").arg(&wasm_path).arg("--");
    for _ in 0..1025 {
        cmd.arg("x");
    }
    let output = cmd.output().expect("run krate with too many app arguments");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("app arguments exceed count limit"));
    assert!(
        !stderr.contains("invalid wasm component"),
        "runtime should not run when app args are invalid"
    );
}

#[test]
fn configured_hello_component_runs_and_matches_expected_fixture_hash() {
    let Some(path) = configured_hello_component() else {
        return;
    };

    let wasm = std::fs::read(&path).expect("read configured hello component");
    let actual_hash = sha256_hex(&wasm);
    eprintln!("hello component sha256: {actual_hash}");

    if let Some(expected_hash) = expected_hello_hash() {
        assert_eq!(
            actual_hash, expected_hash,
            "configured hello component hash does not match the expected shared fixture"
        );
    }

    let output = krate()
        .args(["run"])
        .arg(path)
        .output()
        .expect("run krate hello component");

    assert!(
        output.status.success(),
        "krate run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.lines().collect::<Vec<_>>(), ["Hello, Krate!"]);
}

#[test]
fn configured_phase2_smoke_component_runs_through_uapi() {
    let Some(path) = configured_phase2_smoke_component() else {
        return;
    };

    let dir = tempfile::tempdir().expect("create temp dir");
    std::fs::write(
        dir.path().join("phase2-smoke-input.txt"),
        "Krate Phase 2 input\n",
    )
    .expect("write Phase 2 smoke input");

    let output = krate()
        .current_dir(dir.path())
        .args(["run", "--grant", "fs.read:phase2-smoke-input.txt"])
        .arg(path)
        .output()
        .expect("run krate Phase 2 smoke component");

    assert!(
        output.status.success(),
        "krate run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("phase2-smoke ok"));
    assert!(stdout.contains("file=Krate Phase 2 input"));
    assert!(stdout.contains("locale="));
    assert!(stdout.contains("timezone="));
    assert!(stdout.contains("number=12.5"));
    assert!(stdout.contains("time-ok=true"));
    assert!(stdout.contains("mono-ok=true"));
}

#[test]
fn configured_phase2_smoke_component_denies_missing_file_grant() {
    let Some(path) = configured_phase2_smoke_component() else {
        return;
    };

    let dir = tempfile::tempdir().expect("create temp dir");
    std::fs::write(
        dir.path().join("phase2-smoke-input.txt"),
        "Krate Phase 2 input\n",
    )
    .expect("write Phase 2 smoke input");

    let output = krate()
        .current_dir(dir.path())
        .args(["run"])
        .arg(path)
        .output()
        .expect("run krate Phase 2 smoke component without grant");

    assert_eq!(output.status.code(), Some(25));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("phase2-smoke permission denied: fs.read"));
}

#[test]
fn configured_krate_clock_component_uses_fixed_test_time() {
    let Some(path) = configured_krate_clock_component() else {
        return;
    };

    let output = krate()
        .args(["run", "--test-time", "1234567890"])
        .arg(path)
        .output()
        .expect("run krate-clock component");

    assert!(
        output.status.success(),
        "krate-clock failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("app=krate-clock"));
    assert!(stdout.contains("timezone="));
    assert!(stdout.contains("locale="));
    assert!(stdout.contains("date=1970-01-15 06:56"));
}

#[test]
fn configured_krate_clock_component_matches_deterministic_fixture_snapshot() {
    let Some(path) = configured_krate_clock_component() else {
        return;
    };

    let output = krate()
        .args([
            "run",
            "--test-time",
            "1234567890",
            "--test-locale",
            "en-US",
            "--test-timezone",
            "UTC",
        ])
        .arg(path)
        .output()
        .expect("run krate-clock component with deterministic locale/timezone");

    assert!(
        output.status.success(),
        "krate-clock deterministic snapshot failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        concat!(
            "app=krate-clock\n",
            "timezone=UTC\n",
            "locale=en-US\n",
            "date=1970-01-15 06:56\n"
        )
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn configured_krate_clock_component_applies_positive_timezone_offset() {
    let Some(path) = configured_krate_clock_component() else {
        return;
    };

    let output = krate()
        .args([
            "run",
            "--test-time",
            "1234567890",
            "--test-locale",
            "en-US",
            "--test-timezone",
            "UTC+05:30",
        ])
        .arg(path)
        .output()
        .expect("run krate-clock with positive timezone offset");

    assert!(
        output.status.success(),
        "krate-clock timezone offset run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("timezone=UTC+05:30"));
    assert!(stdout.contains("date=1970-01-15 12:26"));
    assert!(output.stderr.is_empty());
}

#[test]
fn configured_krate_clock_component_applies_negative_timezone_offset() {
    let Some(path) = configured_krate_clock_component() else {
        return;
    };

    let output = krate()
        .args([
            "run",
            "--test-time",
            "0",
            "--test-locale",
            "en-US",
            "--test-timezone",
            "UTC-01:00",
        ])
        .arg(path)
        .output()
        .expect("run krate-clock with negative timezone offset");

    assert!(
        output.status.success(),
        "krate-clock negative timezone offset run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("timezone=UTC-01:00"));
    assert!(stdout.contains("date=1969-12-31 23:00"));
    assert!(output.stderr.is_empty());
}

#[test]
fn configured_krate_clock_component_runs_with_sample_manifest_auto_grant() {
    let Some(path) = configured_krate_clock_component() else {
        return;
    };

    let output = krate()
        .args([
            "run",
            "--auto-grant",
            "--manifest",
            sample_manifest("krate-clock")
                .to_str()
                .expect("manifest path"),
            "--test-time",
            "1234567890",
        ])
        .arg(path)
        .output()
        .expect("run krate-clock with sample manifest");

    assert!(
        output.status.success(),
        "krate-clock manifest run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("app=krate-clock"));
    assert!(stdout.contains("date=1970-01-15 06:56"));
}

// --- fuel budget / untrusted runs (S5) --------------------------------------
//
// A finite fuel budget must stop a run instead of letting it complete or hang.
// `--untrusted` applies a generous default budget that real apps finish under,
// and `krate create` uses that same untrusted run to verify what it authored,
// so a generated infinite loop fails verification rather than hanging.

/// Build the standard clock run args, granting the clock capability via the
/// sample manifest and pinning the time so the run is deterministic.
fn clock_run_args(path: &std::path::Path) -> Vec<String> {
    vec![
        "run".to_string(),
        "--auto-grant".to_string(),
        "--manifest".to_string(),
        sample_manifest("krate-clock")
            .to_str()
            .expect("manifest path")
            .to_string(),
        "--test-time".to_string(),
        "1234567890".to_string(),
        path.to_string_lossy().into_owned(),
    ]
}

#[test]
fn untrusted_default_budget_lets_a_real_app_finish() {
    let Some(path) = configured_krate_clock_component() else {
        return;
    };

    let mut args = clock_run_args(&path);
    // Insert the flag right after "run" so it applies to the run subcommand.
    args.insert(1, "--untrusted".to_string());

    let output = krate()
        .args(&args)
        .output()
        .expect("run krate-clock as untrusted");

    assert!(
        output.status.success(),
        "the default untrusted fuel budget must not break a real app\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("app=krate-clock"));
}

#[test]
fn a_tiny_fuel_budget_stops_the_run_with_limit_exceeded() {
    let Some(path) = configured_krate_clock_component() else {
        return;
    };

    let mut args = clock_run_args(&path);
    args.insert(1, "--fuel".to_string());
    args.insert(2, "1".to_string());

    let output = krate()
        .args(&args)
        .output()
        .expect("run krate-clock with a tiny fuel budget");

    // Exit 4 is Krate's limit-exceeded class: fuel ran out before completion.
    assert_eq!(
        output.status.code(),
        Some(4),
        "a fuel budget of 1 must stop the run (exit 4)\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn explicit_fuel_overrides_the_untrusted_default() {
    let Some(path) = configured_krate_clock_component() else {
        return;
    };

    // With both flags the explicit --fuel 1 must win over the generous
    // --untrusted default, so the run still stops at exit 4.
    let mut args = clock_run_args(&path);
    args.insert(1, "--untrusted".to_string());
    args.insert(2, "--fuel".to_string());
    args.insert(3, "1".to_string());

    let output = krate()
        .args(&args)
        .output()
        .expect("run krate-clock with explicit fuel over untrusted");

    assert_eq!(
        output.status.code(),
        Some(4),
        "explicit --fuel must override the --untrusted default\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn configured_krate_cat_component_reads_granted_files() {
    let Some(path) = configured_krate_cat_component() else {
        return;
    };

    let dir = tempfile::tempdir().expect("create temp dir");
    let fixtures = dir.path().join("fixtures");
    std::fs::create_dir(&fixtures).expect("create fixtures dir");
    std::fs::write(fixtures.join("a.txt"), "hello from A\n").expect("write fixture A");
    std::fs::write(fixtures.join("b.txt"), "hello from B\n").expect("write fixture B");

    let output = krate()
        .current_dir(dir.path())
        .args(["run", "--grant", "fs.read:fixtures/**"])
        .arg(path)
        .args(["--", "fixtures/a.txt", "fixtures/b.txt"])
        .output()
        .expect("run krate-cat component");

    assert!(
        output.status.success(),
        "krate-cat failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "hello from A\nhello from B\n"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn configured_krate_cat_component_reads_from_sandbox_root() {
    let Some(path) = configured_krate_cat_component() else {
        return;
    };

    let dir = tempfile::tempdir().expect("create temp dir");
    let fixtures = dir.path().join("fixtures");
    std::fs::create_dir(&fixtures).expect("create fixtures dir");
    std::fs::write(fixtures.join("a.txt"), "hello from sandbox\n").expect("write fixture A");

    let output = krate()
        .args([
            "run",
            "--sandbox-root",
            dir.path().to_str().expect("sandbox root path"),
            "--grant",
            "fs.read:fixtures/**",
        ])
        .arg(path)
        .args(["--", "fixtures/a.txt"])
        .output()
        .expect("run krate-cat component with sandbox root");

    assert!(
        output.status.success(),
        "krate-cat sandbox-root run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "hello from sandbox\n"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn configured_krate_cat_component_runs_with_sample_manifest_auto_grant() {
    let Some(path) = configured_krate_cat_component() else {
        return;
    };

    let dir = tempfile::tempdir().expect("create temp dir");
    let fixtures = dir.path().join("fixtures");
    std::fs::create_dir(&fixtures).expect("create fixtures dir");
    std::fs::write(fixtures.join("a.txt"), "hello from manifest cat\n").expect("write fixture A");

    let output = krate()
        .current_dir(dir.path())
        .args([
            "run",
            "--auto-grant",
            "--manifest",
            sample_manifest("krate-cat")
                .to_str()
                .expect("manifest path"),
        ])
        .arg(path)
        .args(["--", "./fixtures/a.txt"])
        .output()
        .expect("run krate-cat with sample manifest");

    assert!(
        output.status.success(),
        "krate-cat manifest run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "hello from manifest cat\n"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn configured_krate_cat_component_denies_missing_file_grant() {
    let Some(path) = configured_krate_cat_component() else {
        return;
    };

    let dir = tempfile::tempdir().expect("create temp dir");
    let fixtures = dir.path().join("fixtures");
    std::fs::create_dir(&fixtures).expect("create fixtures dir");
    std::fs::write(fixtures.join("secret.txt"), "not granted\n").expect("write fixture");

    let output = krate()
        .current_dir(dir.path())
        .args(["run"])
        .arg(path)
        .args(["--", "fixtures/secret.txt"])
        .output()
        .expect("run krate-cat component without grant");

    assert_eq!(output.status.code(), Some(5));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("krate-cat: permission denied: fixtures/secret.txt"));
}

#[test]
fn configured_krate_cat_component_denies_file_outside_granted_glob() {
    let Some(path) = configured_krate_cat_component() else {
        return;
    };

    let dir = tempfile::tempdir().expect("create temp dir");
    let fixtures = dir.path().join("fixtures");
    std::fs::create_dir_all(fixtures.join("public")).expect("create public fixtures dir");
    std::fs::write(fixtures.join("secret.txt"), "not granted\n").expect("write fixture");

    let output = krate()
        .current_dir(dir.path())
        .args(["run", "--grant", "fs.read:fixtures/public/**"])
        .arg(path)
        .args(["--", "fixtures/secret.txt"])
        .output()
        .expect("run krate-cat component outside granted glob");

    assert_eq!(output.status.code(), Some(5));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("krate-cat: permission denied: fixtures/secret.txt"));
}

#[test]
fn a_redirect_does_not_carry_a_request_to_an_ungranted_host() {
    // The property: net.connect is granted per host, so the client must not
    // follow a redirect on the app's behalf. If it did, one granted host could
    // send the request anywhere while the permission prompt named only that
    // first host.
    let Some(path) = configured_krate_curl_component() else {
        return;
    };
    let Some((addr, server)) = spawn_redirect_fixture("http://evil.example.com/stolen") else {
        return;
    };
    let url = format!("http://{addr}/start");

    let output = krate()
        .args(["run", "--grant", &format!("net.connect:{addr}")])
        .arg(path)
        .args(["--", &url])
        .output()
        .expect("run krate-curl component");
    let accepted = server.join().expect("redirect fixture thread completed");
    if !accepted {
        eprintln!(
            "skipping redirect fixture: runtime could not connect to localhost in this environment"
        );
        return;
    }

    // Whatever the app does with the 302, the bytes of the redirect target must
    // never appear: reaching evil.example.com would have needed a grant nobody
    // gave.
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("stolen"),
        "the redirect must not have been followed\nstdout:\n{stdout}"
    );
}

#[test]
fn configured_krate_curl_component_fetches_granted_http_url() {
    let Some(path) = configured_krate_curl_component() else {
        return;
    };

    let body = b"hello from curl\n";
    let Some((addr, server)) = spawn_http_fixture(body) else {
        return;
    };
    let url = format!("http://{addr}/fixture.txt");

    let output = krate()
        .args(["run", "--grant", &format!("net.connect:{addr}")])
        .arg(path)
        .args(["--", &url])
        .output()
        .expect("run krate-curl component");
    let accepted = server.join().expect("HTTP fixture thread completed");
    if !accepted {
        eprintln!("skipping krate-curl success fixture: runtime could not connect to localhost fixture in this environment");
        return;
    }

    assert!(
        output.status.success(),
        "krate-curl failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, body);
    assert!(output.stderr.is_empty());
}

#[test]
fn configured_krate_curl_component_rejects_response_above_cli_limit() {
    let Some(path) = configured_krate_curl_component() else {
        return;
    };

    let body = b"too large for this run\n";
    let Some((addr, server)) = spawn_http_fixture(body) else {
        return;
    };
    let url = format!("http://{addr}/fixture.txt");

    let output = krate()
        .args([
            "run",
            "--grant",
            &format!("net.connect:{addr}"),
            "--max-http-response-bytes",
            "8",
        ])
        .arg(path)
        .args(["--", &url])
        .output()
        .expect("run krate-curl component with tiny HTTP response limit");
    let accepted = server.join().expect("HTTP fixture thread completed");
    if !accepted {
        eprintln!("skipping krate-curl response-limit fixture: runtime could not connect to localhost fixture in this environment");
        return;
    }

    assert_eq!(output.status.code(), Some(21));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("krate-curl: response too large"));
}

#[test]
fn configured_krate_curl_component_runs_with_sample_manifest_auto_grant() {
    let Some(path) = configured_krate_curl_component() else {
        return;
    };

    let body = b"hello from manifest curl\n";
    let Some((addr, server)) = spawn_http_fixture(body) else {
        return;
    };
    let url = format!("http://{addr}/fixture.txt");

    let output = krate()
        .args([
            "run",
            "--auto-grant",
            "--manifest",
            sample_manifest("krate-curl")
                .to_str()
                .expect("manifest path"),
        ])
        .arg(path)
        .args(["--", &url])
        .output()
        .expect("run krate-curl with sample manifest");
    let accepted = server.join().expect("HTTP fixture thread completed");
    if !accepted {
        eprintln!("skipping krate-curl manifest fixture: runtime could not connect to localhost fixture in this environment");
        return;
    }

    assert!(
        output.status.success(),
        "krate-curl manifest run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, body);
    assert!(output.stderr.is_empty());
}

#[test]
fn configured_krate_curl_component_denies_missing_net_grant() {
    let Some(path) = configured_krate_curl_component() else {
        return;
    };

    let output = krate()
        .args(["run"])
        .arg(path)
        .args(["--", "http://127.0.0.1:80/blocked"])
        .output()
        .expect("run krate-curl component without grant");

    assert_eq!(output.status.code(), Some(5));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("krate-curl: permission denied"));
}

#[test]
fn configured_krate_curl_component_reports_connect_failure() {
    let Some(path) = configured_krate_curl_component() else {
        return;
    };

    let Some(addr) = reserve_unused_local_addr() else {
        return;
    };
    let url = format!("http://{addr}/unreachable");

    let output = krate()
        .args(["run", "--grant", &format!("net.connect:{addr}")])
        .arg(path)
        .args(["--", &url])
        .output()
        .expect("run krate-curl against unused local port");

    assert_eq!(output.status.code(), Some(21));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("krate-curl: connection failed"));
}

#[test]
fn configured_krate_curl_component_reports_dns_failure() {
    let Some(path) = configured_krate_curl_component() else {
        return;
    };

    let host = "krate-does-not-exist.invalid";
    let url = format!("http://{host}/unreachable");

    let output = krate()
        .args(["run", "--grant", &format!("net.connect:{host}:80")])
        .arg(path)
        .args(["--", &url])
        .output()
        .expect("run krate-curl against unresolved host");

    assert_eq!(output.status.code(), Some(21));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("krate-curl: dns lookup failed")
            || stderr.contains("krate-curl: connection failed"),
        "unexpected stderr for unresolved host path: {stderr}"
    );
}

#[test]
fn configured_krate_curl_component_reports_protocol_error() {
    let Some(path) = configured_krate_curl_component() else {
        return;
    };

    let Some((addr, server)) = spawn_malformed_http_fixture(b"NOT-HTTP\r\n\r\n") else {
        return;
    };
    let url = format!("http://{addr}/malformed");

    let output = krate()
        .args(["run", "--grant", &format!("net.connect:{addr}")])
        .arg(path)
        .args(["--", &url])
        .output()
        .expect("run krate-curl against malformed HTTP fixture");
    let accepted = server
        .join()
        .expect("malformed HTTP fixture thread completed");
    if !accepted {
        eprintln!("skipping krate-curl protocol fixture: runtime could not connect to localhost fixture in this environment");
        return;
    }

    assert_eq!(output.status.code(), Some(21));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("krate-curl: protocol error"));
}

#[test]
fn configured_krate_curl_component_reports_timeout() {
    let Some(path) = configured_krate_curl_component() else {
        return;
    };

    let Some((addr, server)) = spawn_stalling_http_fixture(Duration::from_millis(1500)) else {
        return;
    };
    let url = format!("http://{addr}/stall");

    let output = krate()
        .args([
            "run",
            "--http-timeout-millis",
            "1000",
            "--grant",
            &format!("net.connect:{addr}"),
        ])
        .arg(path)
        .args(["--", &url])
        .output()
        .expect("run krate-curl against stalling HTTP fixture");
    let accepted = server
        .join()
        .expect("stalling HTTP fixture thread completed");
    if !accepted {
        eprintln!("skipping krate-curl timeout fixture: runtime could not connect to localhost fixture in this environment");
        return;
    }

    assert_eq!(output.status.code(), Some(21));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("krate-curl: request timed out"));
}

#[test]
fn configured_krate_go_clock_component_matches_deterministic_fixture_snapshot() {
    let Some(path) = configured_go_component(
        "KRATE_GO_CLOCK_WASM",
        "krate-go-clock component test",
        "krate_go_clock.wasm",
    ) else {
        return;
    };

    let output = krate()
        .args([
            "run",
            "--test-time",
            "1234567890",
            "--test-locale",
            "en-US",
            "--test-timezone",
            "UTC",
        ])
        .arg(path)
        .output()
        .expect("run krate-go-clock component with deterministic locale/timezone");

    assert!(
        output.status.success(),
        "krate-go-clock deterministic snapshot failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        concat!(
            "app=krate-go-clock\n",
            "locale=en-US\n",
            "timezone=UTC\n",
            "date=1970-01-15 06:56\n"
        )
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn configured_krate_go_cat_component_reads_granted_files() {
    let Some(path) = configured_go_component(
        "KRATE_GO_CAT_WASM",
        "krate-go-cat component test",
        "krate_go_cat.wasm",
    ) else {
        return;
    };

    let dir = tempfile::tempdir().expect("create temp dir");
    let fixtures = dir.path().join("fixtures");
    std::fs::create_dir(&fixtures).expect("create fixtures dir");
    std::fs::write(fixtures.join("a.txt"), "hello from go A\n").expect("write fixture A");
    std::fs::write(fixtures.join("b.txt"), "hello from go B\n").expect("write fixture B");

    let output = krate()
        .current_dir(dir.path())
        .args(["run", "--grant", "fs.read:fixtures/**"])
        .arg(path)
        .args(["--", "fixtures/a.txt", "fixtures/b.txt"])
        .output()
        .expect("run krate-go-cat component");

    assert!(
        output.status.success(),
        "krate-go-cat failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "hello from go A\nhello from go B\n"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn configured_krate_go_curl_component_fetches_granted_http_url() {
    let Some(path) = configured_go_component(
        "KRATE_GO_CURL_WASM",
        "krate-go-curl component test",
        "krate_go_curl.wasm",
    ) else {
        return;
    };

    let body = b"hello from go curl\n";
    let Some((addr, server)) = spawn_http_fixture(body) else {
        return;
    };
    let url = format!("http://{addr}/fixture.txt");

    let output = krate()
        .args(["run", "--grant", &format!("net.connect:{addr}")])
        .arg(path)
        .args(["--", &url])
        .output()
        .expect("run krate-go-curl component");
    let accepted = server.join().expect("HTTP fixture thread completed");
    if !accepted {
        eprintln!("skipping krate-go-curl fixture: runtime could not connect to localhost fixture in this environment");
        return;
    }

    assert!(
        output.status.success(),
        "krate-go-curl failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, body);
    assert!(output.stderr.is_empty());
}

#[test]
fn configured_krate_go_curl_component_denies_missing_grant() {
    let Some(path) = configured_go_component(
        "KRATE_GO_CURL_WASM",
        "krate-go-curl component test",
        "krate_go_curl.wasm",
    ) else {
        return;
    };

    let output = krate()
        .arg("run")
        .arg(path)
        .args(["--", "http://example.com/"])
        .output()
        .expect("run krate-go-curl component without grant");

    assert_eq!(output.status.code(), Some(5));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("krate-go-curl: permission denied"));
}

#[test]
fn configured_krate_go_curl_component_reports_unresolved_host() {
    let Some(path) = configured_go_component(
        "KRATE_GO_CURL_WASM",
        "krate-go-curl component test",
        "krate_go_curl.wasm",
    ) else {
        return;
    };

    let host = "krate-does-not-exist.invalid";
    let url = format!("http://{host}/unreachable");

    let output = krate()
        .args(["run", "--grant", &format!("net.connect:{host}:80")])
        .arg(path)
        .args(["--", &url])
        .output()
        .expect("run krate-go-curl against unresolved host");

    assert_eq!(output.status.code(), Some(21));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("krate-go-curl: dns lookup failed")
            || stderr.contains("krate-go-curl: connection failed")
            || stderr.contains("krate-go-curl: fetch failed"),
        "unexpected unresolved-host stderr: {stderr}"
    );
}

#[test]
fn configured_krate_ts_clock_component_matches_deterministic_fixture_snapshot() {
    let Some(path) = configured_ts_component(
        "KRATE_TS_CLOCK_WASM",
        "krate-ts-clock component test",
        "krate_ts_clock.wasm",
    ) else {
        return;
    };

    let output = krate()
        .args([
            "run",
            "--test-time",
            "1234567890",
            "--test-locale",
            "en-US",
            "--test-timezone",
            "UTC",
        ])
        .arg(path)
        .output()
        .expect("run krate-ts-clock component with deterministic locale/timezone");

    assert!(
        output.status.success(),
        "krate-ts-clock deterministic snapshot failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        concat!(
            "app=krate-ts-clock\n",
            "locale=en-US\n",
            "timezone=UTC\n",
            "date=1970-01-15 06:56\n"
        )
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn configured_krate_ts_cat_component_reads_granted_files() {
    let Some(path) = configured_ts_component(
        "KRATE_TS_CAT_WASM",
        "krate-ts-cat component test",
        "krate_ts_cat.wasm",
    ) else {
        return;
    };

    let dir = tempfile::tempdir().expect("create temp dir");
    let fixtures = dir.path().join("fixtures");
    std::fs::create_dir(&fixtures).expect("create fixtures dir");
    std::fs::write(fixtures.join("a.txt"), "hello from ts A\n").expect("write fixture A");
    std::fs::write(fixtures.join("b.txt"), "hello from ts B\n").expect("write fixture B");

    let output = krate()
        .current_dir(dir.path())
        .args(["run", "--grant", "fs.read:fixtures/**"])
        .arg(path)
        .args(["--", "fixtures/a.txt", "fixtures/b.txt"])
        .output()
        .expect("run krate-ts-cat component");

    assert!(
        output.status.success(),
        "krate-ts-cat failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "hello from ts A\nhello from ts B\n"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn configured_krate_ts_curl_component_fetches_granted_http_url() {
    let Some(path) = configured_ts_component(
        "KRATE_TS_CURL_WASM",
        "krate-ts-curl component test",
        "krate_ts_curl.wasm",
    ) else {
        return;
    };

    let body = b"hello from ts curl\n";
    let Some((addr, server)) = spawn_http_fixture(body) else {
        return;
    };
    let url = format!("http://{addr}/fixture.txt");

    let output = krate()
        .args(["run", "--grant", &format!("net.connect:{addr}")])
        .arg(path)
        .args(["--", &url])
        .output()
        .expect("run krate-ts-curl component");
    let accepted = server.join().expect("HTTP fixture thread completed");
    if !accepted {
        eprintln!("skipping krate-ts-curl fixture: runtime could not connect to localhost fixture in this environment");
        return;
    }

    assert!(
        output.status.success(),
        "krate-ts-curl failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, body);
    assert!(output.stderr.is_empty());
}

#[test]
fn configured_krate_ts_curl_component_denies_missing_grant() {
    let Some(path) = configured_ts_component(
        "KRATE_TS_CURL_WASM",
        "krate-ts-curl component test",
        "krate_ts_curl.wasm",
    ) else {
        return;
    };

    let output = krate()
        .arg("run")
        .arg(path)
        .args(["--", "http://example.com/"])
        .output()
        .expect("run krate-ts-curl component without grant");

    assert_eq!(output.status.code(), Some(5));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("krate-ts-curl: permission denied"));
}

#[test]
fn configured_krate_ts_curl_component_reports_unresolved_host() {
    let Some(path) = configured_ts_component(
        "KRATE_TS_CURL_WASM",
        "krate-ts-curl component test",
        "krate_ts_curl.wasm",
    ) else {
        return;
    };

    let host = "krate-does-not-exist.invalid";
    let url = format!("http://{host}/unreachable");

    let output = krate()
        .args(["run", "--grant", &format!("net.connect:{host}:80")])
        .arg(path)
        .args(["--", &url])
        .output()
        .expect("run krate-ts-curl against unresolved host");

    assert_eq!(output.status.code(), Some(21));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("krate-ts-curl: dns lookup failed")
            || stderr.contains("krate-ts-curl: connection failed")
            || stderr.contains("krate-ts-curl: fetch failed"),
        "unexpected unresolved-host stderr: {stderr}"
    );
}

#[test]
fn configured_krate_ts_curl_component_reports_invalid_url() {
    let Some(path) = configured_ts_component(
        "KRATE_TS_CURL_WASM",
        "krate-ts-curl component test",
        "krate_ts_curl.wasm",
    ) else {
        return;
    };

    let output = krate()
        .args(["run", "--grant", "net.connect:*:*"])
        .arg(path)
        .args(["--", "not-a-url"])
        .output()
        .expect("run krate-ts-curl against invalid URL");

    assert_eq!(output.status.code(), Some(20));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("krate-ts-curl: invalid url"));
}

#[test]
fn language_variants_curl_permission_denied_matches_rust_go_ts() {
    let Some(rust_path) = configured_krate_curl_component() else {
        return;
    };
    let Some(go_path) = configured_go_component(
        "KRATE_GO_CURL_WASM",
        "krate-go-curl component test",
        "krate_go_curl.wasm",
    ) else {
        eprintln!("skipping language variant curl denial parity: Go fixture is unavailable");
        return;
    };
    let Some(ts_path) = configured_ts_component(
        "KRATE_TS_CURL_WASM",
        "krate-ts-curl component test",
        "krate_ts_curl.wasm",
    ) else {
        eprintln!(
            "skipping language variant curl denial parity: TypeScript fixture is unavailable"
        );
        return;
    };

    let run_without_grant = |path: &PathBuf, label: &str| {
        let output = krate()
            .arg("run")
            .arg(path)
            .args(["--", "http://example.com/"])
            .output()
            .expect("run language variant curl component without grant");

        assert_eq!(
            output.status.code(),
            Some(5),
            "{label} returned unexpected status for missing net grant"
        );
        assert!(
            output.stdout.is_empty(),
            "{label} wrote stdout on missing grant"
        );
        String::from_utf8_lossy(&output.stderr).to_string()
    };

    let rust_stderr = run_without_grant(&rust_path, "krate-curl");
    let go_stderr = run_without_grant(&go_path, "krate-go-curl");
    let ts_stderr = run_without_grant(&ts_path, "krate-ts-curl");
    assert!(rust_stderr.contains("permission denied"));
    assert!(go_stderr.contains("permission denied"));
    assert!(ts_stderr.contains("permission denied"));
}

#[test]
fn language_variants_curl_invalid_url_matches_rust_go_ts() {
    let Some(rust_path) = configured_krate_curl_component() else {
        return;
    };
    let Some(go_path) = configured_go_component(
        "KRATE_GO_CURL_WASM",
        "krate-go-curl component test",
        "krate_go_curl.wasm",
    ) else {
        eprintln!("skipping language variant curl invalid-url parity: Go fixture is unavailable");
        return;
    };
    let Some(ts_path) = configured_ts_component(
        "KRATE_TS_CURL_WASM",
        "krate-ts-curl component test",
        "krate_ts_curl.wasm",
    ) else {
        eprintln!(
            "skipping language variant curl invalid-url parity: TypeScript fixture is unavailable"
        );
        return;
    };

    let run_invalid_url = |path: &PathBuf, label: &str| {
        let output = krate()
            .args(["run", "--grant", "net.connect:*:*"])
            .arg(path)
            .args(["--", "not-a-url"])
            .output()
            .expect("run language variant curl component against invalid URL");

        assert_eq!(
            output.status.code(),
            Some(20),
            "{label} returned unexpected status for invalid URL"
        );
        assert!(
            output.stdout.is_empty(),
            "{label} wrote stdout on invalid URL"
        );
        String::from_utf8_lossy(&output.stderr).to_string()
    };

    let rust_stderr = run_invalid_url(&rust_path, "krate-curl");
    let go_stderr = run_invalid_url(&go_path, "krate-go-curl");
    let ts_stderr = run_invalid_url(&ts_path, "krate-ts-curl");
    assert!(rust_stderr.contains("invalid url"));
    assert!(go_stderr.contains("invalid url"));
    assert!(ts_stderr.contains("invalid url"));
}

#[test]
fn language_variants_curl_unresolved_host_matches_rust_go_ts() {
    let Some(rust_path) = configured_krate_curl_component() else {
        return;
    };
    let Some(go_path) = configured_go_component(
        "KRATE_GO_CURL_WASM",
        "krate-go-curl component test",
        "krate_go_curl.wasm",
    ) else {
        eprintln!(
            "skipping language variant curl unresolved-host parity: Go fixture is unavailable"
        );
        return;
    };
    let Some(ts_path) = configured_ts_component(
        "KRATE_TS_CURL_WASM",
        "krate-ts-curl component test",
        "krate_ts_curl.wasm",
    ) else {
        eprintln!(
            "skipping language variant curl unresolved-host parity: TypeScript fixture is unavailable"
        );
        return;
    };

    let host = "krate-does-not-exist.invalid";
    let url = format!("http://{host}/unreachable");
    let grant = format!("net.connect:{host}:80");

    let run_unresolved = |path: &PathBuf, label: &str| {
        let output = krate()
            .args(["run", "--grant", &grant])
            .arg(path)
            .args(["--", &url])
            .output()
            .expect("run language variant curl component against unresolved host");

        assert_eq!(
            output.status.code(),
            Some(21),
            "{label} returned unexpected status for unresolved host"
        );
        assert!(
            output.stdout.is_empty(),
            "{label} wrote stdout on unresolved-host path"
        );
        String::from_utf8_lossy(&output.stderr).to_string()
    };

    let rust_stderr = run_unresolved(&rust_path, "krate-curl");
    let go_stderr = run_unresolved(&go_path, "krate-go-curl");
    let ts_stderr = run_unresolved(&ts_path, "krate-ts-curl");

    let has_unresolved_error = |stderr: &str| {
        stderr.contains("dns lookup failed")
            || stderr.contains("connection failed")
            || stderr.contains("fetch failed")
    };

    assert!(
        has_unresolved_error(&rust_stderr),
        "Rust unresolved-host stderr drifted: {rust_stderr}"
    );
    assert!(
        has_unresolved_error(&go_stderr),
        "Go unresolved-host stderr drifted: {go_stderr}"
    );
    assert!(
        has_unresolved_error(&ts_stderr),
        "TypeScript unresolved-host stderr drifted: {ts_stderr}"
    );
}

#[test]
fn configured_krate_go_curl_component_reports_invalid_url() {
    let Some(path) = configured_go_component(
        "KRATE_GO_CURL_WASM",
        "krate-go-curl component test",
        "krate_go_curl.wasm",
    ) else {
        return;
    };

    let output = krate()
        .args(["run", "--grant", "net.connect:*:*"])
        .arg(path)
        .args(["--", "not-a-url"])
        .output()
        .expect("run krate-go-curl against invalid URL");

    assert_eq!(output.status.code(), Some(20));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("krate-go-curl: invalid url"));
}

#[test]
fn language_variants_clock_output_matches_across_rust_go_ts() {
    let Some(rust_path) = configured_krate_clock_component() else {
        return;
    };
    let Some(go_path) = configured_go_component(
        "KRATE_GO_CLOCK_WASM",
        "krate-go-clock component test",
        "krate_go_clock.wasm",
    ) else {
        eprintln!("skipping language variant clock parity: Go fixture is unavailable");
        return;
    };
    let Some(ts_path) = configured_ts_component(
        "KRATE_TS_CLOCK_WASM",
        "krate-ts-clock component test",
        "krate_ts_clock.wasm",
    ) else {
        eprintln!("skipping language variant clock parity: TypeScript fixture is unavailable");
        return;
    };

    let run_clock = |path: &PathBuf, label: &str| {
        let output = krate()
            .args([
                "run",
                "--test-time",
                "1234567890",
                "--test-locale",
                "en-US",
                "--test-timezone",
                "UTC",
            ])
            .arg(path)
            .output()
            .expect("run language variant clock component");

        assert!(
            output.status.success(),
            "{label} failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty(), "{label} wrote to stderr");
        output.stdout
    };

    let rust_stdout = run_clock(&rust_path, "krate-clock");
    let go_stdout = run_clock(&go_path, "krate-go-clock");
    let ts_stdout = run_clock(&ts_path, "krate-ts-clock");

    assert_eq!(go_stdout, rust_stdout, "Go clock output drifted from Rust");
    assert_eq!(
        ts_stdout, rust_stdout,
        "TypeScript clock output drifted from Rust"
    );
}

#[test]
fn language_variants_cat_output_matches_across_rust_go_ts() {
    let Some(rust_path) = configured_krate_cat_component() else {
        return;
    };
    let Some(go_path) = configured_go_component(
        "KRATE_GO_CAT_WASM",
        "krate-go-cat component test",
        "krate_go_cat.wasm",
    ) else {
        eprintln!("skipping language variant cat parity: Go fixture is unavailable");
        return;
    };
    let Some(ts_path) = configured_ts_component(
        "KRATE_TS_CAT_WASM",
        "krate-ts-cat component test",
        "krate_ts_cat.wasm",
    ) else {
        eprintln!("skipping language variant cat parity: TypeScript fixture is unavailable");
        return;
    };

    let dir = tempfile::tempdir().expect("create temp dir");
    let fixtures = dir.path().join("fixtures");
    std::fs::create_dir(&fixtures).expect("create fixtures dir");
    std::fs::write(fixtures.join("a.txt"), "hello from parity A\n").expect("write fixture A");
    std::fs::write(fixtures.join("b.txt"), "hello from parity B\n").expect("write fixture B");

    let run_cat = |path: &PathBuf, label: &str| {
        let output = krate()
            .current_dir(dir.path())
            .args(["run", "--grant", "fs.read:fixtures/**"])
            .arg(path)
            .args(["--", "fixtures/a.txt", "fixtures/b.txt"])
            .output()
            .expect("run language variant cat component");

        assert!(
            output.status.success(),
            "{label} failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty(), "{label} wrote to stderr");
        output.stdout
    };

    let rust_stdout = run_cat(&rust_path, "krate-cat");
    let go_stdout = run_cat(&go_path, "krate-go-cat");
    let ts_stdout = run_cat(&ts_path, "krate-ts-cat");

    assert_eq!(go_stdout, rust_stdout, "Go cat output drifted from Rust");
    assert_eq!(
        ts_stdout, rust_stdout,
        "TypeScript cat output drifted from Rust"
    );
}

#[test]
fn language_variants_curl_output_matches_across_rust_go_ts() {
    let Some(rust_path) = configured_krate_curl_component() else {
        return;
    };
    let Some(go_path) = configured_go_component(
        "KRATE_GO_CURL_WASM",
        "krate-go-curl component test",
        "krate_go_curl.wasm",
    ) else {
        eprintln!("skipping language variant curl parity: Go fixture is unavailable");
        return;
    };
    let Some(ts_path) = configured_ts_component(
        "KRATE_TS_CURL_WASM",
        "krate-ts-curl component test",
        "krate_ts_curl.wasm",
    ) else {
        eprintln!("skipping language variant curl parity: TypeScript fixture is unavailable");
        return;
    };

    let body = b"hello from parity curl\n";
    let Some((addr, server)) = spawn_http_fixture(body) else {
        return;
    };
    let url = format!("http://{addr}/fixture.txt");
    let grant = format!("net.connect:{addr}");

    let run_curl = |path: &PathBuf, label: &str| {
        let output = krate()
            .args(["run", "--grant", &grant])
            .arg(path)
            .args(["--", &url])
            .output()
            .expect("run language variant curl component");

        assert!(
            output.status.success(),
            "{label} failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty(), "{label} wrote to stderr");
        output.stdout
    };

    let rust_stdout = run_curl(&rust_path, "krate-curl");
    let go_stdout = run_curl(&go_path, "krate-go-curl");
    let ts_stdout = run_curl(&ts_path, "krate-ts-curl");

    let accepted = server.join().expect("HTTP fixture thread completed");
    if !accepted {
        eprintln!(
            "skipping language variant curl parity: runtime could not connect to localhost fixture in this environment"
        );
        return;
    }

    assert_eq!(rust_stdout, body, "Rust curl output did not match fixture");
    assert_eq!(go_stdout, rust_stdout, "Go curl output drifted from Rust");
    assert_eq!(
        ts_stdout, rust_stdout,
        "TypeScript curl output drifted from Rust"
    );
}

#[test]
fn fuel_limit_exits_with_limit_code() {
    let Some(path) = configured_hello_component() else {
        return;
    };

    let output = krate()
        .args(["run", "--fuel", "1"])
        .arg(path)
        .output()
        .expect("run krate hello component with low fuel");

    assert_eq!(output.status.code(), Some(4));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("limit exceeded: fuel exhausted"));
}

/// The fuel limit, on a fixture that is always here (K-269).
///
/// The test above needs a component built by a CI lane that installs the
/// component toolchain. Where that fixture is absent it RETURNS, and a test
/// that returns reports ok -- 0.00s for something meant to spawn a process
/// and run a component to exhaustion. Ten tests in this file were in that
/// state, so the suite counted them as passing while they did nothing.
///
/// This one uses a shipped app, which is tracked in git and therefore
/// present on every machine and in every lane. Stopping a run is the
/// difference between "the app failed" and "the app was stopped", and that
/// distinction should not be tested only where a toolchain happens to be
/// installed.
#[test]
fn stopping_a_run_is_reported_differently_from_the_app_failing() {
    let bundle = std::path::Path::new("../../evidence/store/krate-checklist.krate");
    assert!(
        bundle.exists(),
        "the shipped app this test depends on is missing: {}",
        bundle.display()
    );

    // One unit of fuel cannot get a real app through instantiation.
    let stopped = krate()
        .arg("run")
        .arg(bundle)
        .args(["--headless", "--auto-grant", "--fuel", "1"])
        .output()
        .expect("run the app with almost no fuel");
    assert_eq!(
        stopped.status.code(),
        Some(4),
        "a run stopped by a limit has its own exit code: {}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    let stderr = String::from_utf8_lossy(&stopped.stderr);
    assert!(
        stderr.contains("fuel exhausted"),
        "it must say WHY it stopped: {stderr}"
    );

    // The same app with a normal budget runs, so the exit code above is
    // about the limit and not about the app being broken.
    let ran = krate()
        .arg("run")
        .arg(bundle)
        .args(["--headless", "--auto-grant"])
        .output()
        .expect("run the app normally");
    assert_eq!(
        ran.status.code(),
        Some(0),
        "the same app must run when it is given room: {}",
        String::from_utf8_lossy(&ran.stderr)
    );
}

#[test]
fn memory_limit_exits_with_limit_code() {
    let Some(path) = configured_hello_component() else {
        return;
    };

    let output = krate()
        .args(["run", "--mem-limit", "0"])
        .arg(path)
        .output()
        .expect("run krate hello component with low memory");

    assert_eq!(
        output.status.code(),
        Some(4),
        "a memory ceiling that bites is a LIMIT (exit 4), not a generic \
         failure (exit 1): {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    // The prefix is ours and the rest is wasmtime's, so this asserts what we
    // control and what the sentence has to MEAN, not the engine's exact
    // words. It used to demand "limit exceeded: memory limit exceeded" --
    // a phrasing wasmtime no longer uses, so the test pinned a message
    // nothing produced while the real one ("memory minimum size of 17 pages
    // exceeds memory limits") went unclassified and exited 1.
    assert!(
        stderr.contains("limit exceeded:"),
        "the run must say a limit was reached: {stderr}"
    );
    assert!(
        stderr.contains("memory"),
        "and which limit it was: {stderr}"
    );
}

#[test]
fn run_with_manifest_denies_missing_required_capability() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let wasm_path = dir.path().join("app.wasm");
    let manifest_path = dir.path().join("manifest.toml");
    std::fs::write(&wasm_path, b"not actually wasm").expect("write wasm placeholder");
    std::fs::write(
        &manifest_path,
        r#"
            [app]
            id = "com.example.denied"
            name = "Denied"
            version = "1.0.0"
            entry = "app.wasm"
            world = "krate:app/cli@0.1.0"

            [[capabilities]]
            cap = "fs.read:./data/**"
            rationale = "Read data"
            required = true
        "#,
    )
    .expect("write manifest");

    let output = krate()
        .arg("run")
        .arg(&wasm_path)
        .output()
        .expect("run krate with sidecar manifest");

    assert_eq!(output.status.code(), Some(5));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("needs permission it was not given"));
    // The exact capability is still named, alongside its plain phrase.
    assert!(stderr.contains("fs.read:data/**"));
    assert!(stderr.contains("read files in data"));
}

#[test]
fn run_with_manifest_rejects_entry_mismatch() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let wasm_path = dir.path().join("other.wasm");
    std::fs::write(&wasm_path, b"not actually wasm").expect("write wasm placeholder");
    std::fs::write(
        dir.path().join("manifest.toml"),
        r#"
            [app]
            id = "com.example.mismatch"
            name = "Mismatch"
            version = "1.0.0"
            entry = "app.wasm"
            world = "krate:app/cli@0.1.0"
        "#,
    )
    .expect("write manifest");

    let output = krate()
        .arg("run")
        .arg(&wasm_path)
        .output()
        .expect("run krate with mismatched sidecar manifest");

    assert_eq!(output.status.code(), Some(5));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("manifest entry"));
    assert!(stderr.contains("does not match"));
}

#[test]
fn run_with_manifest_and_explicit_grant_reaches_runtime() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let wasm_path = dir.path().join("app.wasm");
    std::fs::write(&wasm_path, b"not actually wasm").expect("write wasm placeholder");
    std::fs::write(
        dir.path().join("manifest.toml"),
        r#"
            [app]
            id = "com.example.granted"
            name = "Granted"
            version = "1.0.0"
            entry = "app.wasm"
            world = "krate:app/cli@0.1.0"

            [[capabilities]]
            cap = "fs.read:./data/**"
            rationale = "Read data"
            required = true
        "#,
    )
    .expect("write manifest");

    let output = krate()
        .args(["run", "--grant", "fs.read:./data/**"])
        .arg(&wasm_path)
        .output()
        .expect("run krate with granted sidecar manifest");

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid wasm component"));
}

/// Looking at an app must not run it (IC-231).
///
/// This is the security property the inspection path exists for: somebody
/// sent you a file, and the safe first move is to look at what it wants
/// before deciding. If looking executed the guest, that decision would come
/// after the app had already had its turn.
///
/// The other dump-caps tests use a file that is not even wasm, so nothing
/// COULD run in them -- they prove the command tolerates junk, not that a
/// real app stays still. This one uses a shipped app that announces itself
/// on stdout when it runs, so the absence of that line is the evidence.
#[test]
fn inspecting_a_real_app_does_not_run_it() {
    let bundle = std::path::Path::new("../../evidence/store/krate-checklist.krate");
    assert!(
        bundle.exists(),
        "the shipped app this test depends on is missing: {}",
        bundle.display()
    );

    // First establish what running it actually looks like, so the assertion
    // below is anchored to observed behaviour rather than a guess.
    let ran = krate()
        .arg("run")
        .arg(bundle)
        .args(["--headless", "--auto-grant"])
        .output()
        .expect("run the app");
    let ran_stdout = String::from_utf8_lossy(&ran.stdout).into_owned();
    assert!(
        ran_stdout.contains("items:"),
        "this test depends on the app announcing itself when it runs; it \
         printed: {ran_stdout}"
    );

    // Now inspect the same app. The announcement must not appear.
    let looked = krate()
        .arg("run")
        .arg(bundle)
        .arg("--dump-caps")
        .output()
        .expect("inspect the app");
    let stdout = String::from_utf8_lossy(&looked.stdout);
    assert!(
        looked.status.success(),
        "inspecting an app must succeed: {}",
        String::from_utf8_lossy(&looked.stderr)
    );
    assert!(
        stdout.contains("Effective capabilities"),
        "inspection must actually report something: {stdout}"
    );
    assert!(
        !stdout.contains("items:"),
        "the app RAN while being inspected -- looking at a file somebody sent \
         you must never execute it: {stdout}"
    );
}

#[test]
fn run_dump_caps_prints_effective_policy_without_running_component() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let wasm_path = dir.path().join("app.wasm");
    std::fs::write(&wasm_path, b"not actually wasm").expect("write wasm placeholder");

    let output = krate()
        .args(["run", "--dump-caps", "--grant", "fs.read:./data/**"])
        .arg(&wasm_path)
        .output()
        .expect("run krate dump caps");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains("Effective capabilities"));
    assert!(stdout.contains("io.stdout"));
    assert!(stdout.contains("fs.read:data/**"));
    assert!(!stderr.contains("invalid wasm component"));
}

#[test]
fn run_dump_caps_inspects_a_gated_app_without_granting_anything() {
    // Looking at an app you were sent is the safe first move, and it is the
    // case where every required capability is still ungranted. Dumping ran
    // behind the permission wall once, so precisely these apps answered with
    // "it did not run" and exit 5 instead of the capability list.
    let dir = tempfile::tempdir().expect("create temp dir");
    let wasm_path = dir.path().join("app.wasm");
    let manifest_path = dir.path().join("manifest.toml");
    std::fs::write(&wasm_path, b"not actually wasm").expect("write wasm placeholder");
    std::fs::write(
        &manifest_path,
        r#"
            [app]
            id = "com.example.gated"
            name = "Gated"
            version = "1.0.0"
            entry = "app.wasm"
            world = "krate:app/cli@0.1.0"

            [[capabilities]]
            cap = "fs.write:./notes/**"
            rationale = "Save notes"
            required = true
        "#,
    )
    .expect("write manifest");

    let output = krate()
        .args([
            "run",
            "--dump-caps",
            "--manifest",
            manifest_path.to_str().expect("manifest path"),
        ])
        .arg(&wasm_path)
        .output()
        .expect("run krate dump caps on a gated app");

    assert!(
        output.status.success(),
        "--dump-caps must inspect without enforcing the wall\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Effective capabilities"));
    // Inspecting must not hand out the grant it is reporting on.
    assert!(!stdout.contains("\n  - fs.write:notes/**"));
    // But it must say what is coming. Listing only the default grants showed no
    // file access at all for an app whose whole purpose is saving files, so the
    // first permission prompt arrived as a surprise.
    assert!(stdout.contains("This app will ask for"));
    assert!(stdout.contains("save files in notes"));
}

#[test]
fn run_dump_caps_json_reports_effective_policy_without_running_component() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let wasm_path = dir.path().join("app.wasm");
    let manifest_path = dir.path().join("manifest.toml");
    std::fs::write(&wasm_path, b"not actually wasm").expect("write wasm placeholder");
    std::fs::write(
        &manifest_path,
        r#"
            [app]
            id = "com.example.dump"
            name = "Dump"
            version = "1.0.0"
            entry = "app.wasm"
            world = "krate:app/cli@0.1.0"

            [[capabilities]]
            cap = "io.stdout"
            rationale = "Print output"
            required = true

            [[capabilities]]
            cap = "fs.read:./data/**"
            rationale = "Read data"
            required = true
        "#,
    )
    .expect("write manifest");

    let output = krate()
        .args([
            "run",
            "--auto-grant",
            "--dump-caps",
            "--dump-caps-format",
            "json",
            "--manifest",
            manifest_path.to_str().expect("manifest path"),
        ])
        .arg(&wasm_path)
        .output()
        .expect("run krate dump caps json");

    assert!(
        output.status.success(),
        "dump caps json failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains(r#""wasm":"#));
    assert!(stdout.contains(r#""id": "com.example.dump""#));
    assert!(stdout.contains(r#""name": "Dump""#));
    assert!(stdout.contains(r#""capabilities":"#));
    assert!(stdout.contains(r#""io.stdout""#));
    assert!(stdout.contains(r#""fs.read:data/**""#));
    assert!(!stderr.contains("invalid wasm component"));
}

#[test]
fn run_log_grants_records_effective_session_policy() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let wasm_path = dir.path().join("app.wasm");
    let manifest_path = dir.path().join("manifest.toml");
    let log_path = dir.path().join("grants.log");
    std::fs::write(&wasm_path, b"not actually wasm").expect("write wasm placeholder");
    std::fs::write(
        &manifest_path,
        r#"
            [app]
            id = "com.example.audit"
            name = "Audit"
            version = "1.0.0"
            entry = "app.wasm"
            world = "krate:app/cli@0.1.0"

            [[capabilities]]
            cap = "io.stdout"
            rationale = "Print output"
            required = true

            [[capabilities]]
            cap = "fs.read:./data/**"
            rationale = "Read data"
            required = true
        "#,
    )
    .expect("write manifest");

    let output = krate()
        .args([
            "run",
            "--auto-grant",
            "--dump-caps",
            "--manifest",
            manifest_path.to_str().expect("manifest path"),
            "--log-grants",
            log_path.to_str().expect("log path"),
        ])
        .arg(&wasm_path)
        .output()
        .expect("run dump caps with grant log");

    assert!(
        output.status.success(),
        "grant log run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let log = std::fs::read_to_string(&log_path).expect("read grant log");
    assert!(log.contains("Krate grant log"));
    assert!(log.contains("app id           com.example.audit"));
    assert!(log.contains("app name         Audit"));
    assert!(log.contains("manifest world   krate:app/cli@0.1.0"));
    assert!(log.contains("  - io.stdout"));
    assert!(log.contains("  - fs.read:data/**"));
}

#[test]
fn run_log_grants_jsonl_records_effective_session_policy() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let wasm_path = dir.path().join("app.wasm");
    let manifest_path = dir.path().join("manifest.toml");
    let log_path = dir.path().join("grants.jsonl");
    std::fs::write(&wasm_path, b"not actually wasm").expect("write wasm placeholder");
    std::fs::write(
        &manifest_path,
        r#"
            [app]
            id = "com.example.audit"
            name = "Audit"
            version = "1.0.0"
            entry = "app.wasm"
            world = "krate:app/cli@0.1.0"

            [[capabilities]]
            cap = "io.stdout"
            rationale = "Print output"
            required = true

            [[capabilities]]
            cap = "fs.read:./data/**"
            rationale = "Read data"
            required = true
        "#,
    )
    .expect("write manifest");

    let output = krate()
        .args([
            "run",
            "--auto-grant",
            "--dump-caps",
            "--manifest",
            manifest_path.to_str().expect("manifest path"),
            "--log-grants",
            log_path.to_str().expect("log path"),
            "--log-grants-format",
            "jsonl",
        ])
        .arg(&wasm_path)
        .output()
        .expect("run dump caps with grant jsonl log");

    assert!(
        output.status.success(),
        "grant jsonl log run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let log = std::fs::read_to_string(&log_path).expect("read grant log");
    let lines = log.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains(r#""format_version":1"#));
    assert!(lines[0].contains(r#""event":"krate.grants""#));
    assert!(lines[0].contains(r#""id":"com.example.audit""#));
    assert!(lines[0].contains(r#""name":"Audit""#));
    assert!(lines[0].contains(r#""io.stdout""#));
    assert!(lines[0].contains(r#""fs.read:data/**""#));
}

#[test]
fn run_with_manifest_auto_grant_reaches_runtime() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let wasm_path = dir.path().join("app.wasm");
    std::fs::write(&wasm_path, b"not actually wasm").expect("write wasm placeholder");
    std::fs::write(
        dir.path().join("manifest.toml"),
        r#"
            [app]
            id = "com.example.auto"
            name = "Auto"
            version = "1.0.0"
            entry = "app.wasm"
            world = "krate:app/cli@0.1.0"

            [[capabilities]]
            cap = "net.connect:api.example.com:443"
            rationale = "Sync data"
            required = true
        "#,
    )
    .expect("write manifest");

    let output = krate()
        .args(["run", "--auto-grant"])
        .arg(&wasm_path)
        .output()
        .expect("run krate with auto-grant");

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid wasm component"));
}

#[test]
fn run_with_manifest_prompt_can_grant_required_capability() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let wasm_path = dir.path().join("app.wasm");
    std::fs::write(&wasm_path, b"not actually wasm").expect("write wasm placeholder");
    std::fs::write(
        dir.path().join("manifest.toml"),
        r#"
            [app]
            id = "com.example.prompt"
            name = "Prompt"
            version = "1.0.0"
            entry = "app.wasm"
            world = "krate:app/cli@0.1.0"

            [[capabilities]]
            cap = "fs.read:./data/**"
            rationale = "Read data"
            required = true
        "#,
    )
    .expect("write manifest");

    let mut child = krate()
        .args(["run", "--prompt"])
        .arg(&wasm_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn krate with prompt");

    child
        .stdin
        .as_mut()
        .expect("child stdin")
        .write_all(b"a\n")
        .expect("write prompt response");

    let output = child.wait_with_output().expect("wait for prompt run");

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("This app is asking to:"));
    // The plain phrase leads, with the exact capability alongside it.
    assert!(stderr.contains("read files in data"));
    assert!(stderr.contains("fs.read:data/**"));
    assert!(stderr.contains("Read data"));
    assert!(stderr.contains("invalid wasm component"));
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut hex, "{byte:02x}").expect("write to string");
    }
    hex
}

fn workspace_path(path: PathBuf) -> PathBuf {
    if path.is_absolute() || path.exists() {
        return path;
    }

    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(path)
}

fn sample_manifest(app: &str) -> PathBuf {
    workspace_path(PathBuf::from(format!("apps/{app}/manifest.toml")))
}

/// Concurrent builds of the same app name must not spoil each
/// other's artifact.
///
/// The shared build cache exists so every app does not recompile the SDK,
/// and it worked -- but it gave every app with the same crate name the same
/// output FILENAME. Cargo's lock serializes the builds and is released
/// between them, so a second build overwrote the first's wasm in the window
/// before it was copied home. What came out was half-written, and the two
/// failures a half-written wasm causes are exactly what CI reported:
///
///   error: ... code.wasm is not a WebAssembly component
///   error: the build produced no dashboard.wasm in ...
///
/// Measured before the fix, six concurrent runs: three exited 6 as they
/// should and three failed. After: ten concurrent runs, all 6.
///
/// Six, not two. Two was tried first and did NOT reproduce it: the
/// sabotage run -- the fix reverted, one shared artifact name again --
/// passed three times running with two builds. The window is small, and a
/// test that catches a race only sometimes is worse than none, because a
/// green run stops meaning anything. Six is the number that failed
/// reliably when this was first reproduced by hand.
#[test]
fn concurrent_builds_of_one_app_name_do_not_spoil_each_other() {
    if !has_cargo_component() {
        eprintln!("skipping: cargo-component not installed");
        return;
    }
    let _build_lock = cargo_build_guard();

    let work = tempfile::tempdir().expect("temp dir");
    let mut kids = Vec::new();
    for i in 0..6 {
        let out = work.path().join(format!("out{i}.krate"));
        let inspect = work.path().join(format!("inspect{i}"));
        kids.push(
            krate()
                .arg("create")
                .arg("a small dashboard")
                .args(["--author-cmd", "true"])
                .arg("--output")
                .arg(&out)
                .arg("--work-dir")
                .arg(&inspect)
                .spawn()
                .expect("spawn create"),
        );
    }
    for (i, kid) in kids.into_iter().enumerate() {
        let done = kid.wait_with_output().expect("wait for create");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&done.stdout),
            String::from_utf8_lossy(&done.stderr)
        );
        // The artifact must never be the half-written one. These two
        // sentences are what a spoiled wasm produces, and neither may
        // appear however the run ends.
        assert!(
            !text.contains("is not a WebAssembly component"),
            "build {i} packed a spoiled artifact: {text}"
        );
        assert!(
            !text.contains("the build produced no"),
            "build {i} lost its artifact to the other build: {text}"
        );
        // And the run itself still reaches its honest verdict: a no-op
        // author leaves the starter unchanged, which is exit 6.
        assert_eq!(
            done.status.code(),
            Some(6),
            "build {i} did not reach the starter verdict: {text}"
        );
    }
}

/// A folder nothing can be saved into is refused before any AI time is
/// spent (K-904). It used to fail at the pack step, after the whole build.
#[cfg(unix)]
#[test]
fn an_unwritable_output_folder_is_refused_before_authoring() {
    use std::os::unix::fs::PermissionsExt;
    let work = tempfile::tempdir().expect("temp dir");
    let locked = work.path().join("locked");
    std::fs::create_dir(&locked).expect("dir");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).expect("lock");
    let marker = work.path().join("authored");
    let done = krate()
        .arg("create")
        .args(["--author-cmd", &format!("touch {}", marker.display())])
        .arg("--output")
        .arg(locked.join("app.krate"))
        .arg("--")
        .arg("a small dashboard")
        .output()
        .expect("run create");
    let _ = std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755));
    let stderr = String::from_utf8_lossy(&done.stderr);
    assert!(!done.status.success());
    assert!(stderr.contains("cannot write to"), "{stderr}");
    assert!(
        !marker.exists(),
        "no authoring started for an app that could not be saved"
    );
}

/// An attachment rides the ordinary create (K-893).
///
/// Attached files went down a separate path that staged them in one inbox
/// shared by every app, outside the workspace the agent works in, ignored
/// --work-dir, and turned the request verdict into "the app could not be
/// built" (exit 1) -- while the judge read Krate's own "The person attached
/// these files..." as things the person asked for. A no-op author leaves
/// the starter unchanged, whose honest verdict is exit 6: that, with the
/// file inside this app's workspace and no attachment prose in the verdict.
#[test]
fn an_attachment_goes_into_the_apps_own_workspace_and_is_not_judged() {
    if !has_cargo_component() {
        eprintln!("skipping: cargo-component not installed");
        return;
    }
    let _build_lock = cargo_build_guard();
    let work = tempfile::tempdir().expect("temp dir");
    let items = work.path().join("items.csv");
    std::fs::write(&items, "milk\neggs\n").expect("write attachment");
    let out = work.path().join("out.krate");
    let inspect = work.path().join("inspect");
    let done = krate()
        .arg("create")
        .args(["--author-cmd", "true"])
        .arg("--output")
        .arg(&out)
        .arg("--work-dir")
        .arg(&inspect)
        .arg("--attach")
        .arg(&items)
        .arg("--")
        .arg("a small dashboard")
        .output()
        .expect("run create");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&done.stdout),
        String::from_utf8_lossy(&done.stderr)
    );
    assert_eq!(
        done.status.code(),
        Some(6),
        "the starter verdict, not an error: {text}"
    );
    assert!(
        inspect
            .join("dashboard")
            .join("attached")
            .join("items.csv")
            .is_file(),
        "the file is staged inside this app's workspace: {text}"
    );
    assert!(
        !text.contains("attached, these") && !text.contains("person, attached"),
        "Krate's attachment prose is not judged as the request: {text}"
    );
}

#[test]
fn create_with_an_agent_seam_builds_the_skeleton_and_refuses_to_call_it_authored() {
    // The agent path drops a minimal skeleton + KRATE_AUTHORING.md, then builds
    // it. Drive it with a no-op author command (`true`), so this exercises the
    // scaffolding and the full build/pack/verify pipeline on the blank
    // skeleton -- without needing an AI. The skeleton must be a valid app on its
    // own, or an agent that starts from it starts from a broken base.
    //
    // And because the author command did nothing, the app that comes out is
    // the untouched starter. It builds, packs and runs, and it is not the
    // dashboard that was asked for -- so the pipeline must say so instead of
    // reporting an authored app. Skipped where the build toolchain is absent.
    if !has_cargo_component() {
        eprintln!("skipping: cargo-component not installed");
        return;
    }
    let _build_lock = cargo_build_guard();
    let work = tempfile::tempdir().expect("temp dir");
    let out = work.path().join("skel.krate");
    let inspect = work.path().join("inspect");
    // A GUI-leaning request, so the GUI skeleton is chosen.
    let output = krate()
        .arg("create")
        .arg("a small dashboard")
        .arg("--author-cmd")
        .arg("true")
        .arg("--output")
        .arg(&out)
        .arg("--work-dir")
        .arg(&inspect)
        .output()
        .expect("run create");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    // The scaffolding and the whole build/pack/verify pipeline must work on
    // the blank skeleton -- and the result must NOT be reported as an authored
    // app. A no-op author command leaves the starter untouched, so the app
    // that comes out says "Replace" and has nothing to do with a dashboard.
    // That is exit 6: it built, and it is not what was asked for. Reporting
    // this as success is the false positive the request verdict exists to
    // stop, so the test pins the refusal rather than the old exit 0.
    assert_eq!(
        output.status.code(),
        Some(6),
        "a no-op author command leaves the starter unchanged, which must not \
         pass as an authored app: {stderr}{stdout}"
    );
    assert!(
        stdout.contains("not what you asked for"),
        "the person must be told the app does not serve the request: {stdout}"
    );
    assert!(
        out.is_file(),
        "the .krate is still written -- it built, it is just not the app asked for"
    );
    // The work dir holds exactly one app directory; find it rather than
    // predicting the name-derivation. The pack and a real skeleton lib.rs were
    // dropped for the agent.
    let app_dir = std::fs::read_dir(&inspect)
        .expect("read work dir")
        .flatten()
        .map(|e| e.path())
        .find(|p| p.is_dir())
        .expect("one app dir under the work dir");
    assert!(
        app_dir.join("KRATE_AUTHORING.md").is_file(),
        "the context pack is dropped beside the skeleton"
    );
    let lib = std::fs::read_to_string(app_dir.join("src/lib.rs")).expect("skeleton lib.rs");
    assert!(
        lib.contains("Replace"),
        "the skeleton is a blank to fill in"
    );
}

#[test]
fn authoring_context_writes_a_pack_with_every_section() {
    // No build tools needed: the pack is generated from embedded sources and
    // the repo's apps tree. Fast, and it guards the subcommand end to end.
    let out_dir = tempfile::tempdir().expect("temp dir");
    let out_file = out_dir.path().join("KRATE_AUTHORING.md");
    // Point it at the workspace root so its apps/ tree seeds the example index.
    let app_dir = workspace_path(PathBuf::from("apps/krate-diceroll"));
    let status = krate()
        .arg("authoring-context")
        .arg(&app_dir)
        .arg("--output")
        .arg(&out_file)
        .status()
        .expect("run authoring-context");
    assert!(status.success(), "authoring-context should succeed");
    let pack = std::fs::read_to_string(&out_file).expect("pack written");
    for needle in [
        "# 1. The SDK",
        "# 2. Capabilities",
        "# 3. Passing the import check",
        "# 4. The GUI world",
        "# 5. A complete worked example",
        "canvas2d::present",
        "random.bytes",
        "krate-notes",
        // The example is inlined rather than pointed at, so that a machine
        // without the repo has real code to copy instead of a path to hunt
        // for. Losing this would restore the eight-minute filesystem search.
        "#![no_std]",
        "### `manifest.toml`",
    ] {
        assert!(pack.contains(needle), "pack should contain {needle:?}");
    }
}

#[test]
fn check_app_reports_a_missing_layout_without_building() {
    // No build tools needed: check-app must fail fast, and clearly, when the
    // directory is not an app. This is the first thing an agent hits if it
    // points check-app at the wrong place.
    let empty = tempfile::tempdir().expect("temp dir");
    let output = krate()
        .arg("check-app")
        .arg(empty.path())
        .output()
        .expect("run check-app");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "check-app on an empty dir must fail: {stderr}"
    );
    // Exit 10 is the layout stage. Distinct code so an agent can branch.
    assert_eq!(output.status.code(), Some(10), "layout stage exit code");
    assert!(
        stderr.contains("not an app directory") && stderr.contains("Cargo.toml"),
        "should name what is missing: {stderr}"
    );
}

#[test]
fn check_app_passes_a_known_good_app_and_emits_json() {
    // The oracle's happy path against a real CLI app that runs clean headless
    // with no arguments. Builds it, checks krate:*-only imports, and runs it --
    // the same guarantees a successful `create` gives. krate-diceroll also pulls
    // a real getrandom-dependent crate (rand) through the SDK backend, so a pass
    // here doubles as a guard that ordinary dependencies still resolve to a
    // 0-wasi component. Skipped where the build toolchain is absent, rather than
    // weakened.
    if !has_cargo_component() {
        eprintln!("skipping: cargo-component not installed");
        return;
    }
    let _build_lock = cargo_build_guard();
    let app_dir = workspace_path(PathBuf::from("apps/krate-diceroll"));
    let output = krate()
        .arg("check-app")
        .arg(&app_dir)
        .arg("--json")
        .output()
        .expect("run check-app");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "check-app on krate-cat should pass.\nstdout: {stdout}\nstderr: {stderr}"
    );
    let value: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("check-app --json emits one JSON object");
    assert_eq!(value["ok"], serde_json::Value::Bool(true));
    // Every stage a CLI app goes through, and only krate:* imports.
    let stages = value["stages"].as_array().expect("stages array");
    assert!(stages.iter().any(|s| s == "build"));
    assert!(stages.iter().any(|s| s == "imports"));
    assert!(stages.iter().any(|s| s == "run"));
    let imports = value["imports"].as_array().expect("imports array");
    assert!(
        imports
            .iter()
            .all(|i| i.as_str().unwrap().starts_with("krate:")),
        "a passing app imports only krate:*: {imports:?}"
    );
    // A build that rewrites source says so in the report (IC-295). This
    // app's lock is committed and current, so a warm build changes nothing
    // -- the field is there, and empty, which is the honest answer.
    let changes = value["source_changes"]
        .as_array()
        .expect("source_changes is reported, empty or not");
    assert!(
        changes
            .iter()
            .all(|c| !c.as_str().unwrap_or("").starts_with("Cargo.lock: updated")),
        "a current lock must not be rewritten by a check: {changes:?}"
    );
}

#[test]
fn check_app_passes_a_cli_app_that_needs_an_argument() {
    // Regression: a CLI app that requires an argument must pass check-app. It
    // used to fail at the run stage because check-app gave CLI apps no argument
    // at all -- so the app printed its usage and exited non-zero, and check-app
    // called a correct app broken. krate-cat reads a file argument; check-app
    // must seed a fixture and pass its path (the same thing create verifies), so
    // the app does its work once and exits 0.
    if !has_cargo_component() {
        eprintln!("skipping: cargo-component not installed");
        return;
    }
    let _build_lock = cargo_build_guard();
    let app_dir = workspace_path(PathBuf::from("apps/krate-cat"));
    let output = krate()
        .arg("check-app")
        .arg(&app_dir)
        .output()
        .expect("run check-app");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "check-app must pass a CLI app that takes an argument: {stderr}"
    );
}

/// A run report names the artifact it is about (IC-717).
///
/// `run --json` carried the app id and version and no identity at all, while
/// the capability dump exposed one. Two bundles can declare the same id and
/// version, so "this app passed" meant "some file called that passed".
///
/// The three identities answer three different questions and this pins all
/// of them, plus the rule that the project identity is absent -- not
/// duplicated from the execution one -- when there is nothing extra to
/// rebuild.
#[test]
fn a_run_report_names_the_exact_artifact_it_ran() {
    // The clock component, because it is built against the CURRENT world.
    // The hello fixture is Phase 1 and cannot instantiate, which would fail
    // this test for a reason that has nothing to do with identities.
    let Some(component) = configured_krate_clock_component() else {
        eprintln!("skipping: KRATE_CLOCK_WASM is not set (no krate-clock component configured)");
        return;
    };
    let dir = tempfile::tempdir().expect("temp dir");
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"dev.krate.identity\"\nname = \"Identity\"\nversion = \"1.0.0\"\n\
         entry = \"code.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n\n\
         [[capabilities]]\ncap = \"time.clock\"\nrationale = \"tell the time\"\n\
         required = true\n",
    )
    .expect("write manifest");

    let bundle = dir.path().join("app.krate");
    let packed = krate()
        .args(["pack"])
        .arg(&component)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--output")
        .arg(&bundle)
        .output()
        .expect("run pack");
    assert!(
        packed.status.success(),
        "pack: {}",
        String::from_utf8_lossy(&packed.stderr)
    );

    let output = krate()
        .arg("run")
        .arg(&bundle)
        .arg("--json")
        .output()
        .expect("run --json");
    let payload: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("the report is one JSON object");

    assert_eq!(payload["schema"], "krate.run.v1");
    let identity = &payload["identity"];
    for layer in ["archive", "execution"] {
        let value = identity[layer]
            .as_str()
            .unwrap_or_else(|| panic!("the report must carry the {layer} identity: {payload}"));
        assert_eq!(value.len(), 64, "{layer} must be a full sha256");
    }
    assert_ne!(
        identity["archive"], identity["execution"],
        "the file's bytes and what runs are different questions and must not \
         be answered with one number",
    );
    assert!(
        identity["project"].is_null(),
        "a bundle with no source has nothing extra to rebuild, and inventing \
         a second identity for it would be a claim about nothing: {payload}",
    );

    // What ran it, not only what ran: a report that cannot say which runtime
    // and platform produced it cannot be compared across machines.
    assert!(
        payload["runtime"]["version"].is_string(),
        "the report must name the runtime that produced it: {payload}"
    );
    // And which rules judged the component valid before it ran, against
    // which WIT (IC-210): "valid" without "against what" is not a claim.
    assert_eq!(
        payload["runtime"]["validator"]["version"], 1,
        "the report must name the validator version: {payload}"
    );
    assert_eq!(
        payload["runtime"]["validator"]["wit"]
            .as_str()
            .map(str::len),
        Some(64),
        "the report must name the WIT the validator was built from: {payload}"
    );
    assert!(
        payload["runtime"]["platform"]
            .as_str()
            .is_some_and(|p| p.contains('-')),
        "platform must be arch-os: {payload}"
    );
}

/// A signed app that was changed afterwards does not run (IC-015).
///
/// The whole point of a signature: the publisher signed one file and this is
/// another, so running it would execute something nobody vouched for while a
/// signature sits inside implying somebody did. Exit 5 -- the permission
/// wall's code -- because this is the product refusing on purpose.
///
/// The other half matters just as much: an UNSIGNED app must be completely
/// unaffected. Almost every app today is unsigned, and refusing those would
/// break the product to enforce a promise nobody has made yet.
#[test]
fn a_signed_app_that_was_changed_afterwards_is_refused() {
    let Some(component) = configured_krate_clock_component() else {
        eprintln!("skipping: KRATE_CLOCK_WASM is not set (no krate-clock component configured)");
        return;
    };
    let dir = tempfile::tempdir().expect("temp dir");
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"dev.krate.signed\"\nname = \"Signed\"\nversion = \"1.0.0\"\n\
         entry = \"code.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n\n\
         [[capabilities]]\ncap = \"time.clock\"\nrationale = \"tell the time\"\n\
         required = true\n",
    )
    .expect("write manifest");

    let bundle = dir.path().join("app.krate");
    assert!(
        krate()
            .args(["pack"])
            .arg(&component)
            .arg("--manifest")
            .arg(&manifest)
            .arg("--output")
            .arg(&bundle)
            .output()
            .expect("pack")
            .status
            .success(),
        "pack must succeed"
    );

    // Unsigned: runs exactly as before. Asserted BEFORE signing, so a
    // regression here cannot hide behind the signing path.
    let unsigned = krate()
        .arg("run")
        .arg(&bundle)
        .args(["--headless", "--auto-grant"])
        .output()
        .expect("run unsigned");
    assert_eq!(
        unsigned.status.code(),
        Some(0),
        "an unsigned app must be untouched by signature enforcement: {}",
        String::from_utf8_lossy(&unsigned.stderr)
    );

    let key = dir.path().join("publisher.key");
    let signed = krate()
        .args(["sign"])
        .arg(&bundle)
        .arg("--key")
        .arg(&key)
        .arg("--generate-key")
        .args(["--namespace", "acme/signed"])
        .output()
        .expect("sign");
    assert!(
        signed.status.success(),
        "sign: {}",
        String::from_utf8_lossy(&signed.stderr)
    );

    // Signed and untouched: still runs.
    assert_eq!(
        krate()
            .arg("run")
            .arg(&bundle)
            .args(["--headless", "--auto-grant"])
            .output()
            .expect("run signed")
            .status
            .code(),
        Some(0),
        "signing an app must not stop it running",
    );

    // Now change one file inside the signed bundle.
    let tampered = dir.path().join("tampered.krate");
    rewrite_bundle_entry(&bundle, &tampered, "manifest.toml", |bytes| {
        let mut text = String::from_utf8_lossy(bytes).into_owned();
        text.push_str("\n# added after signing\n");
        text.into_bytes()
    });

    let refused = krate()
        .arg("run")
        .arg(&tampered)
        .args(["--headless", "--auto-grant"])
        .output()
        .expect("run tampered");
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert_eq!(
        refused.status.code(),
        Some(5),
        "a changed signed app must be refused with the wall's own code: {stderr}",
    );
    assert!(
        stderr.contains("changed after it was signed"),
        "the refusal must say what happened: {stderr}"
    );
    assert!(
        stderr.contains("manifest.toml"),
        "and name the file that changed: {stderr}"
    );
}

/// Copy a bundle, transforming one entry.
fn rewrite_bundle_entry(
    source: &std::path::Path,
    destination: &std::path::Path,
    entry: &str,
    change: impl Fn(&[u8]) -> Vec<u8>,
) {
    use std::io::{Read, Write};
    let data = std::fs::read(source).expect("read bundle");
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&data)).expect("open zip");
    let file = std::fs::File::create(destination).expect("create");
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for index in 0..archive.len() {
        let mut existing = archive.by_index(index).expect("entry");
        let name = existing.name().to_string();
        let mut bytes = Vec::new();
        existing.read_to_end(&mut bytes).expect("read entry");
        writer.start_file(name.clone(), options).expect("start");
        let payload = if name == entry { change(&bytes) } else { bytes };
        writer.write_all(&payload).expect("write");
    }
    writer.finish().expect("finish");
}

/// Adversarial archives, through the CLI a person actually runs (IC-714).
///
/// The register's point is exact: the SOURCE contained duplicate-asset
/// rejection while the shipped debug, release and public binaries accepted
/// the archive, and no test caught the mismatch. A library test cannot --
/// it exercises the code, not the binary. This runs the fixtures through
/// `krate run`, which is what a recipient does.
#[test]
fn adversarial_archives_are_refused_by_the_binary_people_run() {
    let dir = tempfile::tempdir().expect("temp dir");

    for (what, path) in [
        ("a duplicated core record", "manifest.toml"),
        ("a duplicated component", "code.wasm"),
        ("a duplicated asset", "assets/logo.png"),
        ("a duplicated source file", "source/lib.rs"),
        ("a duplicated SDK file", "sdk/krate.wit"),
        ("a duplicated unknown record", "future/thing.bin"),
    ] {
        let bundle = dir.path().join("adversarial.krate");
        std::fs::write(&bundle, archive_naming_one_path_twice(path)).expect("write fixture");

        let output = krate()
            .arg("run")
            .arg(&bundle)
            .args(["--headless", "--auto-grant"])
            .output()
            .expect("run the adversarial bundle");

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_ne!(
            output.status.code(),
            Some(0),
            "{what} must not open: {stderr}"
        );
        assert!(
            stderr.contains("names the same file twice"),
            "{what} must be refused as a duplicate, not something else: {stderr}"
        );
        assert!(
            stderr.contains(&path.to_lowercase()),
            "{what} must NAME the path {path}: {stderr}"
        );
    }
}

/// A path that will not survive being unpacked is refused before it is
/// written (K-257).
///
/// A source tree two hundred directories deep opened and extracted on macOS
/// and would have crossed Windows' path length limit part way through, which
/// surfaces as an I/O error naming a path nobody typed rather than a refusal
/// that says what is wrong.
#[test]
fn a_path_that_cannot_be_unpacked_everywhere_is_refused_by_the_binary_people_run() {
    let dir = tempfile::tempdir().expect("temp dir");

    let deep = format!(
        "source/{}/lib.rs",
        (0..200)
            .map(|i| format!("d{i}"))
            .collect::<Vec<_>>()
            .join("/")
    );
    let bundle = dir.path().join("deep.krate");
    std::fs::write(
        &bundle,
        archive_carrying(&[(deep, b"fn main() {}".to_vec())]),
    )
    .expect("write fixture");

    let output = krate()
        .arg("run")
        .arg(&bundle)
        .args(["--headless", "--auto-grant"])
        .output()
        .expect("run the deep bundle");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_ne!(
        output.status.code(),
        Some(0),
        "a path that deep must not open: {stderr}"
    );
    assert!(
        stderr.contains("nests deeper than"),
        "it must be refused for its depth, not something else: {stderr}"
    );
    assert!(
        stderr.contains("Flatten it"),
        "the refusal must say what to do about it: {stderr}"
    );
}

/// Telling a sender to "just send it" is only true for a receiver who
/// already has Krate (K-195).
///
/// To anyone else a .krate is an unclaimed extension: the system offers
/// "Choose Application" or the App Store, and neither finds Krate. The whole
/// promise is "make an app you can actually send someone", so the sentence
/// that ends the build has to be true for the person receiving it, and name
/// the way through for the one who has nothing installed.
#[test]
fn the_send_advice_does_not_promise_a_double_click_to_someone_without_krate() {
    // The advice lives in the source of the build path, so it is read from
    // there rather than by running a multi-minute build.
    let source = include_str!("../src/main.rs");
    let body: String = source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        !body.contains("to someone; they can double-click it to open it"),
        "the unconditional promise is back: a receiver without Krate gets a \
         dead file, and this sentence tells the sender otherwise"
    );
    assert!(
        body.contains("to someone who has Krate"),
        "the advice must say WHO can double-click it"
    );
    assert!(
        body.contains("krate card"),
        "the advice must name the way through for a receiver without Krate"
    );
}

/// The commit this binary reports must be the one it was built from (K-276).
///
/// One line in build.rs watched `.git/HEAD`, which on a branch holds a ref
/// rather than a SHA and does not change when a commit lands. Cargo
/// therefore never re-ran the script, and the binary reported a commit
/// fourteen behind its own source -- plausibly wrong, which is worse than
/// obviously wrong, because a bug report quotes it and the reader goes
/// looking in the wrong code.
///
/// This runs against whatever git state the checkout is in, so it asserts
/// the two things that hold in every state: the stamp names a commit that
/// exists, and it says when the source was edited.
#[test]
fn the_reported_commit_is_the_one_this_binary_was_built_from() {
    let output = krate().arg("version").output().expect("run version");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let reported = stdout
        .lines()
        .find_map(|line| line.strip_prefix("commit"))
        .map(str::trim)
        .expect("version must report a commit");

    if reported == "unknown" {
        // A build with no git at all -- a published crate, a tarball. Saying
        // "unknown" is the honest answer there.
        eprintln!("skipping the commit-stamp check: this binary was built without git");
        return;
    }

    let (sha, dirty) = match reported.strip_suffix("-dirty") {
        Some(sha) => (sha, true),
        None => (reported, false),
    };

    // The stamp must name a real commit. A stale stamp names one too, so
    // this alone is not enough -- see below.
    let exists = std::process::Command::new("git")
        .args(["cat-file", "-e", &format!("{sha}^{{commit}}")])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    assert!(
        exists,
        "the reported commit {sha} is not a commit in this repo"
    );

    // The part that catches staleness: with a clean tree the stamp must be
    // HEAD itself. With a dirty tree it may be HEAD and must say dirty,
    // because the source that produced it is not the source that commit
    // names.
    //
    // The question is the one build.rs asked, verbatim -- it leaves out the
    // wit-bindgen output that a workspace build rewrites, because that is
    // exactly what dirtied CI's tree between the stamp and this test.
    let query: Vec<&str> = env!("KRATE_DIRTY_TREE_QUERY").split('\t').collect();
    let tracked_changes = std::process::Command::new("git")
        .args(&query)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("git status");
    let changed = String::from_utf8_lossy(&tracked_changes.stdout)
        .trim()
        .to_owned();
    let tree_is_dirty = !changed.is_empty();

    assert_eq!(
        dirty, tree_is_dirty,
        "the stamp says dirty={dirty} and the tracked files say {tree_is_dirty}; \
         git status shows:\n{changed}"
    );

    if !tree_is_dirty {
        let head = std::process::Command::new("git")
            .args(["rev-parse", "--short=12", "HEAD"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .expect("git rev-parse");
        let head = String::from_utf8_lossy(&head.stdout).trim().to_string();
        assert_eq!(
            sha, head,
            "a binary built from a clean tree must report HEAD, not an \
             older commit -- that is exactly the staleness this checks for"
        );
    }
}

/// Publishing must not upload what Krate would refuse to open (K-273).
///
/// Publish is the moment a file stops being one person's problem. It used to
/// open the bundle only to read its name, with `.ok()`, so a malformed
/// archive was treated as one that merely had no name and went to the hub
/// anyway. An archive whose reviewed copy is not the copy that runs is
/// exactly the one that must never be published.
///
/// The hub is pointed at a dead port throughout, so a failure to refuse
/// shows up as a CONNECTION error -- proof the bundle got as far as the
/// network -- rather than as anything leaving this machine.
#[test]
fn publishing_refuses_a_bundle_that_cannot_be_opened() {
    let dir = tempfile::tempdir().expect("temp dir");

    // Names one file twice: what a reviewer reads is not what runs.
    let duplicate = dir.path().join("duplicate.krate");
    std::fs::write(&duplicate, archive_naming_one_path_twice("source/lib.rs"))
        .expect("write fixture");

    let output = krate()
        .arg("publish")
        .arg(&duplicate)
        .env("KRATE_HUB_URL", "http://127.0.0.1:1")
        .output()
        .expect("run publish");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_ne!(
        output.status.code(),
        Some(0),
        "a malformed bundle must not publish: {stderr}"
    );
    assert!(
        stderr.contains("will not be published"),
        "the refusal must say the publish did not happen: {stderr}"
    );
    assert!(
        stderr.contains("names the same file twice"),
        "and why, in the words the opener already uses: {stderr}"
    );
    assert!(
        !stderr.contains("could not reach the hub"),
        "it must be refused BEFORE the network is touched -- reaching the \
         hub means a real hub would have taken it: {stderr}"
    );

    // A component the validator refuses (IC-210): well formed as an archive,
    // instantiates as a component, and is still not a Krate app. Publish
    // must say so in the validator's words, and must not have reached out.
    let chatty = dir.path().join("chatty.krate");
    std::fs::write(
        &chatty,
        archive_with_component(
            include_bytes!("../../bundle/tests/fixtures/extra-export.wasm"),
            &[],
        ),
    )
    .expect("write fixture");
    let output = krate()
        .arg("publish")
        .arg(&chatty)
        .env("KRATE_HUB_URL", "http://127.0.0.1:1")
        .output()
        .expect("run publish");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_ne!(
        output.status.code(),
        Some(0),
        "an invalid component must not publish: {stderr}"
    );
    assert!(
        stderr.contains("will not be published") && stderr.contains("exports more than `run`"),
        "the refusal must carry the validator's reason: {stderr}"
    );
    assert!(
        !stderr.contains("could not reach the hub"),
        "and happen before the network: {stderr}"
    );
}

/// A signature that was damaged after signing is said out loud (K-258).
///
/// Storage already treats it as unsigned, which is the safe handling. What
/// was missing is telling the person: "unsigned" is ordinary and gets a
/// shrug, while "signed, then changed" is a reason to get a fresh copy.
#[test]
fn a_damaged_signature_is_reported_and_an_unsigned_one_is_not() {
    let dir = tempfile::tempdir().expect("temp dir");

    // Unsigned: ordinary, and must not warn.
    let unsigned = dir.path().join("unsigned.krate");
    std::fs::write(&unsigned, archive_carrying(&[])).expect("write fixture");
    let output = krate()
        .arg("run")
        .arg(&unsigned)
        .args(["--headless", "--auto-grant"])
        .output()
        .expect("run the unsigned bundle");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("signature that cannot be read"),
        "an unsigned bundle must not be warned about: {stderr}"
    );

    // Signed once, then the signature replaced with something that is not
    // one. This is the bundle that used to be indistinguishable from the
    // bundle above.
    let damaged = dir.path().join("damaged.krate");
    std::fs::write(
        &damaged,
        archive_carrying(&[(
            "signature.json".to_string(),
            br#"{"corrupted":true}"#.to_vec(),
        )]),
    )
    .expect("write fixture");
    let output = krate()
        .arg("run")
        .arg(&damaged)
        .args(["--headless", "--auto-grant"])
        .output()
        .expect("run the damaged bundle");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("signature that cannot be read"),
        "a damaged signature must be reported to the person: {stderr}"
    );
    assert!(
        stderr.contains("get a fresh copy"),
        "the warning must say what to do about it: {stderr}"
    );
}

/// A file whose name is not ASCII cannot be pinned down (IC-209).
///
/// `caf\u{e9}.rs` written NFC and NFD is two different byte strings and one
/// file on macOS, so a reviewed copy and a substituted one are
/// indistinguishable. `krate run` must say so, name the file, and stop.
#[test]
fn a_path_outside_ascii_is_refused_by_the_binary_people_run() {
    let dir = tempfile::tempdir().expect("temp dir");

    for (what, path) in [
        ("the composed spelling", "source/caf\u{e9}.rs"),
        ("the decomposed spelling", "source/cafe\u{301}.rs"),
    ] {
        let bundle = dir.path().join("unicode.krate");
        std::fs::write(
            &bundle,
            archive_carrying(&[(path.to_string(), b"fn main() {}".to_vec())]),
        )
        .expect("write fixture");

        let output = krate()
            .arg("run")
            .arg(&bundle)
            .args(["--headless", "--auto-grant"])
            .output()
            .expect("run the non-ASCII bundle");

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_ne!(
            output.status.code(),
            Some(0),
            "{what} must not open: {stderr}"
        );
        assert!(
            stderr.contains("outside ASCII"),
            "{what} must be refused for its name, not something else: {stderr}"
        );
        assert!(
            stderr.contains(path),
            "{what} must NAME the path {path:?}: {stderr}"
        );
        assert!(
            stderr.contains("Rename it to ASCII"),
            "{what} must say what to do about it: {stderr}"
        );
    }
}

/// A loose component that fits no Krate world is refused by inspection,
/// before anything runs, with the import it asked for named (IC-231).
///
/// This is the manifestless developer path -- `krate run app.wasm` -- where
/// the runtime used to pick a world by instantiating in each until one did
/// not error, and then reported whatever the LAST world happened to say.
#[test]
fn a_loose_component_that_fits_no_world_is_refused_before_it_runs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wasm = dir.path().join("app.wasm");
    std::fs::write(
        &wasm,
        include_bytes!("../../bundle/tests/fixtures/fits-no-world.wasm"),
    )
    .expect("write component");

    let output = krate()
        .arg("run")
        .arg(&wasm)
        .args(["--headless", "--auto-grant"])
        .output()
        .expect("run the loose component");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(2),
        "not an app this Krate can run: {stderr}"
    );
    assert!(
        stderr.contains("fits no Krate world") && stderr.contains("krate:time/clock@9.9.9"),
        "the refusal must say which import no world provides: {stderr}"
    );

    let output = krate()
        .args(["run", "--json"])
        .arg(&wasm)
        .output()
        .expect("run --json");
    let payload: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("the report is one JSON object");
    assert_eq!(payload["exit"]["class"], "no-matching-world", "{payload}");
    assert!(
        payload["exit"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("krate:time/clock@9.9.9")),
        "{payload}"
    );
}

/// A publisher withdraws a release key, and every machine that imports the
/// signed list refuses what that key signed after the theft -- and nothing
/// it signed before (IC-015, revocation).
///
/// Driven entirely through the CLI, the way a publisher and a recipient
/// would do it: the root delegates a release key, the release key signs,
/// the root withdraws it into a signed list, the recipient imports the
/// list. The list is honoured only from the root that signed it, an older
/// list never replaces a newer one, and an edited list is not a list.
#[test]
fn a_withdrawn_key_is_refused_only_for_what_it_signed_after_the_compromise() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = tempfile::tempdir().expect("home");
    let write = |name: &str, bytes: &[u8]| {
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).expect("write");
        path
    };
    let root = write(
        "root.pkcs8",
        &krate_bundle::signing::SigningKey::generate_pkcs8().expect("root"),
    );
    let release = write(
        "release.pkcs8",
        &krate_bundle::signing::SigningKey::generate_pkcs8().expect("release"),
    );
    let manifest = write("manifest.toml", BUNDLE_MANIFEST.as_bytes());
    let wasm = write(
        "code.wasm",
        include_bytes!("../../bundle/tests/fixtures/minimal-run.wasm"),
    );
    let bundle = dir.path().join("app.krate");
    let ok = |output: std::process::Output, what: &str| {
        assert!(
            output.status.success(),
            "{what} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    };
    ok(
        krate()
            .args(["pack"])
            .arg(&wasm)
            .arg("--manifest")
            .arg(&manifest)
            .arg("-o")
            .arg(&bundle)
            .output()
            .expect("pack"),
        "pack",
    );

    // The root authorises the release key, and the release key signs.
    let delegation = dir.path().join("delegation.json");
    ok(
        krate()
            .args(["delegate", "--namespace", "acme/*", "--days", "30"])
            .arg("--root")
            .arg(&root)
            .arg("--key")
            .arg(&release)
            .arg("-o")
            .arg(&delegation)
            .output()
            .expect("delegate"),
        "delegate",
    );
    ok(
        krate()
            .arg("sign")
            .arg(&bundle)
            .arg("--key")
            .arg(&release)
            .args(["--namespace", "acme/demo"])
            .arg("--delegation")
            .arg(&delegation)
            .output()
            .expect("sign"),
        "sign with a delegation",
    );
    let signed_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs();

    let run = || {
        krate()
            .arg("run")
            .arg(&bundle)
            .args(["--headless", "--auto-grant"])
            .env("HOME", home.path())
            .output()
            .expect("run")
    };
    // No list on this machine: the release runs, its revocation state unknown.
    let before = run();
    assert_eq!(
        before.status.code(),
        Some(0),
        "with no list held, a delegated release runs: {}",
        String::from_utf8_lossy(&before.stderr)
    );

    // The publisher withdraws the key as of an hour BEFORE it signed.
    let stolen = dir.path().join("revocations.json");
    ok(
        krate()
            .arg("revoke")
            .arg("--root")
            .arg(&root)
            .arg("--key")
            .arg(&release)
            .args(["--compromised-from", &(signed_at - 3600).to_string()])
            .args(["--reason", "laptop stolen"])
            .arg("-o")
            .arg(&stolen)
            .output()
            .expect("revoke"),
        "revoke",
    );
    ok(
        krate()
            .args(["revocations", "--import"])
            .arg(&stolen)
            .env("HOME", home.path())
            .output()
            .expect("import"),
        "import",
    );
    let shown = ok(
        krate()
            .arg("revocations")
            .env("HOME", home.path())
            .output()
            .expect("show"),
        "revocations",
    );
    let listing = String::from_utf8_lossy(&shown.stdout);
    assert!(
        listing.contains("1 withdrawn key") && listing.contains("laptop stolen"),
        "the machine must show what it holds: {listing}"
    );

    let refused = run();
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert_eq!(
        refused.status.code(),
        Some(5),
        "a release signed after the theft is refused: {stderr}"
    );
    assert!(
        stderr.contains("withdrawn") && stderr.contains("laptop stolen"),
        "the refusal names the withdrawal and the publisher's reason: {stderr}"
    );

    // A newer list that puts the compromise AFTER the signing: the release
    // was made honestly and runs again.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let later = dir.path().join("later.json");
    ok(
        krate()
            .arg("revoke")
            .arg("--root")
            .arg(&root)
            .arg("--key")
            .arg(&release)
            .args(["--compromised-from", &(signed_at + 3600).to_string()])
            .args(["--reason", "retired"])
            .arg("-o")
            .arg(&later)
            .output()
            .expect("revoke later"),
        "revoke (later)",
    );
    ok(
        krate()
            .args(["revocations", "--import"])
            .arg(&later)
            .env("HOME", home.path())
            .output()
            .expect("import later"),
        "import (later)",
    );
    let honest = run();
    assert_eq!(
        honest.status.code(),
        Some(0),
        "a release signed before the compromise still runs: {}",
        String::from_utf8_lossy(&honest.stderr)
    );

    // Rolling back to the older list would forget nothing here but would in
    // general: refused on its date, not its contents.
    let rollback = krate()
        .args(["revocations", "--import"])
        .arg(&stolen)
        .env("HOME", home.path())
        .output()
        .expect("import old");
    assert!(
        !rollback.status.success() && String::from_utf8_lossy(&rollback.stderr).contains("OLDER"),
        "an older list must not replace a newer one: {}",
        String::from_utf8_lossy(&rollback.stderr)
    );

    // An edited list is not a list, however plausible the edit.
    let edited_text = std::fs::read_to_string(&later)
        .expect("read")
        .replace("retired", "retired early");
    let edited = write("edited.json", edited_text.as_bytes());
    let forged = krate()
        .args(["revocations", "--import"])
        .arg(&edited)
        .env("HOME", home.path())
        .output()
        .expect("import edited");
    assert!(
        !forged.status.success()
            && String::from_utf8_lossy(&forged.stderr).contains("altered or forged"),
        "an edited list must be refused as such: {}",
        String::from_utf8_lossy(&forged.stderr)
    );
}

/// A person answers once, and the next release of the same app does not ask
/// again -- unless it wants more (IC-017).
///
/// Driven through the CLI with a terminal answer piped in, which is the
/// path a person on a terminal actually takes. The update is a genuinely
/// new file: repacked, re-signed, a new version, and in the third case a
/// widened capability.
#[test]
fn a_grant_survives_an_update_and_a_widened_one_is_asked_again() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = tempfile::tempdir().expect("home");
    let minimal = include_bytes!("../../bundle/tests/fixtures/minimal-run.wasm");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(&wasm, minimal).expect("component");
    let key = dir.path().join("key.pkcs8");
    let mut generated = false;

    // Build, sign and run one release. Returns what the run printed on
    // stderr, where the prompt appears.
    let mut release_of = |app_id: &str,
                          version: &str,
                          caps: &[&str],
                          answer: &str|
     -> (Option<i32>, String) {
        let manifest = dir.path().join("manifest.toml");
        let declared: String = caps
            .iter()
            .map(|cap| {
                format!(
                    "\n[[capabilities]]\ncap = \"{cap}\"\nrationale = \"the app's work\"\nrequired = true\n"
                )
            })
            .collect();
        std::fs::write(
            &manifest,
            format!(
                "[app]\nid = \"{app_id}\"\nname = \"Grants\"\nversion = \"{version}\"\n\
                 entry = \"code.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n{declared}"
            ),
        )
        .expect("manifest");
        let bundle = dir.path().join(format!("{app_id}-v{version}.krate"));
        assert!(krate()
            .args(["pack"])
            .arg(&wasm)
            .arg("--manifest")
            .arg(&manifest)
            .arg("-o")
            .arg(&bundle)
            .status()
            .expect("pack")
            .success());
        let mut sign = krate();
        sign.arg("sign").arg(&bundle).arg("--key").arg(&key);
        if !generated {
            sign.arg("--generate-key");
            generated = true;
        }
        assert!(
            sign.args(["--namespace", "grants/app"])
                .output()
                .expect("sign")
                .status
                .success(),
            "sign must succeed"
        );

        let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_krate"))
            .arg("run")
            .arg(&bundle)
            .args(["--headless", "--prompt"])
            .env("HOME", home.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn");
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .expect("stdin")
            .write_all(answer.as_bytes())
            .expect("answer");
        let out = child.wait_with_output().expect("wait");
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).to_string()
                + &String::from_utf8_lossy(&out.stdout),
        )
    };

    let (code, first) = release_of("dev.krate.grants", "1.0.0", &["fs.read:notes/**"], "A\n");
    assert_eq!(code, Some(0), "the app runs once allowed: {first}");
    assert!(
        first.contains("fs.read:notes/**"),
        "the first run must ASK for the capability: {first}"
    );

    // Second run: a new release of the same app, the same capability, and
    // no answer on stdin at all. It must not need one.
    let (code, second) = release_of("dev.krate.grants", "2.0.0", &["fs.read:notes/**"], "");
    assert_eq!(
        code,
        Some(0),
        "an update asking for what was already granted must not ask again: {second}"
    );

    // Third: the update wants every file instead of the notes folder. That
    // is a different question and must be asked, and with no answer piped
    // in the run is refused rather than allowed.
    let (code, third) = release_of("dev.krate.grants", "3.0.0", &["fs.read:**"], "");
    assert_ne!(
        code,
        Some(0),
        "a widened capability must not ride in on the old yes: {third}"
    );
    assert!(
        third.contains("fs.read:**"),
        "and the person must be told what the new ask is: {third}"
    );

    // A PARTIAL answer is remembered partially. Two capabilities are asked
    // for and the person picks only the first; the next release must still
    // ask about the second. Remembering everything the manifest DECLARED
    // instead of what was granted would turn "allow just this one" into
    // "allow both, from now on" -- silently, on the next run.
    //
    // Both capabilities have to be ones the prompt actually asks about:
    // a default-granted capability like time.clock never appears, so a
    // pair including it cannot express a partial answer at all. The first
    // draft of this used one, and the run it expected to be refused was
    // correctly allowed.
    let pair = &["fs.read:partial/**", "net.connect:example.com:443"];
    // Both are required, so granting only the first leaves the run refused
    // -- and that refusal is beside the point here. What matters is that
    // the yes to the first was recorded and the no to the second was not.
    let (_, fourth) = release_of("dev.krate.partial", "1.0.0", pair, "1\n");
    assert!(
        fourth.contains("fs.read:partial/**") && fourth.contains("net.connect:example.com:443"),
        "both must be asked the first time, or a partial answer means nothing: {fourth}"
    );
    let (code, fifth) = release_of("dev.krate.partial", "2.0.0", pair, "");
    assert_ne!(
        code,
        Some(0),
        "the capability the person did NOT pick must still be asked about: {fifth}"
    );
    assert!(
        fifth.contains("net.connect:example.com:443"),
        "and it is the unpicked one that is asked: {fifth}"
    );
    assert!(
        !fifth.contains("fs.read:partial/**"),
        "while the one they did pick is not asked again: {fifth}"
    );
}

/// A run asked to paint a frame does not report success when it painted
/// nothing (IC-743, tests 1504 and 1505).
///
/// The frame is written by the GUI host, which an app with no window
/// never reaches, so `--shoot` on a CLI app exited 0 and produced no
/// file. Every caller that trusted the exit code believed a picture had
/// been painted -- check-app's painting stage among them, which proved
/// only that the run ended. An app's own clean exit is not evidence
/// about a picture.
#[test]
fn a_run_that_painted_nothing_does_not_report_a_frame() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(
        &wasm,
        include_bytes!("../../bundle/tests/fixtures/minimal-run.wasm"),
    )
    .expect("component");
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"dev.krate.noframe\"\nname = \"No Frame\"\nversion = \"1.0.0\"\n\
         entry = \"code.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n",
    )
    .expect("manifest");
    let frame = dir.path().join("frame.png");

    let output = krate()
        .arg("run")
        .arg(&wasm)
        .arg("--manifest")
        .arg(&manifest)
        .args(["--auto-grant", "--headless"])
        .arg("--shoot")
        .arg(&frame)
        .output()
        .expect("run --shoot");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_ne!(
        output.status.code(),
        Some(0),
        "a run that painted nothing must not report success: {stderr}"
    );
    assert!(
        !frame.exists() || std::fs::metadata(&frame).map(|m| m.len()).unwrap_or(0) == 0,
        "the fixture must actually paint nothing, or this proves something else"
    );
    assert!(
        stderr.contains("none was painted"),
        "and it must say what did not happen: {stderr}"
    );
    assert!(
        stderr.contains("drop --shoot"),
        "and what to do about it -- a CLI app has no window to photograph: {stderr}"
    );
}

/// Every adversarial archive is refused by the BINARY people run, not
/// only by the library (IC-714, tests 1471-1486).
///
/// This row exists because the two disagreed: the source contained the
/// duplicate-asset rejection and the shipped binaries accepted the
/// archive anyway. A library test cannot catch that -- it tests the
/// library. So the fixtures go through `krate run`, the same path a
/// recipient takes, and each must be refused with a message about what
/// is actually wrong.
///
/// The archives assembled here cover the shapes a committed fixture
/// cannot: the zip crate's writer refuses to emit a duplicate name, and
/// Python's zipfile will not write some of these either, so they are
/// built byte by byte in the test.
#[test]
fn every_adversarial_archive_is_refused_by_the_binary_people_run() {
    let dir = tempfile::tempdir().expect("tempdir");
    let minimal = include_bytes!("../../bundle/tests/fixtures/minimal-run.wasm");

    // The committed fixtures, through the public path. Each was built to
    // exercise a shape our own writer cannot produce.
    let committed: [(&str, &[u8], &str); 4] = [
        (
            "a duplicate name from another writer",
            include_bytes!("../../bundle/tests/fixtures/duplicate-from-another-writer.krate"),
            "names the same file twice",
        ),
        (
            "an entry nobody can read",
            include_bytes!("../../bundle/tests/fixtures/locked-entry.krate"),
            "locked entry",
        ),
        (
            "a duplicate source path",
            include_bytes!("../../krate-hub/tests/fixtures/duplicate-source-path.krate"),
            "names the same file twice",
        ),
        (
            "a forged size bomb",
            include_bytes!("../../krate-hub/tests/fixtures/forged-size-bomb.krate"),
            "",
        ),
    ];
    for (what, bytes, expected) in committed {
        let path = dir.path().join(format!("{}.krate", what.replace(' ', "-")));
        std::fs::write(&path, bytes).expect("write fixture");
        let output = krate()
            .arg("run")
            .arg(&path)
            .args(["--headless", "--auto-grant"])
            .output()
            .expect("run");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_ne!(
            output.status.code(),
            Some(0),
            "{what} must be refused by the binary, not only by the library: {stderr}"
        );
        if !expected.is_empty() {
            assert!(
                stderr.contains(expected),
                "{what} must be refused for what it IS ({expected:?}): {stderr}"
            );
        }
    }

    // Path shapes, assembled here. Each must be refused; an entry that is
    // merely ignored would leave the archive opening as though it were
    // clean, which is how a hostile record survives review.
    let path_shapes: [(&str, &str); 6] = [
        (
            "a traversal out of assets",
            "assets/../../../../tmp/krate-test-escape",
        ),
        (
            "a traversal out of source",
            "source/../../../../tmp/krate-test-escape",
        ),
        (
            "a traversal out of the sdk",
            "sdk/../../../../tmp/krate-test-escape",
        ),
        ("a dot segment", "source/./lib.rs"),
        ("a doubled separator", "assets//logo.png"),
        ("a backslash separator", r"assets\logo.png"),
    ];
    for (what, entry) in path_shapes {
        let path = dir.path().join(format!("{}.krate", what.replace(' ', "-")));
        std::fs::write(
            &path,
            archive_with_component(minimal, &[(entry.to_string(), b"payload".to_vec())]),
        )
        .expect("write");
        let output = krate()
            .arg("run")
            .arg(&path)
            .args(["--headless", "--auto-grant"])
            .output()
            .expect("run");
        assert_ne!(
            output.status.code(),
            Some(0),
            "{what} ({entry}) must be refused: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // 1471-1474 and 1478: two records for one canonical path, built by
    // hand because no ordinary writer will emit them. Each pair is refused
    // for what it is, whichever copy comes first -- a reader that keeps the
    // last and a reviewer that shows the first must never disagree.
    let manifest_b = "[app]\nid = \"com.example.other\"\nname = \"Other\"\n\
                      version = \"9.9.9\"\nentry = \"code.wasm\"\n\
                      world = \"krate:app/cli@0.1.0\"\n";
    type Entries = Vec<(String, Vec<u8>)>;
    let twins: [(&str, Entries, &str); 7] = [
        (
            "a second manifest (1471)",
            vec![("manifest.toml".into(), manifest_b.as_bytes().to_vec())],
            "names the same file twice",
        ),
        (
            "a second component (1472)",
            vec![("code.wasm".into(), minimal.to_vec())],
            "names the same file twice",
        ),
        (
            "the same asset twice (1473)",
            vec![
                ("assets/logo.png".into(), b"first".to_vec()),
                ("assets/logo.png".into(), b"second".to_vec()),
            ],
            "names the same file twice",
        ),
        (
            "the same SDK file twice (1474)",
            vec![
                ("sdk/wit/world.wit".into(), b"first".to_vec()),
                ("sdk/wit/world.wit".into(), b"second".to_vec()),
            ],
            "names the same file twice",
        ),
        (
            "two spellings that differ only in case (1478)",
            vec![
                ("assets/Logo.png".into(), b"first".to_vec()),
                ("assets/logo.png".into(), b"second".to_vec()),
            ],
            "names the same file twice",
        ),
        (
            "a composed Unicode name (1478)",
            vec![("source/caf\u{e9}.rs".into(), b"nfc".to_vec())],
            "",
        ),
        (
            "a decomposed Unicode name beside the composed one (1478)",
            vec![
                ("source/caf\u{e9}.rs".into(), b"nfc".to_vec()),
                ("source/cafe\u{301}.rs".into(), b"nfd".to_vec()),
            ],
            "",
        ),
    ];
    for (what, extra, expected) in &twins {
        let path = dir.path().join(format!("{}.krate", what.replace(' ', "-")));
        std::fs::write(&path, archive_with_component(minimal, extra)).expect("write");
        let output = krate()
            .arg("run")
            .arg(&path)
            .args(["--headless", "--auto-grant"])
            .output()
            .expect("run");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_ne!(
            output.status.code(),
            Some(0),
            "{what} must be refused by the binary: {stderr}"
        );
        if !expected.is_empty() {
            assert!(
                stderr.contains(expected),
                "{what} must be refused for what it IS ({expected:?}): {stderr}"
            );
        }
    }

    // 1485: names that point outside the extraction on some filesystem
    // family. A Unix reader sees `C:` and `\\server` as ordinary
    // characters; a recipient on Windows does not.
    //
    // Two rules meet here, and the test holds both. Profile 2 -- what every
    // Krate since 2026-09-14 writes -- refuses a top-level record it does
    // not name, so each of these is refused outright. Profile 1 ignores
    // unknown top-level records by its recorded rules (the library's
    // `open_ignores_extra_entries_including_traversal_attempts`), and old
    // bundles must keep opening; there the guarantee is that the record is
    // never written anywhere, which is checked, not assumed.
    let escapes: [(&str, &str); 4] = [
        ("an absolute path", "/tmp/krate-test-escape"),
        ("a drive-letter path", "C:/krate-test-escape"),
        (
            "a drive-letter path with a backslash",
            r"C:\krate-test-escape",
        ),
        ("a UNC path", r"\\server\share\krate-test-escape"),
    ];
    for (what, entry) in escapes {
        for profile in [1u32, 2] {
            let mut extra = vec![(entry.to_string(), b"payload".to_vec())];
            if profile == 2 {
                extra.insert(0, ("krate-profile".to_string(), b"2".to_vec()));
            }
            let path = dir
                .path()
                .join(format!("{}-profile{profile}.krate", what.replace(' ', "-")));
            std::fs::write(&path, archive_with_component(minimal, &extra)).expect("write");
            let output = krate()
                .arg("run")
                .arg(&path)
                .args(["--headless", "--auto-grant"])
                .current_dir(dir.path())
                .output()
                .expect("run");
            let stderr = String::from_utf8_lossy(&output.stderr);
            if profile == 2 {
                assert_ne!(
                    output.status.code(),
                    Some(0),
                    "{what} ({entry}) must be refused under profile 2: {stderr}"
                );
                assert!(
                    stderr.contains("krate-test-escape"),
                    "and the refusal must name the record: {stderr}"
                );
            }
        }
    }
    // Wherever a profile 1 reader might have put them: nowhere. On Unix the
    // names are literal file names in the working directory; on Windows
    // they are what they say, a drive root. (Joining `C:` onto a Windows
    // path means "the current directory on drive C", which always exists,
    // so the Unix check would be wrong there.)
    #[cfg(not(windows))]
    {
        assert!(!std::path::Path::new("/tmp/krate-test-escape").exists());
        for stray in [
            "C:",
            r"C:\krate-test-escape",
            r"\\server\share\krate-test-escape",
        ] {
            assert!(
                !dir.path().join(stray).exists(),
                "an ignored record must not be written, even under the working directory: {stray}"
            );
        }
    }
    #[cfg(windows)]
    assert!(
        !std::path::Path::new(r"C:\krate-test-escape").exists(),
        "an ignored drive-letter record must not be written to the drive root"
    );

    // Inside a namespace the name is judged whatever the profile: a
    // drive-relative segment would be an alternate data stream on NTFS.
    for profile in [1u32, 2] {
        let mut extra = vec![(
            "assets/C:krate-test-escape".to_string(),
            b"payload".to_vec(),
        )];
        if profile == 2 {
            extra.insert(0, ("krate-profile".to_string(), b"2".to_vec()));
        }
        let path = dir
            .path()
            .join(format!("drive-in-assets-profile{profile}.krate"));
        std::fs::write(&path, archive_with_component(minimal, &extra)).expect("write");
        let output = krate()
            .arg("run")
            .arg(&path)
            .args(["--headless", "--auto-grant"])
            .output()
            .expect("run");
        assert_ne!(
            output.status.code(),
            Some(0),
            "a drive-relative asset name must be refused under profile {profile}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // 1484: more records than the container allows is refused before the
    // guest is launched. One over the limit, each record a few bytes. A file
    // run from this machine is judged by the large local tier (IC-942), so
    // the limit here is that tier's.
    let crowd: Vec<(String, Vec<u8>)> = (0..=krate_bundle::LOCAL_LIMITS.entry_count)
        .map(|i| (format!("assets/f{i}.txt"), b"x".to_vec()))
        .collect();
    let crowded = dir.path().join("too-many-records.krate");
    std::fs::write(&crowded, archive_with_component(minimal, &crowd)).expect("write");
    let output = krate()
        .arg("run")
        .arg(&crowded)
        .args(["--headless", "--auto-grant"])
        .output()
        .expect("run");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_ne!(
        output.status.code(),
        Some(0),
        "{} records must be refused: {stderr}",
        crowd.len() + 2,
    );
    assert!(
        stderr.contains(&format!(
            "more than {} files",
            krate_bundle::LOCAL_LIMITS.entry_count
        )),
        "and refused for the count, not for something else in the archive: {stderr}"
    );

    // 1486: a rejection leaves nothing behind.
    //
    // Measured in a temp directory of this test's own, handed to the child
    // through TMPDIR. Two earlier versions counted `krate-open-*` in the
    // SYSTEM temp directory and both were wrong for the same reason: 38
    // tests in this suite run a bundle, cargo runs them in parallel, and
    // their live extractions are in flight throughout. "Is it empty"
    // failed on the Linux lane; "did the count grow" failed locally two
    // runs in three. A shared directory cannot answer a question about one
    // test.
    //
    // Private, it answers exactly: every archive below is refused after the
    // opener has begun unpacking, so anything left here afterwards is an
    // extraction that was not cleaned up.
    let private_tmp = dir.path().join("extraction-probe");
    std::fs::create_dir_all(&private_tmp).expect("private temp dir");
    for (what, bytes, _) in committed {
        let path = dir
            .path()
            .join(format!("recheck-{}.krate", what.replace(' ', "-")));
        std::fs::write(&path, bytes).expect("write fixture");
        let _ = krate()
            .arg("run")
            .arg(&path)
            .args(["--headless", "--auto-grant"])
            .env("TMPDIR", &private_tmp)
            .output()
            .expect("run");
    }
    let leftovers: Vec<String> = std::fs::read_dir(&private_tmp)
        .expect("read the private temp dir")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert!(
        leftovers.is_empty(),
        "refusing {} archives left extraction state behind: {leftovers:?}",
        committed.len(),
    );

    assert!(
        !std::path::Path::new("/tmp/krate-test-escape").exists(),
        "no traversal may write outside the extraction directory"
    );
}

/// A shared group admits the apps its publisher named, and removing one
/// sticks (IC-738, test 1518, plus the revoked-member case).
///
/// Walked through the binary a publisher and a recipient use: the root
/// signs a list, `krate sign --group` carries it, opening the app teaches
/// this machine the list, a newer list without an app replaces it, and the
/// old list arriving again inside an old bundle is refused rather than
/// re-admitting the app.
#[test]
fn a_shared_group_admits_named_apps_and_a_removal_sticks() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = tempfile::tempdir().expect("home");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(
        &wasm,
        include_bytes!("../../bundle/tests/fixtures/minimal-run.wasm"),
    )
    .expect("component");
    let root = dir.path().join("root.key");
    let run = |args: &[&std::ffi::OsStr]| {
        krate()
            .args(args)
            .env("HOME", home.path())
            .output()
            .expect("krate")
    };
    let os = |s: &str| std::ffi::OsString::from(s);

    // Two apps from one publisher, each packed and signed with the root.
    let pack = |id: &str| {
        let manifest = dir.path().join(format!("{id}.toml"));
        std::fs::write(
            &manifest,
            format!(
                "[app]\nid = \"{id}\"\nname = \"{id}\"\nversion = \"1.0.0\"\n\
                 entry = \"code.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n"
            ),
        )
        .expect("manifest");
        let bundle = dir.path().join(format!("{id}.krate"));
        let out = run(&[
            &os("pack"),
            wasm.as_os_str(),
            &os("--manifest"),
            manifest.as_os_str(),
            &os("-o"),
            bundle.as_os_str(),
        ]);
        assert!(
            out.status.success(),
            "pack: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        bundle
    };
    let budget = pack("com.acme.budget");
    let reports = pack("com.acme.reports");
    let old_reports = dir.path().join("old-reports.krate");

    let sign_list = |members: &[&str], out: &std::path::Path| {
        let mut args = vec![
            os("group"),
            os("sign"),
            os("--root"),
            root.as_os_str().to_owned(),
            os("--group"),
            os("family-budget"),
            os("-o"),
            out.as_os_str().to_owned(),
        ];
        for member in members {
            args.push(os("--member"));
            args.push(os(member));
        }
        let refs: Vec<&std::ffi::OsStr> = args.iter().map(|a| a.as_os_str()).collect();
        let result = run(&refs);
        assert!(
            result.status.success(),
            "group sign: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    };
    let sign_app = |bundle: &std::path::Path, list: &std::path::Path, first: bool| {
        let mut args = vec![
            os("sign"),
            bundle.as_os_str().to_owned(),
            os("--key"),
            root.as_os_str().to_owned(),
            os("--namespace"),
            os("acme/apps"),
            os("--group"),
            list.as_os_str().to_owned(),
        ];
        if first {
            args.push(os("--generate-key"));
        }
        let refs: Vec<&std::ffi::OsStr> = args.iter().map(|a| a.as_os_str()).collect();
        let result = run(&refs);
        assert!(
            result.status.success(),
            "sign: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    };
    let known = || String::from_utf8_lossy(&run(&[&os("group"), &os("list")]).stdout).to_string();

    // The root key is made by the first sign; the list needs it, so the
    // first app is signed once without a list to create the key.
    let bootstrap = run(&[
        &os("sign"),
        budget.as_os_str(),
        &os("--key"),
        root.as_os_str(),
        &os("--namespace"),
        &os("acme/apps"),
        &os("--generate-key"),
    ]);
    assert!(
        bootstrap.status.success(),
        "{}",
        String::from_utf8_lossy(&bootstrap.stderr)
    );

    // List 1 admits both apps; each carries it.
    let first = dir.path().join("family-v1.json");
    sign_list(&["com.acme.budget", "com.acme.reports"], &first);
    sign_app(&budget, &first, false);
    sign_app(&reports, &first, false);
    std::fs::copy(&reports, &old_reports).expect("keep the old reports bundle");
    let _ = run(&[
        &os("run"),
        budget.as_os_str(),
        &os("--headless"),
        &os("--auto-grant"),
    ]);
    let listed = known();
    assert!(
        listed.contains("family-budget") && listed.contains("com.acme.reports"),
        "opening a signed app teaches this machine its group list: {listed}"
    );

    // List 2, a second later, removes reports; budget carries it.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let second = dir.path().join("family-v2.json");
    sign_list(&["com.acme.budget"], &second);
    sign_app(&budget, &second, false);
    let _ = run(&[
        &os("run"),
        budget.as_os_str(),
        &os("--headless"),
        &os("--auto-grant"),
    ]);
    let listed = known();
    assert!(
        listed.contains("com.acme.budget") && !listed.contains("com.acme.reports"),
        "the newer list replaces the older one: {listed}"
    );

    // The old reports bundle, carrying list 1, is opened again. The removal
    // must stick.
    let replay = run(&[
        &os("run"),
        old_reports.as_os_str(),
        &os("--headless"),
        &os("--auto-grant"),
    ]);
    let stderr = String::from_utf8_lossy(&replay.stderr);
    assert!(
        stderr.contains("older list for shared group family-budget"),
        "an old list turning up again is refused, and said so: {stderr}"
    );
    let listed = known();
    assert!(
        !listed.contains("com.acme.reports"),
        "and the removed app stays removed: {listed}"
    );

    // A list signed by somebody else cannot be carried at all.
    let stranger = dir.path().join("stranger.key");
    let other = pack("com.acme.other");
    let made = run(&[
        &os("sign"),
        other.as_os_str(),
        &os("--key"),
        stranger.as_os_str(),
        &os("--namespace"),
        &os("acme/apps"),
        &os("--generate-key"),
        &os("--group"),
        second.as_os_str(),
    ]);
    assert!(
        !made.status.success(),
        "a list from another publisher is refused at sign"
    );
    assert!(
        String::from_utf8_lossy(&made.stderr).contains("different publisher"),
        "{}",
        String::from_utf8_lossy(&made.stderr)
    );
}

/// A real app uses a shared group through the binary (IC-738, test 1518 and
/// the unauthorized and revoked-member cases).
///
/// `group-probe.wasm` is apps/krate-group-probe built: it calls
/// `krate:store/group` for `family-budget` and prints one line naming the
/// outcome. Packed under several ids, it is every kind of caller: named
/// members that share a value, an unsigned app, an app that never declared
/// the group, a signed app its publisher did not name, and a member the
/// publisher later removed.
#[test]
fn a_group_is_shared_by_its_members_and_closed_to_everyone_else() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = tempfile::tempdir().expect("home");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(&wasm, include_bytes!("fixtures/group-probe.wasm")).expect("probe");
    let root = dir.path().join("root.key");
    let os = |s: &str| std::ffi::OsString::from(s);
    let krate_in_home = |args: &[std::ffi::OsString]| {
        krate()
            .args(args)
            .env("HOME", home.path())
            .output()
            .expect("krate")
    };

    let pack = |id: &str, declares_group: bool| {
        let manifest = dir.path().join(format!("{id}.toml"));
        let mut text = format!(
            "[app]\nid = \"{id}\"\nname = \"{id}\"\nversion = \"1.0.0\"\n\
             entry = \"code.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n\n\
             [[capabilities]]\ncap = \"io.stdout\"\nrationale = \"print\"\nrequired = true\n\n\
             [[capabilities]]\ncap = \"io.args\"\nrationale = \"args\"\nrequired = true\n"
        );
        if declares_group {
            text.push_str(
                "\n[[capabilities]]\ncap = \"store.group:family-budget\"\nrationale = \"share\"\nrequired = false\n",
            );
        }
        std::fs::write(&manifest, text).expect("manifest");
        let bundle = dir.path().join(format!("{id}.krate"));
        let out = krate_in_home(&[
            os("pack"),
            wasm.clone().into(),
            os("--manifest"),
            manifest.into(),
            os("-o"),
            bundle.clone().into(),
        ]);
        assert!(
            out.status.success(),
            "pack: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        bundle
    };
    let sign = |bundle: &std::path::Path, list: Option<&std::path::Path>, generate: bool| {
        let mut args = vec![
            os("sign"),
            bundle.into(),
            os("--key"),
            root.clone().into(),
            os("--namespace"),
            os("acme/apps"),
        ];
        if generate {
            args.push(os("--generate-key"));
        }
        if let Some(list) = list {
            args.push(os("--group"));
            args.push(list.into());
        }
        let out = krate_in_home(&args);
        assert!(
            out.status.success(),
            "sign: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    let list = |name: &str, members: &[&str]| {
        let path = dir.path().join(name);
        let mut args = vec![
            os("group"),
            os("sign"),
            os("--root"),
            root.clone().into(),
            os("--group"),
            os("family-budget"),
            os("-o"),
            path.clone().into(),
        ];
        for member in members {
            args.push(os("--member"));
            args.push(os(member));
        }
        let out = krate_in_home(&args);
        assert!(
            out.status.success(),
            "group sign: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        path
    };
    let call = |bundle: &std::path::Path, words: &[&str]| {
        let mut args = vec![os("run"), bundle.into(), os("--auto-grant"), os("--")];
        args.extend(words.iter().map(|w| os(w)));
        let out = krate_in_home(&args);
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };

    let budget = pack("com.acme.budget", true);
    let reports = pack("com.acme.reports", true);
    let game = pack("com.acme.game", true);
    let quiet = pack("com.acme.quiet", false);
    let unsigned = pack("com.acme.unsigned", true);

    // Before anything is signed: an unsigned app declaring the group, and an
    // app that never declared it.
    assert_eq!(
        call(&unsigned, &["set", "total", "1"]),
        "error=not-a-member"
    );
    assert_eq!(call(&quiet, &["get", "total"]), "error=denied");

    // The publisher names budget and reports.
    sign(&budget, None, true);
    let first = list("v1.json", &["com.acme.budget", "com.acme.reports"]);
    sign(&budget, Some(&first), false);
    sign(&reports, Some(&first), false);
    sign(&game, Some(&first), false);

    assert_eq!(call(&budget, &["set", "total", "1250"]), "set=ok");
    assert_eq!(
        call(&reports, &["get", "total"]),
        "get=1250",
        "a second named app reads what the first wrote -- the group is shared"
    );
    assert_eq!(
        call(&game, &["get", "total"]),
        "error=not-a-member",
        "the same publisher's app is refused when the list does not name it"
    );
    assert_eq!(call(&unsigned, &["get", "total"]), "error=not-a-member");

    // The publisher removes reports. A newer list reaches this machine
    // inside budget; reports' own bundle still carries the old one.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let second = list("v2.json", &["com.acme.budget"]);
    sign(&budget, Some(&second), false);
    assert_eq!(call(&budget, &["get", "total"]), "get=1250");
    assert_eq!(
        call(&reports, &["get", "total"]),
        "error=not-a-member",
        "a removed member is refused, even opening its own bundle that carries the old list"
    );

    // Test 1520: a group's data through export, import and uninstall. It is
    // the group's, not one app's, so it travels in a member's export, goes
    // back only to a member of the same publisher's group, and is never
    // deleted with one app.
    let data = |args: &[std::ffi::OsString]| {
        let out = krate_in_home(args);
        (
            out.status.code(),
            format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ),
        )
    };
    let copy = dir.path().join("budget.krate-data");
    let (code, said) = data(&[
        os("data"),
        os("export"),
        budget.clone().into(),
        os("-o"),
        copy.clone().into(),
    ]);
    assert_eq!(code, Some(0), "{said}");
    assert!(
        said.contains("group") && said.contains("groups/family-budget.kv"),
        "the export carries the group: {said}"
    );
    let group_file = std::fs::read_dir(home.path().join(".krate/groups"))
        .expect("groups dir")
        .map(|e| e.expect("entry").path().join("family-budget.kv"))
        .find(|p| p.is_file())
        .expect("the group's store on disk");
    std::fs::remove_file(&group_file).expect("lose the group's data");
    assert_eq!(call(&budget, &["get", "total"]), "get=none");

    let import = |into: &std::path::Path, extra: &[&str]| {
        let mut args = vec![
            os("data"),
            os("import"),
            copy.clone().into(),
            os("--into"),
            into.into(),
        ];
        args.extend(extra.iter().map(|a| os(a)));
        data(&args)
    };
    let (code, said) = import(&reports, &["--accept-different-app"]);
    assert_eq!(code, Some(1), "{said}");
    assert!(said.contains("not a member"), "{said}");
    assert!(
        !group_file.exists(),
        "a removed member cannot restore its way back into the group"
    );

    // Another publisher with its own family-budget group, naming its app: a
    // member, but of a different group that happens to share the name.
    let other_root = dir.path().join("other-root.key");
    let rival = pack("com.rival.budget", true);
    let rival_list = dir.path().join("rival.json");
    for args in [
        vec![
            os("sign"),
            rival.clone().into(),
            os("--key"),
            other_root.clone().into(),
            os("--namespace"),
            os("rival/apps"),
            os("--generate-key"),
        ],
        vec![
            os("group"),
            os("sign"),
            os("--root"),
            other_root.clone().into(),
            os("--group"),
            os("family-budget"),
            os("-o"),
            rival_list.clone().into(),
            os("--member"),
            os("com.rival.budget"),
        ],
        vec![
            os("sign"),
            rival.clone().into(),
            os("--key"),
            other_root.clone().into(),
            os("--namespace"),
            os("rival/apps"),
            os("--group"),
            rival_list.clone().into(),
        ],
    ] {
        let out = krate_in_home(&args);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    assert_eq!(call(&rival, &["get", "total"]), "get=none");
    let (code, said) = import(&rival, &["--accept-different-app"]);
    assert_eq!(code, Some(1), "{said}");
    assert!(said.contains("another publisher"), "{said}");
    assert_eq!(
        call(&rival, &["get", "total"]),
        "get=none",
        "a same-named group under another publisher gets nothing"
    );

    let (code, said) = import(&budget, &[]);
    assert_eq!(code, Some(0), "{said}");
    assert_eq!(
        call(&budget, &["get", "total"]),
        "get=1250",
        "the member gets its group back"
    );
    let (code, said) = import(&budget, &[]);
    assert_eq!(
        code,
        Some(1),
        "existing group data is not overwritten unasked: {said}"
    );

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let prefix = tempfile::tempdir().expect("prefix");
        let out = krate_in_home(&[
            os("install"),
            budget.clone().into(),
            os("--prefix"),
            prefix.path().into(),
        ]);
        if out.status.success() {
            let (code, said) = data(&[
                os("uninstall"),
                os("com.acme.budget"),
                os("--prefix"),
                prefix.path().into(),
                os("--delete-data"),
            ]);
            assert_eq!(code, Some(0), "{said}");
            assert!(
                said.contains("Kept the shared group family-budget"),
                "{said}"
            );
            assert!(
                group_file.is_file(),
                "deleting one app's data never deletes the group other apps share"
            );
        } else {
            eprintln!("skipping the uninstall check: this platform has no installer");
        }
    }

    // Test 1519: data two apps shared by reusing one id comes into the
    // group -- atomically, reversibly, and never by Krate choosing between
    // claimants.
    std::fs::remove_file(&group_file).expect("start the group empty");
    let legacy = |id: &str| home.path().join(format!(".krate/store/{id}.kv"));
    std::fs::create_dir_all(legacy("x").parent().unwrap()).expect("store dir");
    std::fs::write(legacy("com.acme.budget"), "total\tOTAw\n").expect("legacy budget"); // 900
    std::fs::write(legacy("com.acme.reports"), "total\tNDI=\n").expect("legacy reports"); // 42
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let third = list("v3.json", &["com.acme.budget", "com.acme.reports"]);
    sign(&budget, Some(&third), false);
    let adopt = |app: &std::path::Path, extra: &[&str]| {
        let mut args = vec![
            os("group"),
            os("adopt"),
            app.into(),
            os("--group"),
            os("family-budget"),
        ];
        args.extend(extra.iter().map(|a| os(a)));
        data(&args)
    };

    let (code, said) = adopt(&game, &[]);
    assert_eq!(code, Some(1), "{said}");
    assert!(said.contains("not a member"), "{said}");

    let (code, said) = adopt(&budget, &[]);
    assert_eq!(code, Some(1), "two claimants: {said}");
    assert!(
        said.contains("will not choose")
            && said.contains("com.acme.budget")
            && said.contains("com.acme.reports"),
        "{said}"
    );
    assert!(!group_file.exists(), "a refusal changes nothing");

    let (code, said) = adopt(&budget, &["--from", "com.acme.budget"]);
    assert_eq!(code, Some(0), "{said}");
    assert_eq!(call(&budget, &["get", "total"]), "get=900");
    assert_eq!(call(&reports, &["get", "total"]), "get=900");
    assert_eq!(
        std::fs::read_to_string(legacy("com.acme.budget")).expect("legacy"),
        "total\tOTAw\n",
        "the old store is left exactly as it was"
    );

    let (code, said) = adopt(&budget, &["--from", "com.acme.reports"]);
    assert_eq!(
        code,
        Some(1),
        "a group with data is a second claimant: {said}"
    );
    assert_eq!(call(&budget, &["get", "total"]), "get=900");

    let (code, said) = adopt(&budget, &["--undo"]);
    assert_eq!(code, Some(0), "{said}");
    assert!(!group_file.exists(), "undo takes the copy back out");
    assert!(legacy("com.acme.budget").is_file());

    let (code, said) = adopt(&budget, &["--from", "com.acme.reports"]);
    assert_eq!(code, Some(0), "{said}");
    assert_eq!(call(&budget, &["set", "total", "43"]), "set=ok");
    let (code, said) = adopt(&budget, &["--undo"]);
    assert_eq!(
        code,
        Some(1),
        "undo after an app wrote would lose the writing: {said}"
    );
    assert!(said.contains("changed since"), "{said}");
    assert_eq!(call(&budget, &["get", "total"]), "get=43");
}

/// An app prints what its window shows (krate:ui/print, the capability
/// roadmap's `ui.print`), through the binary a person runs.
///
/// `print-probe.wasm` is apps/krate-print-probe built: it draws a rate
/// card, prints it, and exits without waiting for input -- which also
/// covers K-881, the abort at teardown when an app left a presented
/// window open. `KRATE_PRINT_TO` makes the host write the document instead
/// of showing a dialog: on macOS through the system's own print operation
/// as a save job, elsewhere as the PDF the viewer would have opened.
#[test]
fn an_app_prints_its_window_and_the_document_has_its_pages() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(&wasm, include_bytes!("fixtures/print-probe.wasm")).expect("probe");
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"com.acme.print-probe\"\nname = \"print-probe\"\nversion = \"1.0.0\"\n\
         entry = \"code.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n\n\
         [[capabilities]]\ncap = \"ui.window:create\"\nrationale = \"draw\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"io.stdout\"\nrationale = \"print\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"io.args\"\nrationale = \"args\"\nrequired = true\n",
    )
    .expect("manifest");
    let bundle = dir.path().join("print.krate");
    assert!(krate()
        .arg("pack")
        .arg(&wasm)
        .arg("--manifest")
        .arg(&manifest)
        .arg("-o")
        .arg(&bundle)
        .status()
        .expect("pack")
        .success());
    let run = |mode: &str| {
        let pdf = dir.path().join(format!("{mode}.pdf"));
        let out = krate()
            .env("KRATE_PRINT_TO", &pdf)
            .arg("run")
            .arg(&bundle)
            .arg("--auto-grant")
            .args(["--", mode])
            .output()
            .expect("run");
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(
            out.status.success() && !stderr.contains("panicked"),
            "the run ends cleanly, window left open or not (K-881): {stderr}"
        );
        (
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
            std::fs::read(&pdf).ok(),
        )
    };
    let delivered = if cfg!(target_os = "macos") {
        "printed"
    } else {
        "opened-in-viewer"
    };

    let (said, one) = run("one");
    assert_eq!(said, delivered);
    let one = one.expect("the document was written");
    assert!(
        one.starts_with(b"%PDF-") && one.len() > 2_000,
        "a real document"
    );

    let (said, two) = run("two");
    assert_eq!(said, format!("pages=2 {delivered}"));
    let two = two.expect("the two-page document was written");
    assert!(two.starts_with(b"%PDF-"));
    if !cfg!(target_os = "macos") {
        assert!(
            String::from_utf8_lossy(&two).contains("/Count 2"),
            "two pages in the document"
        );
    }

    assert_eq!(run("bad-window").0, "error=invalid-window");
    assert_eq!(
        run("empty").0,
        "error=other",
        "finishing with no pages added is refused, not an empty print"
    );
}

/// An app notices files change in a folder it may list (krate:fs/watch,
/// the capability roadmap's `fs.watch`).
///
/// `watch-probe.wasm` is apps/krate-watch-probe built: it starts a watch,
/// then creates, changes and removes one file itself, printing what each
/// look reported -- and that a look sooner than a quarter second is empty.
#[test]
fn a_watch_reports_what_changed_in_a_folder_and_nothing_outside_its_grant() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(&wasm, include_bytes!("fixtures/watch-probe.wasm")).expect("probe");
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"com.acme.watch-probe\"\nname = \"watch-probe\"\nversion = \"1.0.0\"\n\
         entry = \"code.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n\n\
         [[capabilities]]\ncap = \"io.stdout\"\nrationale = \"print\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"io.args\"\nrationale = \"args\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"fs.list:./inbox/**\"\nrationale = \"watch\"\nrequired = false\n\n\
         [[capabilities]]\ncap = \"fs.write:./inbox/**\"\nrationale = \"drop\"\nrequired = false\n\n\
         [[capabilities]]\ncap = \"fs.remove:./inbox/**\"\nrationale = \"take\"\nrequired = false\n",
    )
    .expect("manifest");
    let bundle = dir.path().join("watch.krate");
    assert!(krate()
        .arg("pack")
        .arg(&wasm)
        .arg("--manifest")
        .arg(&manifest)
        .arg("-o")
        .arg(&bundle)
        .status()
        .expect("pack")
        .success());
    std::fs::create_dir_all(dir.path().join("inbox")).expect("inbox");
    std::fs::create_dir_all(dir.path().join("other")).expect("other");
    let run = |grants: &[&str], folder: &str| {
        let out = krate()
            .current_dir(dir.path())
            .arg("run")
            .arg(&bundle)
            .args(grants)
            .args(["--", folder])
            .output()
            .expect("run");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };

    assert_eq!(
        run(&["--auto-grant"], "inbox"),
        "created: soon=empty then created inbox/note.txt\n\
         modified: soon=empty then modified inbox/note.txt\n\
         removed: soon=empty then removed inbox/note.txt\n\
         stopped=gone"
    );
    assert_eq!(
        run(&["--grant", "io.stdout", "--grant", "io.args"], "inbox"),
        "start=permission-denied",
        "a watch needs what listing needs"
    );
    assert_eq!(
        run(&["--auto-grant"], "other"),
        "start=permission-denied",
        "a folder outside the list grant cannot be watched"
    );
}

/// Krate's own logs never land in an app's output (K-884).
///
/// The log writer defaulted to stdout, so on a Windows runner a wgpu driver
/// warning appeared in the middle of what an app printed. With every log
/// level on, stdout must still be exactly the app's line.
#[test]
fn krate_logs_go_to_stderr_never_into_what_the_app_prints() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(&wasm, include_bytes!("fixtures/say-probe.wasm")).expect("probe");
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"com.acme.say-probe\"\nname = \"say-probe\"\nversion = \"1.0.0\"\n\
         entry = \"code.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n\n\
         [[capabilities]]\ncap = \"io.stdout\"\nrationale = \"print\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"io.args\"\nrationale = \"args\"\nrequired = true\n",
    )
    .expect("manifest");
    let bundle = dir.path().join("say.krate");
    assert!(krate()
        .arg("pack")
        .arg(&wasm)
        .arg("--manifest")
        .arg(&manifest)
        .arg("-o")
        .arg(&bundle)
        .status()
        .expect("pack")
        .success());
    let out = krate()
        .env("KRATE_LOG", "debug")
        .arg("run")
        .arg(&bundle)
        .arg("--auto-grant")
        .args(["--", "rate", "3"])
        .output()
        .expect("run");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("DEBUG"),
        "the fixture must really produce logs, or this proves nothing"
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "error=invalid-request rate 3 is outside 0.5 to 2",
        "stdout is the app's and only the app's"
    );
}

/// An ordinary C++ program runs as a Krate app (IC-908): the STL, iostream,
/// files, folders, the clock, setjmp -- and its static constructors run
/// exactly once.
///
/// `cxx-probe.wasm` is apps/krate-cxx-probe built. Constructors ran twice
/// once (K-1009): the app's export ran them and the C runtime ran them
/// again, and every self-registering C++ list -- Counter-Strike's menus --
/// became a cycle the program looped on forever.
#[test]
fn a_cpp_program_runs_and_its_constructors_run_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(&wasm, include_bytes!("fixtures/cxx-probe.wasm")).expect("probe");
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"com.acme.cxx-probe\"\nname = \"cxx-probe\"\nversion = \"1.0.0\"\n\
         entry = \"code.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n\n\
         [[capabilities]]\ncap = \"io.stdout\"\nrationale = \"print\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"io.args\"\nrationale = \"args\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"fs.read:./probe-dir/**\"\nrationale = \"read\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"fs.write:./probe-dir/**\"\nrationale = \"write\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"fs.list:./probe-dir/**\"\nrationale = \"list\"\nrequired = true\n",
    )
    .expect("manifest");
    let bundle = dir.path().join("cxx.krate");
    assert!(krate()
        .arg("pack")
        .arg(&wasm)
        .arg("--manifest")
        .arg(&manifest)
        .arg("-o")
        .arg(&bundle)
        .status()
        .expect("pack")
        .success());
    let out = krate()
        .current_dir(dir.path())
        .arg("run")
        .arg("--headless")
        .arg(&bundle)
        .arg("--auto-grant")
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&out.stdout);
    for line in [
        "global: constructed",
        "ctors: 1",
        "file: hello from C++ 42",
        "w+: round-trip",
        "many: 300",
        "setjmp: came back with 7",
        "probe: ok",
        "exit: 0",
    ] {
        assert!(
            stdout.lines().any(|l| l.trim() == line),
            "missing {line:?} in: {stdout}{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// The binary a person runs times an app the way a phone runs it (IC-915).
///
/// `KRATE_DRIVE_SCROLL` feeds the app scroll steps through its own wait and
/// then closes it; `KRATE_COST_REPORT` says how long it took to reach its
/// first frame and how long each answered step took. `shelf.wasm`
/// (apps/krate-shelf) scrolls its Listen Now page, so every step down and
/// back up is answered with a frame and timed. A drive that never ran, or a
/// report without the first frame, would leave check-app's phone stage
/// judging nothing.
#[test]
fn a_scroll_drive_times_the_app_and_closes_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(&wasm, include_bytes!("fixtures/shelf.wasm")).expect("component");
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"com.acme.shelf\"\nname = \"shelf\"\nversion = \"1.0.0\"\n\
         entry = \"code.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n\n\
         [[capabilities]]\ncap = \"ui.window:create\"\nrationale = \"window\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"io.args\"\nrationale = \"args\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"audio.playback\"\nrationale = \"play\"\nrequired = true\n",
    )
    .expect("manifest");
    let report = dir.path().join("cost.json");
    let started = std::time::Instant::now();
    let out = krate()
        .arg("run")
        .arg(&wasm)
        .arg("--manifest")
        .arg(&manifest)
        .args(["--auto-grant", "--headless"])
        .env("KRATE_SCREEN", "390x844")
        .env("KRATE_DRIVE_SCROLL", "12")
        .env("KRATE_COST_REPORT", &report)
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(20),
        "the drive closes the app when its steps are done"
    );
    let cost: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report).expect("a cost report"))
            .expect("json");
    assert_eq!(cost["steps"], serde_json::json!(12), "{cost}");
    assert!(
        cost["first_frame_ms"].as_f64().unwrap_or(0.0) > 0.0,
        "{cost}"
    );
    assert_eq!(
        cost["frames"],
        serde_json::json!(12),
        "each step down and back up is answered with a frame, and each is timed: {cost}"
    );
}

/// An app can tap back on a phone, and a computer says plainly it cannot
/// (krate:ui/haptics, IC-917).
///
/// `haptics-probe.wasm` is apps/krate-haptics-probe built. Its quick mode
/// plays every feel and three impacts -- one in range, one far over, one
/// NaN -- and reports `available`. On a computer every call must answer
/// and do nothing, and `available` must be false: an app told a desktop
/// can tap back would make a tap its only sign that something happened.
#[test]
fn an_app_taps_back_on_a_phone_and_a_computer_says_it_cannot() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(&wasm, include_bytes!("fixtures/haptics-probe.wasm")).expect("probe");
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"dev.krate.haptics-probe\"\nname = \"Tap Back\"\nversion = \"1.0.0\"\n\
         entry = \"code.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n\n\
         [[capabilities]]\ncap = \"ui.window:create\"\nrationale = \"window\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"io.stdout\"\nrationale = \"print\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"io.args\"\nrationale = \"args\"\nrequired = true\n",
    )
    .expect("manifest");
    let out = krate()
        .arg("run")
        .arg("--headless")
        .arg(&wasm)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--auto-grant")
        .arg("--")
        .arg("quick")
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "every haptics call answers on a computer: {stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.lines().any(|l| l == "haptics:available=no"),
        "a computer has nothing to tap with and says so: {stdout}"
    );
}

/// Photos: picking with no one there answers as cancelled, adding refuses
/// what is not a picture, and adding needs photos.add (krate:ui/photos,
/// IC-920).
///
/// `phone-check.wasm` is apps/krate-phone-check built; its quick-photos mode
/// picks, then adds junk bytes and then a real PNG. Granted, the junk is
/// invalid and the PNG finds no library on a computer; without photos.add
/// the PNG is not-allowed -- the guard answers before the library is asked.
#[test]
fn photos_pick_and_add_answer_as_they_must() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(&wasm, include_bytes!("fixtures/phone-check.wasm")).expect("probe");
    let manifest = |with_photos: bool| {
        let path = dir.path().join(if with_photos {
            "with.toml"
        } else {
            "without.toml"
        });
        let photos = if with_photos {
            "[[capabilities]]\ncap = \"photos.add\"\nrationale = \"add\"\nrequired = true\n\n"
        } else {
            ""
        };
        std::fs::write(
            &path,
            format!(
                "[app]\nid = \"dev.krate.phone-check\"\nname = \"Phone Check\"\nversion = \"1.0.0\"\n\
                 entry = \"code.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n\n\
                 [[capabilities]]\ncap = \"ui.window:create\"\nrationale = \"window\"\nrequired = true\n\n\
                 {photos}\
                 [[capabilities]]\ncap = \"io.stdout\"\nrationale = \"print\"\nrequired = true\n\n\
                 [[capabilities]]\ncap = \"io.args\"\nrationale = \"args\"\nrequired = true\n"
            ),
        )
        .expect("manifest");
        path
    };
    let run = |manifest: &std::path::Path| -> String {
        let out = krate()
            .arg("run")
            .arg("--headless")
            .arg(&wasm)
            .arg("--manifest")
            .arg(manifest)
            .arg("--auto-grant")
            .arg("--")
            .arg("quick-photos")
            .output()
            .expect("run");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let granted = run(&manifest(true));
    assert!(
        granted
            .lines()
            .any(|l| l == "photos:pick=0 junk=invalid png=unavailable"),
        "granted: no pick headless, junk refused, no library on a computer: {granted}"
    );
    let refused = run(&manifest(false));
    assert!(
        refused
            .lines()
            .any(|l| l == "photos:pick=0 junk=not-allowed png=not-allowed"),
        "without photos.add, adding is not allowed before anything else: {refused}"
    );
}

/// The share sheet refuses what it must before it opens, and a computer
/// says plainly it has none (krate:ui/share, IC-919).
///
/// `phone-check.wasm` is apps/krate-phone-check built. Its quick-share mode
/// asks to share nothing, a file named with a path, and a plain note. The
/// first two are invalid wherever they run -- a path in a name would let an
/// app write outside the staging folder -- and the note finds no sheet on a
/// computer.
#[test]
fn the_share_sheet_refuses_nothing_and_paths_and_a_computer_has_none() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(&wasm, include_bytes!("fixtures/phone-check.wasm")).expect("probe");
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"dev.krate.phone-check\"\nname = \"Phone Check\"\nversion = \"1.0.0\"\n\
         entry = \"code.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n\n\
         [[capabilities]]\ncap = \"ui.window:create\"\nrationale = \"window\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"io.stdout\"\nrationale = \"print\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"io.args\"\nrationale = \"args\"\nrequired = true\n",
    )
    .expect("manifest");
    let out = krate()
        .arg("run")
        .arg("--headless")
        .arg(&wasm)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--auto-grant")
        .arg("--")
        .arg("quick-share")
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout
            .lines()
            .any(|l| l == "share:empty=invalid path=invalid note=unavailable"),
        "nothing and a path are refused; a computer has no sheet: {stdout}"
    );
}

/// An app reaches a Bluetooth device only through the person's pick, and
/// only when allowed (krate:ui/bluetooth behind bluetooth.connect, IC-917).
///
/// `bluetooth-probe.wasm` is apps/krate-bluetooth-probe built ("Heart").
/// With KRATE_BLUETOOTH=heart a stand-in heart-rate strap is nearby: the
/// app picks it, subscribes to its measurement, reads a beat and the
/// battery. Not granted -- not even declared -- it hears not-allowed though
/// the strap is there. A computer with no Bluetooth says unavailable.
#[test]
fn an_app_reaches_a_bluetooth_device_only_when_allowed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(&wasm, include_bytes!("fixtures/bluetooth-probe.wasm")).expect("probe");
    let manifest = |with_bluetooth: bool| {
        let path = dir.path().join(if with_bluetooth {
            "with.toml"
        } else {
            "without.toml"
        });
        let bluetooth = if with_bluetooth {
            "[[capabilities]]\ncap = \"bluetooth.connect\"\nrationale = \"heart rate\"\nrequired = true\n\n"
        } else {
            ""
        };
        std::fs::write(
            &path,
            format!(
                "[app]\nid = \"dev.krate.bluetooth-probe\"\nname = \"Heart\"\nversion = \"1.0.0\"\n\
                 entry = \"code.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n\n\
                 [[capabilities]]\ncap = \"ui.window:create\"\nrationale = \"window\"\nrequired = true\n\n\
                 {bluetooth}\
                 [[capabilities]]\ncap = \"io.stdout\"\nrationale = \"print\"\nrequired = true\n\n\
                 [[capabilities]]\ncap = \"io.args\"\nrationale = \"args\"\nrequired = true\n"
            ),
        )
        .expect("manifest");
        path
    };
    let run = |manifest: &std::path::Path, strap: bool| -> String {
        let mut cmd = krate();
        cmd.arg("run")
            .arg("--headless")
            .arg(&wasm)
            .arg("--manifest")
            .arg(manifest)
            .arg("--auto-grant")
            .arg("--")
            .arg("quick")
            .env_remove("KRATE_BLUETOOTH");
        if strap {
            cmd.env("KRATE_BLUETOOTH", "heart");
        }
        let out = cmd.output().expect("run");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let with = manifest(true);
    let without = manifest(false);
    let picked = run(&with, true);
    assert!(
        picked
            .lines()
            .any(|l| l == "bluetooth:Test Heart subscribed=yes bpm=72 battery=87"),
        "a granted app picks the strap and reads it: {picked}"
    );
    let refused = run(&without, true);
    assert!(
        refused.lines().any(|l| l == "bluetooth:not-allowed"),
        "an app never granted bluetooth.connect hears not-allowed, strap or no strap: {refused}"
    );
    let computer = run(&with, false);
    assert!(
        computer.lines().any(|l| l == "bluetooth:unavailable"),
        "a computer with no Bluetooth says so: {computer}"
    );
}

/// An app learns where the phone is only when the person allowed it
/// (krate:ui/location behind location.read, IC-917).
///
/// `location-probe.wasm` is apps/krate-location-probe built ("Where Am I").
/// Granted, with KRATE_LOCATION standing in for a fix, it reads that fix
/// back. Not granted -- here, not even declared -- it hears not-allowed
/// even though a fix is there: the guard, not the device, decides. And a
/// computer without a fix says unavailable.
#[test]
fn an_app_learns_where_the_phone_is_only_when_allowed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(&wasm, include_bytes!("fixtures/location-probe.wasm")).expect("probe");
    let manifest = |with_location: bool| {
        let path = dir.path().join(if with_location {
            "with.toml"
        } else {
            "without.toml"
        });
        let location = if with_location {
            "[[capabilities]]\ncap = \"location.read\"\nrationale = \"position\"\nrequired = true\n\n"
        } else {
            ""
        };
        std::fs::write(
            &path,
            format!(
                "[app]\nid = \"dev.krate.location-probe\"\nname = \"Where Am I\"\nversion = \"1.0.0\"\n\
                 entry = \"code.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n\n\
                 [[capabilities]]\ncap = \"ui.window:create\"\nrationale = \"window\"\nrequired = true\n\n\
                 {location}\
                 [[capabilities]]\ncap = \"io.stdout\"\nrationale = \"print\"\nrequired = true\n\n\
                 [[capabilities]]\ncap = \"io.args\"\nrationale = \"args\"\nrequired = true\n"
            ),
        )
        .expect("manifest");
        path
    };
    let run = |manifest: &std::path::Path, fix: Option<&str>| -> String {
        let mut cmd = krate();
        cmd.arg("run")
            .arg("--headless")
            .arg(&wasm)
            .arg("--manifest")
            .arg(manifest)
            .arg("--auto-grant")
            .arg("--")
            .arg("quick")
            .env_remove("KRATE_LOCATION");
        if let Some(fix) = fix {
            cmd.env("KRATE_LOCATION", fix);
        }
        let out = cmd.output().expect("run");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let with = manifest(true);
    let without = manifest(false);
    let granted = run(&with, Some("1.29027,103.85196,12"));
    assert!(
        granted
            .lines()
            .any(|l| l == "location:1.29027,103.85196 within 12 m"),
        "a granted app reads the fix: {granted}"
    );
    let refused = run(&without, Some("1.29027,103.85196,12"));
    assert!(
        refused.lines().any(|l| l == "location:not-allowed"),
        "an app never granted location.read hears not-allowed, fix or no fix: {refused}"
    );
    let computer = run(&with, None);
    assert!(
        computer.lines().any(|l| l == "location:unavailable"),
        "a computer with no fix says so: {computer}"
    );
}

/// An app reads how the phone is held, and a computer says it cannot
/// (krate:ui/motion, IC-917).
///
/// `motion-probe.wasm` is apps/krate-motion-probe built ("Level"). On a
/// computer `available` is false. Headless with KRATE_MOTION standing in
/// for a phone tilted so, the app reads back exactly that gravity: the
/// reading reaches the app whole, axis for axis.
#[test]
fn an_app_reads_how_the_phone_is_held_and_a_computer_says_it_cannot() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(&wasm, include_bytes!("fixtures/motion-probe.wasm")).expect("probe");
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"dev.krate.motion-probe\"\nname = \"Level\"\nversion = \"1.0.0\"\n\
         entry = \"code.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n\n\
         [[capabilities]]\ncap = \"ui.window:create\"\nrationale = \"window\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"io.stdout\"\nrationale = \"print\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"io.args\"\nrationale = \"args\"\nrequired = true\n",
    )
    .expect("manifest");
    let run = |motion: Option<&str>| -> String {
        let mut cmd = krate();
        cmd.arg("run")
            .arg("--headless")
            .arg(&wasm)
            .arg("--manifest")
            .arg(&manifest)
            .arg("--auto-grant")
            .arg("--")
            .arg("quick")
            .env_remove("KRATE_MOTION");
        if let Some(motion) = motion {
            cmd.env("KRATE_MOTION", motion);
        }
        let out = cmd.output().expect("run");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let computer = run(None);
    assert!(
        computer.lines().any(|l| l == "motion:available=no"),
        "a computer has no sensors and says so: {computer}"
    );
    let tilted = run(Some("0.25,-0.5,-0.83"));
    assert!(
        tilted
            .lines()
            .any(|l| l == "motion:gravity=0.25,-0.50,-0.83 heading=0"),
        "the reading reaches the app axis for axis: {tilted}"
    );
}

/// An app learns what kind of screen it is on (krate:ui/screen, IC-912).
///
/// `pointer-probe.wasm` prints what `krate::layout::Screen::of` reads. A
/// headless window is a desktop one: a pointer, nothing covered. Given a
/// phone's screen it is touched, its width or height is compact, and the
/// notch, home bar and Krate's close button cover its edges -- the numbers
/// an app keeps its text and controls clear of. A host that answered
/// "pointer" everywhere would leave every phone app with 28-point targets
/// under a finger.
#[test]
fn an_app_learns_what_kind_of_screen_it_is_on() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(&wasm, include_bytes!("fixtures/pointer-probe.wasm")).expect("probe");
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"com.acme.pointer-probe\"\nname = \"pointer-probe\"\nversion = \"1.0.0\"\n\
         entry = \"code.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n\n\
         [[capabilities]]\ncap = \"ui.window:create\"\nrationale = \"window\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"io.stdout\"\nrationale = \"print\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"io.args\"\nrationale = \"args\"\nrequired = true\n",
    )
    .expect("manifest");
    let screen_line = |screen: Option<&str>| -> String {
        let mut cmd = krate();
        cmd.arg("run")
            .arg("--headless")
            .arg(&wasm)
            .arg("--manifest")
            .arg(&manifest)
            .arg("--auto-grant");
        if let Some(screen) = screen {
            cmd.env("KRATE_SCREEN", screen);
        }
        let out = cmd.output().expect("run");
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        stdout
            .lines()
            .find(|line| line.starts_with("screen="))
            .unwrap_or_else(|| {
                panic!(
                    "no screen line in: {stdout}{}",
                    String::from_utf8_lossy(&out.stderr)
                )
            })
            .to_string()
    };
    assert_eq!(
        screen_line(None),
        "screen=480x320 width=compact height=compact input=pointer safe=0,0,0,0",
        "a headless window is a desktop one"
    );
    assert_eq!(
        screen_line(Some("390x844")),
        "screen=390x844 width=compact height=medium input=touch safe=89,0,34,0",
        "a phone held upright"
    );
    assert_eq!(
        screen_line(Some("844x390")),
        "screen=844x390 width=medium height=compact input=touch safe=50,47,21,47",
        "a phone turned sideways"
    );
}

/// The binary a person runs measures a frame on a phone's screen (IC-911).
///
/// `weather-card.wasm` is apps/krate-weather built: it lays out from its
/// real size. `weather-desktop-only.wasm` is the same app sabotaged to lay
/// out for 1280 points whatever the screen. Given a phone's screen
/// (`KRATE_SCREEN`), the first fits and the second cuts its day cards at
/// the side -- and on a desktop both fit, so the phone is what tells them
/// apart. Without the screen override every run would be judged on the
/// size the app asked for, and a phone would never be seen.
#[test]
fn a_frame_is_measured_on_a_phone_screen_and_a_desktop_only_layout_is_cut() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"com.acme.weather-card\"\nname = \"weather-card\"\nversion = \"1.0.0\"\n\
         entry = \"code.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n\n\
         [[capabilities]]\ncap = \"ui.window:create\"\nrationale = \"window\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"io.stdout\"\nrationale = \"print\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"io.args\"\nrationale = \"args\"\nrequired = true\n",
    )
    .expect("manifest");
    let measure = |bytes: &[u8], name: &str, screen: &str| -> serde_json::Value {
        let wasm = dir.path().join(format!("{name}.wasm"));
        std::fs::write(&wasm, bytes).expect("component");
        std::fs::copy(&wasm, dir.path().join("code.wasm")).expect("entry");
        let report = dir.path().join(format!("{name}-{screen}.json"));
        let frame = dir.path().join(format!("{name}-{screen}.png"));
        let out = krate()
            .arg("run")
            .arg(dir.path().join("code.wasm"))
            .arg("--manifest")
            .arg(&manifest)
            .args(["--auto-grant", "--shoot"])
            .arg(&frame)
            .arg("--check-layout")
            .env("KRATE_SCREEN", screen)
            .env("KRATE_FIT_REPORT", &report)
            .output()
            .expect("run");
        assert!(
            out.status.success(),
            "{name} on {screen}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let text = std::fs::read_to_string(&report)
            .unwrap_or_else(|_| panic!("{name} on {screen} wrote no screen report"));
        serde_json::from_str(&text).expect("report is JSON")
    };
    let fits = include_bytes!("fixtures/weather-card.wasm");
    let desktop_only = include_bytes!("fixtures/weather-desktop-only.wasm");
    assert_ne!(
        &fits[..],
        &desktop_only[..],
        "the sabotage must be a different app"
    );

    let phone = measure(fits, "fits", "390x844");
    assert_eq!(
        phone["facts"]["screen"],
        serde_json::json!([390.0, 844.0]),
        "{phone}"
    );
    assert_eq!(
        phone["fixed"],
        serde_json::json!(false),
        "it lays out from its real size: {phone}"
    );
    assert!(phone["facts"]["texts"].as_u64().unwrap_or(0) > 0, "{phone}");
    assert_eq!(phone["facts"]["off_edge"], serde_json::json!([]), "{phone}");
    assert!(
        phone["facts"]["smallest_text"][0].as_f64().unwrap_or(0.0) >= 13.0,
        "{phone}"
    );

    let cut = measure(desktop_only, "desktop-only", "390x844");
    let off = cut["facts"]["off_edge"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        !off.is_empty(),
        "a 1280-point layout on a phone is cut at the side: {cut}"
    );
    assert!(
        off.iter().any(|entry| entry[0]
            .as_str()
            .unwrap_or("")
            .starts_with("the box holding ")),
        "the cut is named by what it holds: {cut}"
    );

    let desk = measure(desktop_only, "desktop-only", "1280x800");
    assert_eq!(
        desk["facts"]["off_edge"],
        serde_json::json!([]),
        "on a desktop it fits: {desk}"
    );
}

/// A game asks about the mouse through the binary a person runs
/// (krate:ui/pointer, IC-909).
///
/// `pointer-probe.wasm` is apps/krate-pointer-probe built. Headless there is
/// no pointer: it must be nowhere, hold nothing, move nothing, and refuse to
/// be captured with a reason -- never a crash, and never a capture that
/// claims to hold a pointer that does not exist.
#[test]
fn an_app_asks_about_the_mouse_and_a_headless_capture_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(&wasm, include_bytes!("fixtures/pointer-probe.wasm")).expect("probe");
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"com.acme.pointer-probe\"\nname = \"pointer-probe\"\nversion = \"1.0.0\"\n\
         entry = \"code.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n\n\
         [[capabilities]]\ncap = \"ui.window:create\"\nrationale = \"window\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"io.stdout\"\nrationale = \"print\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"io.args\"\nrationale = \"args\"\nrequired = true\n",
    )
    .expect("manifest");
    let bundle = dir.path().join("pointer.krate");
    assert!(krate()
        .arg("pack")
        .arg(&wasm)
        .arg("--manifest")
        .arg(&manifest)
        .arg("-o")
        .arg(&bundle)
        .status()
        .expect("pack")
        .success());
    let out = krate()
        .arg("run")
        .arg("--headless")
        .arg(&bundle)
        .arg("--auto-grant")
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "the interface links and answers: {stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = stdout
        .lines()
        .find(|line| line.starts_with("position="))
        .unwrap_or_else(|| panic!("no report line in: {stdout}"));
    assert!(
        report.starts_with(
            "position=none primary=up secondary=up middle=up motion=0,0 captured=false touches=0 keyboard=false capture=refused: "
        ),
        "headless: nowhere, nothing held, nothing moved, capture refused with a reason: {report}"
    );
    assert!(stdout.contains("released captured=false"), "{stdout}");
    // krate:gfx/canvas-images: a kept picture draws both ways; a short
    // buffer, an unknown canvas and a forgotten handle are refused.
    assert!(
        stdout.lines().any(
            |l| l == "kept=ok draw=ok round=ok short=refused unknown=refused forgotten=refused"
        ),
        "{stdout}"
    );
}

/// An app speaks through the binary a person runs (krate:speech/synthesis,
/// the capability roadmap's first Tier 1 item).
///
/// `say-probe.wasm` is apps/krate-say-probe built. `say` starts a line,
/// waits to hear it start, stops it, and reports -- so the whole contract
/// runs in a fraction of a second of sound. A computer with no voice
/// (Linux without espeak-ng) answers `unsupported`, never a crash, and a
/// line never keeps sounding after `stop`.
#[test]
fn an_app_speaks_a_line_and_stops_when_told() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(&wasm, include_bytes!("fixtures/say-probe.wasm")).expect("probe");
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"com.acme.say-probe\"\nname = \"say-probe\"\nversion = \"1.0.0\"\n\
         entry = \"code.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n\n\
         [[capabilities]]\ncap = \"io.stdout\"\nrationale = \"print\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"io.args\"\nrationale = \"args\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"audio.playback\"\nrationale = \"speak\"\nrequired = false\n",
    )
    .expect("manifest");
    let bundle = dir.path().join("say.krate");
    assert!(krate()
        .arg("pack")
        .arg(&wasm)
        .arg("--manifest")
        .arg(&manifest)
        .arg("-o")
        .arg(&bundle)
        .status()
        .expect("pack")
        .success());
    let ask = |words: &[&str]| {
        let out = krate()
            .arg("run")
            .arg(&bundle)
            .arg("--auto-grant")
            .arg("--")
            .args(words)
            .output()
            .expect("run");
        assert!(
            out.status.success(),
            "every answer is an answer, not a crash: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };

    assert!(
        ask(&["rate", "3"]).starts_with("error=invalid-request"),
        "a rate outside 0.5 to 2.0 is refused the same way everywhere"
    );
    // How far ahead a stream is (K-1011): a stream that does not exist is
    // an answer, not a trap -- which also proves the interface links.
    assert_eq!(ask(&["queue"]), "queue=invalid-stream");
    let voices = ask(&["voices"]);
    let said = ask(&["say", "Krate can talk"]);
    let no_voice = |line: &str| line.starts_with("error=unsupported");
    if cfg!(target_os = "macos") {
        // Every Mac ships voices, and this is the platform the speech was
        // measured on: it must really speak here.
        assert!(
            voices
                .strip_prefix("voices=")
                .and_then(|n| n.parse::<u32>().ok())
                > Some(0),
            "{voices}"
        );
        assert_eq!(said, "said=started,stopped");
    } else {
        assert!(
            voices.starts_with("voices=") || no_voice(&voices),
            "{voices}"
        );
        assert!(
            said == "said=started,stopped" || no_voice(&said),
            "a line either speaks and stops, or there is no voice: {said}"
        );
    }
}

/// An app can ask for more memory, and gets it only when granted (K-416;
/// the capability survey's item 3).
///
/// `memory-probe.wasm` is apps/krate-memory-probe built: it asks for N MiB
/// with `try_reserve_exact` and prints `held=N` or `refused=N`. Every case
/// ends in an answer, never a crash -- the half K-395 fixed.
#[test]
fn an_app_that_declares_more_memory_gets_it_only_when_granted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(&wasm, include_bytes!("fixtures/memory-probe.wasm")).expect("probe");
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"com.acme.memory-probe\"\nname = \"memory-probe\"\nversion = \"1.0.0\"\n\
         entry = \"code.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n\n\
         [[capabilities]]\ncap = \"io.stdout\"\nrationale = \"print\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"io.args\"\nrationale = \"args\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"memory.budget:1024\"\nrationale = \"hold a picture\"\nrequired = false\n",
    )
    .expect("manifest");
    let bundle = dir.path().join("probe.krate");
    assert!(krate()
        .arg("pack")
        .arg(&wasm)
        .arg("--manifest")
        .arg(&manifest)
        .arg("-o")
        .arg(&bundle)
        .status()
        .expect("pack")
        .success());
    let ask = |extra: &[&str], mib: &str| {
        let out = krate()
            .arg("run")
            .arg(&bundle)
            .args(extra)
            .args(["--", mib])
            .output()
            .expect("run");
        assert!(
            out.status.success(),
            "every answer is an answer, not a crash: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };

    assert_eq!(
        ask(&["--grant", "io.stdout", "--grant", "io.args"], "400"),
        "refused=400",
        "the fixture must really need more than the default 256 MiB"
    );
    assert_eq!(
        ask(&["--auto-grant"], "400"),
        "held=400",
        "the declared budget, granted, is what the app gets"
    );
    assert_eq!(
        ask(&["--auto-grant"], "1100"),
        "refused=1100",
        "and no more than it declared"
    );
    assert_eq!(
        ask(&["--auto-grant", "--mem-limit", "200"], "400"),
        "refused=400",
        "an explicit --mem-limit wins over the declaration"
    );
}

/// An operator's --grant cannot hand an app a capability its manifest never
/// declared (K-870, IC-733). K-387 closed this on the MCP path; the command
/// line kept the unchecked parser, and an app with its store declaration
/// deleted saved 11 of 11 times under `--grant store.kv`.
#[test]
fn a_grant_the_manifest_never_declared_is_refused_on_the_command_line() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(&wasm, include_bytes!("fixtures/memory-probe.wasm")).expect("probe");
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"com.acme.plain\"\nname = \"plain\"\nversion = \"1.0.0\"\n\
         entry = \"code.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n\n\
         [[capabilities]]\ncap = \"io.stdout\"\nrationale = \"print\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"io.args\"\nrationale = \"args\"\nrequired = true\n",
    )
    .expect("manifest");
    let bundle = dir.path().join("plain.krate");
    assert!(krate()
        .arg("pack")
        .arg(&wasm)
        .arg("--manifest")
        .arg(&manifest)
        .arg("-o")
        .arg(&bundle)
        .status()
        .expect("pack")
        .success());

    let refused = krate()
        .arg("run")
        .arg(&bundle)
        .args([
            "--grant",
            "io.stdout",
            "--grant",
            "io.args",
            "--grant",
            "store.kv",
            "--",
            "10",
        ])
        .output()
        .expect("run");
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        !refused.status.success(),
        "an undeclared grant must stop the run: {stderr}"
    );
    assert!(
        stderr.contains("store.kv") && stderr.contains("not declared"),
        "and say which capability and why: {stderr}"
    );

    // The declared ones alone still run -- the check is not a blanket refusal.
    let fine = krate()
        .arg("run")
        .arg(&bundle)
        .args(["--grant", "io.stdout", "--grant", "io.args", "--", "10"])
        .output()
        .expect("run");
    assert!(
        fine.status.success(),
        "{}",
        String::from_utf8_lossy(&fine.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&fine.stdout).trim(), "held=10");
}

/// A tiny HTTP server for the big-file path's tests (IC-945): every request
/// goes to `handle(method, path, headers, body)`, which answers
/// `(status, extra headers, body)`. One request per connection.
#[allow(clippy::type_complexity)]
fn spawn_fake_hub(
    handle: std::sync::Arc<
        dyn Fn(&str, &str, &[(String, String)], &[u8]) -> (u16, Vec<(String, String)>, Vec<u8>)
            + Send
            + Sync,
    >,
) -> Option<(SocketAddr, std::sync::Arc<std::sync::atomic::AtomicBool>)> {
    let listener = bind_local_fixture_listener("fake hub")?;
    let addr = listener.local_addr().expect("addr");
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stopping = stop.clone();
    listener.set_nonblocking(true).expect("nonblocking");
    thread::spawn(move || {
        while !stopping.load(std::sync::atomic::Ordering::SeqCst) {
            let stream = match listener.accept() {
                Ok((stream, _)) => stream,
                Err(err) if err.kind() == ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(_) => break,
            };
            let handle = handle.clone();
            thread::spawn(move || {
                let mut stream = stream;
                stream.set_nonblocking(false).ok();
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    if stream.read(&mut byte).unwrap_or(0) == 0 {
                        return;
                    }
                    head.push(byte[0]);
                }
                let head = String::from_utf8_lossy(&head).to_string();
                let mut lines = head.split("\r\n");
                let mut first = lines.next().unwrap_or("").split(' ');
                let method = first.next().unwrap_or("").to_string();
                let path = first.next().unwrap_or("").to_string();
                let headers: Vec<(String, String)> = lines
                    .filter_map(|l| l.split_once(':'))
                    .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
                    .collect();
                let length: usize = headers
                    .iter()
                    .find(|(k, _)| k == "content-length")
                    .and_then(|(_, v)| v.parse().ok())
                    .unwrap_or(0);
                let mut body = vec![0u8; length];
                if stream.read_exact(&mut body).is_err() {
                    return;
                }
                let (status, extra, out) = handle(&method, &path, &headers, &body);
                let mut response = format!(
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n",
                    out.len()
                );
                for (k, v) in extra {
                    response.push_str(&format!("{k}: {v}\r\n"));
                }
                response.push_str("\r\n");
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.write_all(&out);
            });
        }
    });
    Some((addr, stop))
}

/// A bundle with `noise_bytes` of incompressible asset, packed by the CLI.
fn noisy_bundle(dir: &std::path::Path, noise_bytes: usize, seed: u64) -> PathBuf {
    let wasm = dir.join("code.wasm");
    std::fs::write(
        &wasm,
        include_bytes!("../../bundle/tests/fixtures/minimal-run.wasm"),
    )
    .expect("component");
    let manifest = dir.join("manifest.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"dev.krate.bigpub\"\nname = \"Big Pub\"\nversion = \"1.0.0\"\n\
         entry = \"code.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n",
    )
    .expect("manifest");
    let assets = dir.join("assets");
    std::fs::create_dir_all(&assets).expect("assets");
    let mut x = seed | 1;
    let noise: Vec<u8> = (0..noise_bytes)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect();
    std::fs::write(assets.join("noise.bin"), noise).expect("noise");
    let bundle = dir.join("bigpub.krate");
    let packed = krate()
        .arg("pack")
        .arg(&wasm)
        .arg("--manifest")
        .arg(&manifest)
        .arg("-o")
        .arg(&bundle)
        .output()
        .expect("pack");
    assert!(
        packed.status.success(),
        "{}",
        String::from_utf8_lossy(&packed.stderr)
    );
    bundle
}

/// `krate publish` sends an app over the publish door's 5 MiB through the
/// big-file path (IC-945): started, sent in parts, finished, and the hub's
/// verdict waited for -- and a publish cut off part way resumes, sending
/// only the parts still missing.
#[test]
fn a_big_app_is_published_in_parts_and_a_cut_off_publish_resumes() {
    use std::sync::{Arc, Mutex};
    let dir = tempfile::tempdir().expect("tempdir");
    let home = tempfile::tempdir().expect("home");
    std::fs::create_dir_all(home.path().join(".krate")).expect("krate home");
    std::fs::write(
        home.path().join(".krate/github.json"),
        r#"{"login":"alice","name":"Alice","avatar_url":null,"token":"krs_alice"}"#,
    )
    .expect("sign-in");
    let bundle = noisy_bundle(dir.path(), 12 * 1024 * 1024, 7);
    let bytes = std::fs::read(&bundle).expect("bundle");
    let sha = format!("{:x}", Sha256::digest(&bytes));
    let part_size = 5 * 1024 * 1024usize;
    let parts = bytes.len().div_ceil(part_size);

    #[derive(Default)]
    struct Hub {
        starts: usize,
        door: usize,
        sent: Vec<usize>,
        stored: std::collections::BTreeMap<usize, Vec<u8>>,
        fail_part: Option<usize>,
        finished: bool,
    }
    let hub = Arc::new(Mutex::new(Hub {
        fail_part: Some(2),
        ..Default::default()
    }));
    let state = hub.clone();
    let sha_for_hub = sha.clone();
    let Some((addr, stop)) = spawn_fake_hub(Arc::new(move |method, path, headers, body| {
        let mut hub = state.lock().unwrap();
        let header = |name: &str| {
            headers
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        let ok = |json: serde_json::Value| (200, vec![], json.to_string().into_bytes());
        match (method, path) {
            ("POST", "/publish") => {
                hub.door += 1;
                (413, vec![], b"bundle too large".to_vec())
            }
            ("POST", "/publish/big/start") => {
                hub.starts += 1;
                assert_eq!(header("authorization"), "Bearer krs_alice");
                let ask: serde_json::Value = serde_json::from_slice(body).unwrap();
                assert_eq!(ask["sha256"], sha_for_hub.as_str());
                assert_eq!(ask["kind"], "bundle");
                assert_eq!(ask["name"], "Big Pub");
                ok(serde_json::json!({
                    "id": "upload0001", "parts": parts, "part_size": part_size, "token": "tok"
                }))
            }
            ("PUT", p) if p.starts_with("/publish/big/upload0001/part/") => {
                assert_eq!(header("x-krate-upload"), "tok");
                let n: usize = p.rsplit('/').next().unwrap().parse().unwrap();
                hub.sent.push(n);
                if hub.fail_part == Some(n) {
                    return (503, vec![], b"busy".to_vec());
                }
                hub.stored.insert(n, body.to_vec());
                ok(serde_json::json!({ "part": n, "etag": format!("e{n}") }))
            }
            ("POST", "/publish/big/upload0001/finish") => {
                let ask: serde_json::Value = serde_json::from_slice(body).unwrap();
                assert_eq!(ask["parts"].as_array().unwrap().len(), parts);
                hub.finished = true;
                (
                    202,
                    vec![],
                    serde_json::json!({ "status": "judging" })
                        .to_string()
                        .into_bytes(),
                )
            }
            ("GET", "/publish/big/upload0001") => ok(serde_json::json!({
                "status": "listed", "url": format!("https://hub.example/a/{}", &sha_for_hub[..12])
            })),
            _ => (404, vec![], b"not found".to_vec()),
        }
    })) else {
        eprintln!("skipping a_big_app_is_published_in_parts_and_a_cut_off_publish_resumes: no local port for the fake hub");
        return;
    };
    let publish = || {
        krate()
            .arg("publish")
            .arg(&bundle)
            .args(["--hub", &format!("http://{addr}")])
            .args(["--shot", "/nonexistent.png"])
            .env("HOME", home.path())
            .output()
            .expect("publish")
    };

    // Part 2 fails three times: the publish stops and says to run it again.
    let first = publish();
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(!first.status.success(), "{said}");
    assert!(said.contains("publish again"), "{said}");
    {
        let hub = hub.lock().unwrap();
        assert_eq!(
            hub.door, 0,
            "an app over 5 MiB never goes to the one-request door"
        );
        assert_eq!(hub.sent, vec![1, 2, 2, 2]);
    }

    // Run again: no new start, only the missing parts, then listed.
    hub.lock().unwrap().fail_part = None;
    let second = publish();
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&second.stderr)
    );
    assert!(second.status.success(), "{said}");
    assert!(
        said.contains(&format!("krate run https://hub.example/a/{}", &sha[..12])),
        "{said}"
    );
    let hub = hub.lock().unwrap();
    assert_eq!(hub.starts, 1, "the resumed publish did not start over");
    assert_eq!(
        &hub.sent[4..],
        &(2..=parts).collect::<Vec<_>>()[..],
        "only the missing parts"
    );
    assert!(hub.finished);
    let assembled: Vec<u8> = hub.stored.values().flatten().copied().collect();
    assert_eq!(assembled, bytes, "the parts are the bundle, in order");
    assert!(
        !home
            .path()
            .join(format!(".krate/uploads/{sha}-bundle.json"))
            .exists(),
        "a finished upload forgets its resume file"
    );
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// `krate run <link>` to a big-tier app (IC-945): the hub says
/// `x-krate-tier: big`, so the app is saved to a file with its progress
/// shown, held to its address, and opened as a file on this desktop -- and
/// a second run does not download it again.
#[test]
fn a_big_tier_link_is_saved_to_a_file_and_run_from_it() {
    use std::sync::{atomic::AtomicUsize, Arc};
    let dir = tempfile::tempdir().expect("tempdir");
    let cache = tempfile::tempdir().expect("cache");
    let bundle = noisy_bundle(dir.path(), 64 * 1024, 3);
    let bytes = Arc::new(std::fs::read(&bundle).expect("bundle"));
    let sha = format!("{:x}", Sha256::digest(&bytes[..]));
    let gets = Arc::new(AtomicUsize::new(0));
    let (served, counted, path_sha) = (bytes.clone(), gets.clone(), sha.clone());
    let Some((addr, stop)) = spawn_fake_hub(Arc::new(move |_method, path, _headers, _body| {
        if path == format!("/a/{path_sha}") {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            (
                200,
                vec![("x-krate-tier".into(), "big".into())],
                served.to_vec(),
            )
        } else if path == "/a/0000000000000000" {
            (
                200,
                vec![("x-krate-tier".into(), "big".into())],
                served.to_vec(),
            )
        } else {
            (404, vec![], b"not found".to_vec())
        }
    })) else {
        eprintln!("skipping a_big_tier_link_is_saved_to_a_file_and_run_from_it: no local port for the fake hub");
        return;
    };
    let run = |path: &str| {
        krate()
            .arg("run")
            .arg(format!("http://{addr}{path}"))
            .args(["--insecure-http", "--auto-grant"])
            .env("KRATE_CACHE_DIR", cache.path())
            .output()
            .expect("run")
    };
    let first = run(&format!("/a/{sha}"));
    let said = String::from_utf8_lossy(&first.stderr).to_string();
    assert!(
        first.status.success(),
        "{said}{}",
        String::from_utf8_lossy(&first.stdout)
    );
    assert!(said.contains("downloading:"), "progress is shown: {said}");
    let saved = cache.path().join("downloads").join(format!("{sha}.krate"));
    assert_eq!(
        std::fs::read(&saved).expect("saved"),
        *bytes,
        "saved under its address"
    );
    assert!(
        said.contains(&saved.display().to_string()),
        "says where it is: {said}"
    );

    let written = std::fs::metadata(&saved)
        .and_then(|m| m.modified())
        .expect("mtime");
    std::thread::sleep(Duration::from_millis(20));
    let second = run(&format!("/a/{sha}"));
    assert!(second.status.success());
    assert_eq!(
        std::fs::metadata(&saved)
            .and_then(|m| m.modified())
            .expect("mtime"),
        written,
        "the saved copy was used, not downloaded again"
    );
    // The second run asked (one request), and read the saved file rather
    // than a second download: the fake answers in full either way, so the
    // proof is that nothing was written again.
    assert_eq!(gets.load(std::sync::atomic::Ordering::SeqCst), 2);

    // Bytes that are not the ones the address names do not run.
    let bad = run("/a/0000000000000000");
    assert!(!bad.status.success());
    assert!(
        String::from_utf8_lossy(&bad.stderr).contains("not the ones it names"),
        "{}",
        String::from_utf8_lossy(&bad.stderr)
    );
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// `krate judge` gives the hub's own answer (IC-945): for the publish
/// door, exactly what the wasm validator in the Worker answers (the same
/// function builds both); for the big tier, the same checks at its own
/// ceilings, so the build service can judge what the Worker cannot hold.
#[test]
fn krate_judge_answers_as_the_hub_does_at_either_tier() {
    use std::io::Write as _;
    let dir = tempfile::tempdir().expect("tempdir");
    let manifest_text =
        "[app]\nid = \"dev.krate.judged\"\nname = \"Judged\"\nversion = \"1.0.0\"\n\
                         entry = \"code.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n";
    let component = include_bytes!("../../bundle/tests/fixtures/minimal-run.wasm");
    let bundle_with = |name: &str, extra: usize| {
        let path = dir.path().join(name);
        let file = std::fs::File::create(&path).expect("create");
        let mut writer = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer.start_file("manifest.toml", opts).expect("m");
        writer.write_all(manifest_text.as_bytes()).expect("w");
        writer.start_file("code.wasm", opts).expect("c");
        writer.write_all(component).expect("w");
        for i in 0..extra {
            writer
                .start_file(format!("assets/a{i}.ron"), opts)
                .expect("a");
            writer.write_all(b"()").expect("w");
        }
        writer.finish().expect("finish");
        path
    };
    let judge = |path: &std::path::Path, tier: &str| {
        let out = krate()
            .arg("judge")
            .arg(path)
            .args(["--tier", tier])
            .output()
            .expect("judge");
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        )
    };

    // The door's answer, byte for byte.
    let small = bundle_with("small.krate", 1);
    let bytes = std::fs::read(&small).expect("read");
    let door = krate_bundle::judgement_json(krate_bundle::judge_bytes(
        &bytes,
        krate_bundle::HUB_MAX_EXPANDED_BYTES,
    ));
    let (code, said) = judge(&small, "hub");
    assert_eq!(code, Some(0), "{said}");
    assert_eq!(said, door, "krate judge --tier hub is the door's answer");

    // Past the door's entry count: the door refuses, the big tier judges.
    let crowd = bundle_with("crowd.krate", krate_bundle::HUB_LIMITS.entry_count);
    let (code, said) = judge(&crowd, "hub");
    assert_eq!(code, Some(1));
    let answer: serde_json::Value = serde_json::from_str(&said).expect("json");
    assert_eq!(answer["ok"], false, "{said}");
    let (code, said) = judge(&crowd, "hub-big");
    assert_eq!(code, Some(0), "{said}");
    let answer: serde_json::Value = serde_json::from_str(&said).expect("json");
    assert_eq!(answer["ok"], true);
    assert_eq!(
        answer["judgement"]["manifest"]["app"]["id"],
        "dev.krate.judged"
    );

    // Past the big tier's size: refused from the file's size, unread (a
    // sparse file costs no disk).
    let huge = dir.path().join("huge.krate");
    std::fs::File::create(&huge)
        .expect("create")
        .set_len(krate_bundle::HUB_BIG_LIMITS.bundle_bytes + 1)
        .expect("size");
    let (code, said) = judge(&huge, "hub-big");
    assert_eq!(code, Some(1));
    assert!(
        said.contains(&krate_bundle::HUB_BIG_LIMITS.bundle_bytes.to_string()),
        "names the big tier's ceiling: {said}"
    );
}

/// A large bundle is unpacked once and reused (IC-944): the second run of
/// the same file writes nothing again, and nothing of it lands in the temp
/// folder, where a killed run used to leave every byte (687 MB for Veloren).
#[test]
fn a_large_bundle_is_unpacked_once_and_reused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cache = tempfile::tempdir().expect("cache");
    let tmp = tempfile::tempdir().expect("tmp");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(
        &wasm,
        include_bytes!("../../bundle/tests/fixtures/minimal-run.wasm"),
    )
    .expect("component");
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"dev.krate.large\"\nname = \"large\"\nversion = \"1.0.0\"\n\
         entry = \"code.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n",
    )
    .expect("manifest");
    // Nine MiB that deflate cannot shrink, so the bundle is over the
    // cache's 8 MiB floor.
    let assets = dir.path().join("assets");
    std::fs::create_dir_all(&assets).expect("assets");
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let noise: Vec<u8> = (0..9 * 1024 * 1024)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect();
    std::fs::write(assets.join("noise.bin"), &noise).expect("noise");
    let bundle = dir.path().join("large.krate");
    let packed = krate()
        .arg("pack")
        .arg(&wasm)
        .arg("--manifest")
        .arg(&manifest)
        .arg("-o")
        .arg(&bundle)
        .output()
        .expect("pack");
    assert!(
        packed.status.success(),
        "{}",
        String::from_utf8_lossy(&packed.stderr)
    );
    assert!(std::fs::metadata(&bundle).expect("meta").len() >= 8 * 1024 * 1024);

    let run = || {
        let out = krate()
            .arg("run")
            .arg(&bundle)
            .arg("--auto-grant")
            .env("KRATE_CACHE_DIR", cache.path())
            .env("TMPDIR", tmp.path())
            .output()
            .expect("run");
        assert!(
            out.status.success(),
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    };
    let unpacked_noise = || {
        let opened = cache.path().join("opened");
        let entries: Vec<_> = std::fs::read_dir(&opened)
            .expect("opened")
            .flatten()
            .filter(|e| e.path().join(".krate-opened.json").is_file())
            .collect();
        assert_eq!(entries.len(), 1, "one entry for one bundle");
        entries[0].path().join("assets/noise.bin")
    };
    let in_tmp = || {
        std::fs::read_dir(tmp.path())
            .expect("tmp")
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("krate-open-"))
            .count()
    };

    run();
    let noise_file = unpacked_noise();
    assert_eq!(std::fs::read(&noise_file).expect("noise"), noise);
    let written = std::fs::metadata(&noise_file)
        .and_then(|m| m.modified())
        .expect("mtime");
    assert_eq!(in_tmp(), 0, "nothing unpacked into the temp folder");

    run();
    assert_eq!(unpacked_noise(), noise_file, "the same entry");
    assert_eq!(
        std::fs::metadata(&noise_file)
            .and_then(|m| m.modified())
            .expect("mtime"),
        written,
        "the second run did not unpack again"
    );
    assert_eq!(in_tmp(), 0);
}

/// An installed app's own files live outside its `.app` and survive
/// installing it again (IC-943, K-1076).
///
/// The app is opened the way a double-click opens it: its own executable,
/// no arguments. It writes probe-dir/hello.txt. Before IC-943 that landed in
/// `Contents/Resources`, inside the signed bundle: `codesign -v` failed, and
/// the next install swapped in a fresh `.app` and the file was gone. A file
/// an older Krate left in the bundle is moved out on the next open.
///
/// The consent window must never appear in a test, so the grants are
/// remembered the way a person's "Open" would remember them, and a plain
/// `krate run` (which refuses instead of asking) proves they cover the app
/// before the double-click path is tried.
#[cfg(target_os = "macos")]
#[test]
fn an_installed_apps_files_live_outside_it_and_survive_reinstall() {
    let dir = tempfile::tempdir().expect("tempdir");
    let prefix = tempfile::tempdir().expect("prefix");
    let home = tempfile::tempdir().expect("home");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(&wasm, include_bytes!("fixtures/cxx-probe.wasm")).expect("probe");
    let required = [
        "io.stdout",
        "io.args",
        "fs.read:./probe-dir/**",
        "fs.write:./probe-dir/**",
        "fs.list:./probe-dir/**",
    ];
    let mut manifest_text = "[app]\nid = \"com.acme.keeps-files\"\nname = \"keeps-files\"\n\
         version = \"1.0.0\"\nentry = \"code.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n"
        .to_string();
    for cap in required {
        manifest_text.push_str(&format!(
            "\n[[capabilities]]\ncap = \"{cap}\"\nrationale = \"probe\"\nrequired = true\n"
        ));
    }
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(&manifest, manifest_text).expect("manifest");
    let bundle = dir.path().join("keeps-files.krate");
    assert!(krate()
        .arg("pack")
        .arg(&wasm)
        .arg("--manifest")
        .arg(&manifest)
        .arg("-o")
        .arg(&bundle)
        .status()
        .expect("pack")
        .success());

    // What one "Open" in the consent window remembers, for this machine's
    // own unsigned app (IC-737).
    let keys = home.path().join(".krate/keys");
    std::fs::create_dir_all(&keys).expect("keys");
    std::fs::write(keys.join("isolate-development"), "").expect("isolate");
    let developer = "0123456789abcdef0123456789abcdef";
    std::fs::write(keys.join("development-id"), developer).expect("id");
    let principal = format!("com.acme.keeps-files+{}", &developer[..16]);
    let grants = home.path().join(".krate/grants");
    std::fs::create_dir_all(&grants).expect("grants");
    std::fs::write(
        grants.join(format!("{principal}.json")),
        format!(
            "{{\"schema\":\"krate.grants.v1\",\"principal\":\"{principal}\",\"granted\":[{}]}}",
            required
                .iter()
                .map(|cap| format!("\"{cap}\""))
                .collect::<Vec<_>>()
                .join(",")
        ),
    )
    .expect("grants file");

    let install = || {
        let out = krate()
            .arg("install")
            .arg(&bundle)
            .arg("--prefix")
            .arg(prefix.path())
            .env("HOME", home.path())
            .output()
            .expect("install");
        assert!(
            out.status.success(),
            "install: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        std::fs::read_dir(prefix.path())
            .expect("prefix")
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|x| x == "app"))
            .expect("an installed .app")
    };
    let app = install();
    let payload = app.join("Contents/Resources/app.krate");

    // The guard: a run that cannot ask must find every permission remembered.
    let check = krate()
        .current_dir(dir.path())
        .arg("run")
        .arg("--headless")
        .arg(&payload)
        .env("HOME", home.path())
        .stdin(Stdio::null())
        .output()
        .expect("run");
    assert!(
        String::from_utf8_lossy(&check.stdout).contains("probe: ok"),
        "the remembered grants must cover the app before a double-click is tried: {}",
        String::from_utf8_lossy(&check.stderr)
    );

    // A file an older Krate let the app write inside its bundle.
    let old = app.join("Contents/Resources/probe-dir");
    std::fs::create_dir_all(&old).expect("old dir");
    std::fs::write(old.join("from-before.txt"), "kept").expect("old file");

    let double_click = |app: &std::path::Path| {
        let exe = std::fs::read_dir(app.join("Contents/MacOS"))
            .expect("MacOS")
            .flatten()
            .map(|e| e.path())
            .next()
            .expect("an executable");
        let out = Command::new(exe)
            .env("HOME", home.path())
            .env("KRATE_TEST_WATCHDOG_SECS", "120")
            .env("KRATE_QUIET_WINDOW", "1")
            .stdin(Stdio::null())
            .output()
            .expect("open the installed app");
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        assert!(
            stdout.contains("probe: ok"),
            "the installed app must run from a double-click: {stdout}{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    double_click(&app);

    let data = home
        .path()
        .join("Library/Application Support/Krate/apps/com.acme.keeps-files");
    assert!(
        data.join("probe-dir/hello.txt").is_file(),
        "the app's file is in its data folder"
    );
    assert_eq!(
        std::fs::read_to_string(data.join("probe-dir/from-before.txt"))
            .ok()
            .as_deref(),
        Some("kept"),
        "what an older Krate left in the bundle is moved out"
    );
    let resources: Vec<String> = std::fs::read_dir(app.join("Contents/Resources"))
        .expect("resources")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert!(
        !resources.iter().any(|n| n == "probe-dir"),
        "nothing the app wrote is left inside the bundle: {resources:?}"
    );
    let sealed = Command::new("/usr/bin/codesign")
        .arg("-v")
        .arg(&app)
        .output()
        .expect("codesign");
    assert!(
        sealed.status.success(),
        "the installed app still verifies after it ran: {}",
        String::from_utf8_lossy(&sealed.stderr)
    );

    // A newer install replaces the .app; the files stay.
    let app = install();
    assert!(
        data.join("probe-dir/hello.txt").is_file(),
        "reinstall kept the file"
    );
    assert!(data.join("probe-dir/from-before.txt").is_file());
    double_click(&app);
    assert!(data.join("probe-dir/hello.txt").is_file());
}

/// Uninstalling removes the app and keeps what the person wrote in it,
/// unless they ask otherwise (IC-278, IC-396).
///
/// Those are two decisions and were one command's silence: there was no
/// uninstall at all, so the only way to remove an installed app was to
/// delete a folder and guess what else belonged to it. Keeping data is
/// the default because the opposite assumption cannot be undone.
#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn uninstalling_removes_the_app_and_keeps_the_data_unless_told_otherwise() {
    let dir = tempfile::tempdir().expect("tempdir");
    let prefix = tempfile::tempdir().expect("prefix");
    let home = tempfile::tempdir().expect("home");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(
        &wasm,
        include_bytes!("../../bundle/tests/fixtures/minimal-run.wasm"),
    )
    .expect("component");

    let pack = |id: &str| {
        let manifest = dir.path().join(format!("{id}.toml"));
        std::fs::write(
            &manifest,
            format!(
                "[app]\nid = \"{id}\"\nname = \"{id}\"\nversion = \"1.0.0\"\n\
                 entry = \"code.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n"
            ),
        )
        .expect("manifest");
        let bundle = dir.path().join(format!("{id}.krate"));
        assert!(krate()
            .args(["pack"])
            .arg(&wasm)
            .arg("--manifest")
            .arg(&manifest)
            .arg("-o")
            .arg(&bundle)
            .status()
            .expect("pack")
            .success());
        bundle
    };
    let install = |bundle: &std::path::Path| {
        krate()
            .arg("install")
            .arg(bundle)
            .arg("--prefix")
            .arg(prefix.path())
            .env("HOME", home.path())
            .output()
            .expect("install")
    };

    let keep = pack("dev.krate.keep");
    let drop = pack("dev.krate.drop");
    if !install(&keep).status.success() {
        eprintln!("skipping the uninstall check: this platform has no installer");
        return;
    }
    assert!(
        install(&drop).status.success(),
        "the second app must install"
    );

    // Both are listed, by id.
    let listed = krate()
        .arg("installed")
        .arg("--prefix")
        .arg(prefix.path())
        .env("HOME", home.path())
        .output()
        .expect("installed");
    let text = String::from_utf8_lossy(&listed.stdout);
    assert!(
        text.contains("dev.krate.keep") && text.contains("dev.krate.drop"),
        "both installed apps must be listed: {text}"
    );

    // Data the person made. Written where the runtime keeps an unsigned
    // app's store, which is what these fixtures are.
    let store = home.path().join(".krate/store");
    std::fs::create_dir_all(&store).expect("store dir");
    let notes = store.join("dev.krate.drop.kv");
    std::fs::write(&notes, b"what the person wrote").expect("their data");

    // Removing one app leaves the other, and leaves the data.
    let out = krate()
        .args(["uninstall", "dev.krate.drop"])
        .arg("--prefix")
        .arg(prefix.path())
        .env("HOME", home.path())
        .output()
        .expect("uninstall");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(
        said.contains("saved data is kept"),
        "it must say the data was kept, or the person cannot know: {said}"
    );
    assert!(
        notes.exists(),
        "uninstalling must not take the person's data with it"
    );

    let listed = krate()
        .arg("installed")
        .arg("--prefix")
        .arg(prefix.path())
        .env("HOME", home.path())
        .output()
        .expect("installed");
    let text = String::from_utf8_lossy(&listed.stdout);
    assert!(
        text.contains("dev.krate.keep"),
        "the other app stays: {text}"
    );
    assert!(
        !text.contains("dev.krate.drop"),
        "the removed one is gone: {text}"
    );

    // Asking for the data to go removes it.
    assert!(
        install(&drop).status.success(),
        "reinstall for the second half"
    );
    let out = krate()
        .args(["uninstall", "dev.krate.drop", "--delete-data"])
        .arg("--prefix")
        .arg(prefix.path())
        .env("HOME", home.path())
        .output()
        .expect("uninstall --delete-data");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !notes.exists(),
        "--delete-data must actually delete it, or the flag is a lie"
    );

    // An app that is not installed is a clear refusal, not a silent success.
    let out = krate()
        .args(["uninstall", "dev.krate.never"])
        .arg("--prefix")
        .arg(prefix.path())
        .env("HOME", home.path())
        .output()
        .expect("uninstall missing");
    assert_ne!(
        out.status.code(),
        Some(0),
        "removing nothing is not a success"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("dev.krate.never"),
        "and it names what it could not find"
    );
}

/// An installed app carries a signature that describes IT, not the engine
/// it was built around (IC-396).
///
/// The launcher is a hard link to the engine, so the wrapper arrived
/// carrying the engine's own ad-hoc signature -- whose identifier and
/// resource rules are about the engine. `codesign --verify --strict`
/// therefore failed on every installed app with "code has no resources
/// but signature indicates they must be present", which is macOS
/// correctly saying the signature is not about this thing.
///
/// Ad-hoc is what a local wrapper can honestly have: a real identity
/// would need a certificate Krate does not hold on the person's machine.
/// What it buys is a signature that matches the bundle.
#[test]
#[cfg(target_os = "macos")]
fn an_installed_app_is_signed_as_itself() {
    let dir = tempfile::tempdir().expect("tempdir");
    let prefix = tempfile::tempdir().expect("prefix");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(
        &wasm,
        include_bytes!("../../bundle/tests/fixtures/minimal-run.wasm"),
    )
    .expect("component");
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"dev.krate.signedwrapper\"\nname = \"Signed Wrapper\"\n\
         version = \"1.0.0\"\nentry = \"code.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n",
    )
    .expect("manifest");
    let bundle = dir.path().join("app.krate");
    assert!(krate()
        .args(["pack"])
        .arg(&wasm)
        .arg("--manifest")
        .arg(&manifest)
        .arg("-o")
        .arg(&bundle)
        .status()
        .expect("pack")
        .success());
    assert!(krate()
        .arg("install")
        .arg(&bundle)
        .arg("--prefix")
        .arg(prefix.path())
        .output()
        .expect("install")
        .status
        .success());

    let installed = std::fs::read_dir(prefix.path())
        .expect("read prefix")
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .expect("an .app was installed");

    let verify = std::process::Command::new("codesign")
        .args(["--verify", "--strict"])
        .arg(&installed)
        .output();
    let Ok(verify) = verify else {
        eprintln!("skipping: codesign is not available on this machine");
        return;
    };
    assert!(
        verify.status.success(),
        "the installed app must pass a strict signature check: {}",
        String::from_utf8_lossy(&verify.stderr)
    );

    // And the signature must be ABOUT this app: inheriting the engine's
    // identifier is exactly the state that failed strict verification.
    let described = std::process::Command::new("codesign")
        .args(["-dv"])
        .arg(&installed)
        .output()
        .expect("codesign -dv");
    let text = String::from_utf8_lossy(&described.stderr);
    assert!(
        text.contains("Identifier=dev.krate.app.dev.krate.signedwrapper"),
        "the signature must name this app, not the engine: {text}"
    );
}

/// Installing over an app that is already there never leaves the person
/// with neither (IC-278).
///
/// The wrapper is built beside the target and swapped in, so the app they
/// had exists until a complete replacement is ready. The old code removed
/// the installed app and then wrote the new one, which leaves a window --
/// a full disk, a crash, a killed terminal -- where both are gone.
///
/// Driven through the CLI: install, reinstall, and check the app is whole
/// throughout and that no staging directory is left behind.
#[test]
fn reinstalling_an_app_leaves_no_moment_with_neither_copy() {
    let dir = tempfile::tempdir().expect("tempdir");
    let prefix = tempfile::tempdir().expect("prefix");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(
        &wasm,
        include_bytes!("../../bundle/tests/fixtures/minimal-run.wasm"),
    )
    .expect("component");
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"dev.krate.reinstall\"\nname = \"Reinstall\"\nversion = \"1.0.0\"\n\
         entry = \"code.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n",
    )
    .expect("manifest");
    let bundle = dir.path().join("app.krate");
    assert!(krate()
        .args(["pack"])
        .arg(&wasm)
        .arg("--manifest")
        .arg(&manifest)
        .arg("-o")
        .arg(&bundle)
        .status()
        .expect("pack")
        .success());

    let install = || {
        krate()
            .arg("install")
            .arg(&bundle)
            .arg("--prefix")
            .arg(prefix.path())
            .output()
            .expect("install")
    };
    let entries = || {
        let mut names: Vec<String> = std::fs::read_dir(prefix.path())
            .expect("read prefix")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        names
    };

    let first = install();
    if !first.status.success() {
        // Windows has no install path at all -- `krate install` says so and
        // exits non-zero. Naming the reason keeps this out of the
        // setup-failure bucket in the test report.
        eprintln!("skipping the reinstall check on Windows: there is no installer to drive");
        return;
    }
    let installed = entries();
    assert!(
        !installed.is_empty(),
        "the install reported success and left nothing: {installed:?}"
    );

    // The same app again. What it must leave behind is EXACTLY what the
    // first install left -- no more, no fewer -- so a half-finished wrapper
    // or a set-aside copy shows up as a difference. The prefix holds one
    // .app on macOS and a data root on Linux, so this compares the two
    // listings rather than counting them.
    let again = install();
    assert!(
        again.status.success(),
        "reinstall failed: {}",
        String::from_utf8_lossy(&again.stderr)
    );
    let after = entries();
    assert_eq!(
        after, installed,
        "a reinstall must leave exactly what the first one did, with no staging debris: {after:?}"
    );
    assert!(
        !after
            .iter()
            .any(|n| n.contains(".installing") || n.contains(".replaced")),
        "no half-finished wrapper may survive: {after:?}"
    );
}

/// A person can see what they have allowed, and take it back (IC-017).
///
/// A remembered permission with no way to withdraw it is a wall that only
/// ever opens. This drives the whole loop through the CLI: grant, see it
/// listed, forget it, and be asked again.
#[test]
fn what_was_allowed_can_be_seen_and_taken_back() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = tempfile::tempdir().expect("home");
    let minimal = include_bytes!("../../bundle/tests/fixtures/minimal-run.wasm");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(&wasm, minimal).expect("component");
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"dev.krate.takeback\"\nname = \"Takeback\"\nversion = \"1.0.0\"\n\
         entry = \"code.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n\n\
         [[capabilities]]\ncap = \"fs.read:taken/**\"\nrationale = \"the app's work\"\n\
         required = true\n",
    )
    .expect("manifest");
    let bundle = dir.path().join("app.krate");
    assert!(krate()
        .args(["pack"])
        .arg(&wasm)
        .arg("--manifest")
        .arg(&manifest)
        .arg("-o")
        .arg(&bundle)
        .status()
        .expect("pack")
        .success());
    let key = dir.path().join("key.pkcs8");
    assert!(krate()
        .arg("sign")
        .arg(&bundle)
        .arg("--key")
        .arg(&key)
        .args(["--generate-key", "--namespace", "takeback/app"])
        .output()
        .expect("sign")
        .status
        .success());

    let permissions = |args: &[&str]| -> String {
        let out = krate()
            .arg("permissions")
            .args(args)
            .env("HOME", home.path())
            .output()
            .expect("permissions");
        assert!(
            out.status.success(),
            "permissions failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).to_string()
    };
    let run = |answer: &str| -> Option<i32> {
        use std::io::Write;
        let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_krate"))
            .arg("run")
            .arg(&bundle)
            .args(["--headless", "--prompt"])
            .env("HOME", home.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn");
        child
            .stdin
            .as_mut()
            .expect("stdin")
            .write_all(answer.as_bytes())
            .expect("answer");
        child.wait().expect("wait").code()
    };

    let empty = permissions(&[]);
    assert!(
        empty.contains("No app has been allowed anything yet"),
        "a fresh machine says so plainly: {empty}"
    );

    assert_eq!(run("A\n"), Some(0), "granted, so it runs");
    assert_eq!(run(""), Some(0), "and is not asked again");

    let listed = permissions(&[]);
    assert!(
        listed.contains("dev.krate.takeback") && listed.contains("fs.read:taken/**"),
        "what was allowed must be visible, in the words it was asked in: {listed}"
    );

    let forgotten = permissions(&["--forget", "dev.krate.takeback"]);
    assert!(forgotten.contains("will ask again"), "{forgotten}");
    assert_ne!(
        run(""),
        Some(0),
        "after taking it back the app must ask again, and be refused with no answer"
    );
    assert!(
        !permissions(&[]).contains("fs.read:taken/**"),
        "and it is gone from the list"
    );
}

/// A data profile is named on the trust screen, a bad name is refused before
/// anything runs, and a screenshot run is a preview by default (IC-245).
#[test]
fn a_profile_is_named_on_the_trust_screen_and_a_bad_name_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = dir.path().join("app.krate");
    std::fs::write(&bundle, archive_carrying(&[])).expect("write");

    let shown = krate()
        .args(["run", "--dump-caps", "--profile", "preview"])
        .arg(&bundle)
        .output()
        .expect("dump caps");
    let stdout = String::from_utf8_lossy(&shown.stdout);
    assert!(
        stdout.contains("kept in the preview profile"),
        "the trust screen must say where the data goes: {stdout}"
    );
    let default = krate()
        .args(["run", "--dump-caps"])
        .arg(&bundle)
        .output()
        .expect("dump caps");
    assert!(
        !String::from_utf8_lossy(&default.stdout).contains("profile"),
        "the default profile is not a thing to announce"
    );

    for bad in ["../escape", "Default", "with space"] {
        let refused = krate()
            .args(["run", "--profile", bad])
            .arg(&bundle)
            .args(["--headless", "--auto-grant"])
            .output()
            .expect("run");
        let stderr = String::from_utf8_lossy(&refused.stderr);
        assert_eq!(refused.status.code(), Some(2), "{bad:?}: {stderr}");
        assert!(
            stderr.contains("--profile"),
            "{bad:?}: the refusal names the flag: {stderr}"
        );
    }
}

/// The three identities move exactly as their labels say, on the surface a
/// person reads them from (IC-212).
///
/// A signed app is one release, named by one id the product prints
/// (IC-389, test 468; K-308): the same file twice is the same release,
/// a new version is another, and an unsigned file is none.
#[test]
fn a_signed_app_reports_its_release_id_and_an_unsigned_one_reports_none() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"com.example.release\"\nname = \"Release\"\nversion = \"1.0.0\"\n\
         entry = \"code.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n",
    )
    .expect("manifest");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(
        &wasm,
        include_bytes!("../../bundle/tests/fixtures/minimal-run.wasm"),
    )
    .expect("wasm");
    let app = dir.path().join("app.krate");
    krate_bundle::pack(&manifest, &wasm, &app).expect("pack");
    let report = |path: &std::path::Path| -> serde_json::Value {
        let output = krate()
            .args(["run", "--json", "--headless", "--auto-grant"])
            .arg(path)
            .output()
            .expect("run --json");
        serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
            panic!(
                "no JSON report: {}",
                String::from_utf8_lossy(&output.stderr)
            )
        })
    };
    assert!(
        report(&app)["identity"]["release"].is_null(),
        "unsigned: no release"
    );

    let key = krate_bundle::signing::SigningKey::from_pkcs8(
        &krate_bundle::signing::SigningKey::generate_pkcs8().expect("key"),
    )
    .expect("key");
    krate_bundle::sign_bundle(&app, &key, "com.example.release", "1.0.0", 1_700_000_000)
        .expect("sign");
    let release = report(&app)["identity"]["release"].clone();
    let id = release["id"]
        .as_str()
        .expect("a signed app has a release id")
        .to_string();
    assert_eq!(id.len(), 64, "{release}");
    assert_eq!(release["version"].as_str(), Some("1.0.0"));

    let copy = dir.path().join("copy.krate");
    std::fs::copy(&app, &copy).expect("copy");
    assert_eq!(
        report(&copy)["identity"]["release"]["id"].as_str(),
        Some(id.as_str()),
        "the same file is the same release"
    );

    krate_bundle::sign_bundle(&app, &key, "com.example.release", "1.0.1", 1_700_000_000)
        .expect("re-sign");
    assert_ne!(
        report(&app)["identity"]["release"]["id"].as_str(),
        Some(id.as_str()),
        "a new version is a new release"
    );

    let looked = krate()
        .args(["run", "--dump-caps"])
        .arg(&copy)
        .output()
        .expect("inspect");
    let stdout = String::from_utf8_lossy(&looked.stdout);
    assert!(
        stdout.contains(&format!("release {id}")),
        "the screen a person reads names the release: {stdout}"
    );
}

/// An app the hub removed is refused on this machine, offline, by the
/// list it last fetched (IC-669, test 1311); a machine holding no list
/// runs it and says nothing was checked.
#[test]
fn a_removed_app_is_refused_offline_by_the_held_blocklist() {
    let home = tempfile::tempdir().expect("home");
    let bounce =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../evidence/ported/bounce.krate");
    let hash = krate_bundle::sha256_hex(&std::fs::read(&bounce).expect("bounce"));
    let run = || {
        krate()
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .args(["run", "--headless", "--auto-grant"])
            .arg(&bounce)
            .output()
            .expect("run")
    };

    // No list held: not refused.
    let free = run();
    assert_eq!(
        free.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&free.stderr)
    );
    let held = krate()
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .arg("blocklist")
        .output()
        .expect("blocklist");
    assert!(String::from_utf8_lossy(&held.stdout).contains("no blocklist held"));

    // The hub's list, as `krate blocklist --update` would have written it.
    std::fs::create_dir_all(home.path().join(".krate")).unwrap();
    std::fs::write(
        home.path().join(".krate/blocklist.json"),
        serde_json::json!({
            "schema": "krate.hub-blocklist.v1",
            "hub": "https://hub.example",
            "issued_at": 1_700_000_000u64,
            "fetched_at": 1_700_000_100u64,
            "blocked": [{
                "hash": hash,
                "reason": "impersonates a bank",
                "scope": "listing",
                "emergency": true,
                "at": 1_700_000_000u64,
                "notice": "https://hub.example/takedown/abc",
            }],
        })
        .to_string(),
    )
    .unwrap();
    let blocked = run();
    assert_eq!(
        blocked.status.code(),
        Some(5),
        "a removed app must be refused, exit 5 like the wall"
    );
    let stderr = String::from_utf8_lossy(&blocked.stderr);
    assert!(
        stderr.contains("the hub removed this app: impersonates a bank"),
        "{stderr}"
    );
    assert!(stderr.contains("emergency"), "{stderr}");
    assert!(
        stderr.contains("https://hub.example/takedown/abc"),
        "{stderr}"
    );
    assert!(
        stderr.contains("krate blocklist --update"),
        "and how to refresh: {stderr}"
    );
    assert!(
        !String::from_utf8_lossy(&blocked.stdout).contains("items:"),
        "the app must not have run"
    );

    // A different file with the same name is not the blocked bytes.
    let other = home.path().join("bounce.krate");
    std::fs::write(
        &other,
        raw_bundle(
            "[app]\nid = \"com.example.other\"\nname = \"Other\"\nversion = \"1.0.0\"\n\
             entry = \"code.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n",
            include_bytes!("../../bundle/tests/fixtures/minimal-run.wasm"),
            &[],
        ),
    )
    .unwrap();
    let ok = krate()
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .args(["run", "--headless", "--auto-grant"])
        .arg(&other)
        .output()
        .expect("run other");
    assert_eq!(
        ok.status.code(),
        Some(0),
        "the block is on the bytes, not the name: {}",
        String::from_utf8_lossy(&ok.stderr)
    );

    let shown = krate()
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .arg("blocklist")
        .output()
        .expect("blocklist");
    let text = String::from_utf8_lossy(&shown.stdout);
    assert!(
        text.contains("1 removed app")
            && text.contains(&hash[..12])
            && text.contains("(emergency)"),
        "{text}"
    );
}

/// The task oracle through the binary (IC-743, test 1506): a task file's
/// steps are driven after the usability checks, the report says whether
/// the task completed, and a failing step says what WAS on screen.
#[test]
fn the_usability_report_says_whether_the_task_completed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bounce =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../evidence/ported/bounce.krate");
    let run = |task: &str| -> serde_json::Value {
        let task_path = dir.path().join("krate-check.toml");
        std::fs::write(&task_path, task).expect("task file");
        let report = dir.path().join("usability.json");
        let _ = std::fs::remove_file(&report);
        let output = krate()
            .args(["run", "--headless", "--auto-grant", "--usability-report"])
            .arg(&report)
            .arg("--task")
            .arg(&task_path)
            .arg(&bounce)
            .output()
            .expect("run");
        let text = std::fs::read_to_string(&report).unwrap_or_else(|_| {
            panic!(
                "no report was written: {}",
                String::from_utf8_lossy(&output.stderr)
            )
        });
        serde_json::from_str(&text).expect("json")
    };
    // bounce draws its own canvas: nothing is labelled, so the only task
    // it can complete is one that expects nothing in particular.
    let held = run("name = \"keep the ball on screen\"\n[[step]]\nwait_ms = 300\n[[step]]\nexpect_no_text = \"Game over\"\n");
    assert_eq!(held["task"]["outcome"], "held", "{held}");
    assert_eq!(held["task_name"], "keep the ball on screen");
    assert_eq!(held["task_steps_done"], 2);

    let broke = run("name = \"press start\"\n[[step]]\nclick = \"Start\"\n[[step]]\nexpect_text = \"running\"\n");
    assert_eq!(broke["task"]["outcome"], "broke", "{broke}");
    let detail = broke["task"]["detail"].as_str().unwrap_or("");
    assert!(
        detail.contains("step 1 of 2")
            && detail.contains("\"Start\"")
            && detail.contains("on screen: []"),
        "{detail}"
    );
    assert_eq!(broke["task_steps_done"], 0);

    // A task file that is not one is refused before anything runs.
    let task_path = dir.path().join("krate-check.toml");
    std::fs::write(
        &task_path,
        "name = \"no expectation\"\n[[step]]\nclick = \"Start\"\n",
    )
    .unwrap();
    let output = krate()
        .args(["run", "--headless", "--auto-grant", "--usability-report"])
        .arg(dir.path().join("r2.json"))
        .arg("--task")
        .arg(&task_path)
        .arg(&bounce)
        .output()
        .expect("run");
    assert_ne!(output.status.code(), Some(0));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("never looks at the screen"),
        "a task with no expectation is refused with the reason: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A fork says what it was changed from, and the product prints it
/// (IC-397, test 494; K-312).
///
/// The record is an entry, `derived-from.json`, so it is written here by
/// the raw writer the way any tool could write it, and read back through
/// the product's own two doors: `run --json` reports it under `identity`,
/// and `run --dump-caps` -- the screen where a person decides whether to
/// trust the app -- says the app was changed from another one and whether
/// that one was signed. The parent's signer is never printed: their
/// signature does not cover these bytes (F-013).
#[test]
fn a_fork_names_its_parent_and_the_product_says_so() {
    const MANIFEST: &str = "[app]\nid = \"com.example.fork\"\nname = \"Fork\"\n\
                            version = \"1.0.0\"\nentry = \"code.wasm\"\n\
                            world = \"krate:app/cli@0.1.0\"\n";
    let dir = tempfile::tempdir().expect("tempdir");
    let minimal = include_bytes!("../../bundle/tests/fixtures/minimal-run.wasm");
    let report = |name: &str, bytes: &[u8]| -> serde_json::Value {
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).expect("write");
        let output = krate()
            .args(["run", "--json", "--headless", "--auto-grant"])
            .arg(&path)
            .output()
            .expect("run --json");
        serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
            panic!(
                "{name}: no JSON report: {}",
                String::from_utf8_lossy(&output.stderr)
            )
        })
    };

    let parent = report("parent.krate", &raw_bundle(MANIFEST, minimal, &[]));
    assert!(
        parent["identity"]["derived_from"].is_null(),
        "an original was changed from nothing: {parent}"
    );
    let parent_archive = parent["identity"]["archive"]
        .as_str()
        .expect("the parent's archive identity")
        .to_string();
    let parent_execution = parent["identity"]["execution"]
        .as_str()
        .expect("the parent's execution identity")
        .to_string();

    let record = serde_json::json!({
        "schema": "krate.bundle.derived-from.v1",
        "archive": parent_archive,
        "execution": parent_execution,
        "project": null,
        "parent_signed": false,
        "at": 1_700_000_000u64,
    });
    let fork_bytes = raw_bundle(
        MANIFEST,
        minimal,
        &[(
            "derived-from.json".to_string(),
            serde_json::to_vec(&record).unwrap(),
        )],
    );
    let fork = report("fork.krate", &fork_bytes);
    let derived = &fork["identity"]["derived_from"];
    assert_eq!(
        derived["archive"].as_str(),
        Some(parent_archive.as_str()),
        "the fork must name the parent file it was changed from: {fork}"
    );
    assert_eq!(
        derived["execution"].as_str(),
        Some(parent_execution.as_str())
    );
    assert_eq!(derived["parent_signed"].as_bool(), Some(false));
    assert_eq!(
        fork["identity"]["execution"].as_str(),
        Some(parent_execution.as_str()),
        "what runs did not change, so the fork's execution identity is the parent's"
    );
    assert_ne!(
        fork["identity"]["archive"].as_str(),
        Some(parent_archive.as_str()),
        "the fork is a different file"
    );

    // The screen a person reads.
    let looked = krate()
        .args(["run", "--dump-caps"])
        .arg(dir.path().join("fork.krate"))
        .output()
        .expect("inspect the fork");
    let stdout = String::from_utf8_lossy(&looked.stdout);
    assert!(
        looked.status.success(),
        "{}",
        String::from_utf8_lossy(&looked.stderr)
    );
    assert!(
        stdout.contains("changed from another app"),
        "inspection must say this is a fork: {stdout}"
    );
    assert!(
        stdout.contains(&parent_archive[..12]) && stdout.contains("was not signed"),
        "and name the parent file and whether it was signed: {stdout}"
    );
    let original = krate()
        .args(["run", "--dump-caps"])
        .arg(dir.path().join("parent.krate"))
        .output()
        .expect("inspect the parent");
    assert!(
        !String::from_utf8_lossy(&original.stdout).contains("changed from another app"),
        "an original is not called a fork"
    );
}

/// archive is the exact file bytes; execution is what runs (manifest,
/// component, assets); project is what can be rebuilt (adds source and
/// SDK). Each change below moves the identities it should and none it
/// should not, measured through `krate run --json` rather than the library,
/// because the number a person compares is the one the product prints.
#[test]
fn each_identity_moves_only_when_what_it_names_changes() {
    const MANIFEST: &str = "[app]\nid = \"com.example.layers\"\nname = \"Layers\"\n\
                            version = \"1.0.0\"\nentry = \"code.wasm\"\n\
                            world = \"krate:app/cli@0.1.0\"\n";
    let dir = tempfile::tempdir().expect("tempdir");
    let minimal = include_bytes!("../../bundle/tests/fixtures/minimal-run.wasm");
    let identity = |name: &str, bytes: &[u8]| -> (String, String, serde_json::Value) {
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).expect("write");
        let output = krate()
            .args(["run", "--json", "--headless", "--auto-grant"])
            .arg(&path)
            .output()
            .expect("run --json");
        let payload: serde_json::Value =
            serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
                panic!(
                    "{name}: no JSON report: {}",
                    String::from_utf8_lossy(&output.stderr)
                )
            });
        let id = &payload["identity"];
        (
            id["archive"]
                .as_str()
                .unwrap_or_else(|| panic!("{name}: archive: {payload}"))
                .to_string(),
            id["execution"]
                .as_str()
                .unwrap_or_else(|| panic!("{name}: execution: {payload}"))
                .to_string(),
            id["project"].clone(),
        )
    };

    let (archive0, execution0, project0) =
        identity("base.krate", &raw_bundle(MANIFEST, minimal, &[]));
    assert!(
        project0.is_null(),
        "no source: no project identity to claim"
    );

    // Raw repack, reordered and recompressed: the same app in a different
    // file. `krate pack` deflates what the raw writer stored.
    let manifest_path = dir.path().join("manifest.toml");
    std::fs::write(&manifest_path, MANIFEST).expect("manifest");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(&wasm, minimal).expect("component");
    let packed = dir.path().join("packed.krate");
    assert!(krate()
        .args(["pack"])
        .arg(&wasm)
        .arg("--manifest")
        .arg(&manifest_path)
        .arg("-o")
        .arg(&packed)
        .status()
        .expect("pack")
        .success());
    let packed_bytes = std::fs::read(&packed).expect("read");
    let (archive1, execution1, _) = identity("packed-copy.krate", &packed_bytes);
    assert_ne!(archive1, archive0, "a repack is a different file");
    assert_eq!(execution1, execution0, "a repack is the same app");

    // Manifest whitespace: the same manifest with Windows line endings.
    let crlf = MANIFEST.replace('\n', "\r\n");
    let (archive2, execution2, _) = identity("crlf.krate", &raw_bundle(&crlf, minimal, &[]));
    assert_ne!(archive2, archive0);
    assert_eq!(
        execution2, execution0,
        "line endings are not a different app (K-265)"
    );

    // Manifest semantic change: a different name is a different app.
    let renamed = MANIFEST.replace("Layers", "Renamed");
    let (_, execution3, _) = identity("renamed.krate", &raw_bundle(&renamed, minimal, &[]));
    assert_ne!(
        execution3, execution0,
        "a semantic manifest change moves what runs"
    );

    // Code mutation moves what runs.
    let other = include_bytes!("../../bundle/tests/fixtures/minimal-run-other.wasm");
    let (_, execution4, _) = identity("other.krate", &raw_bundle(MANIFEST, other, &[]));
    assert_ne!(execution4, execution0, "different code is a different app");

    // Asset mutation moves what runs; source mutation moves only the project.
    let with_asset = raw_bundle(
        MANIFEST,
        minimal,
        &[("assets/logo.png".to_string(), b"PNG1".to_vec())],
    );
    let (_, execution5, _) = identity("asset.krate", &with_asset);
    assert_ne!(execution5, execution0, "an asset is part of what runs");
    let with_source = raw_bundle(
        MANIFEST,
        minimal,
        &[("source/lib.rs".to_string(), b"fn a() {}".to_vec())],
    );
    let (_, execution6, project6) = identity("source.krate", &with_source);
    assert_eq!(execution6, execution0, "source is not part of what runs");
    let project6 = project6
        .as_str()
        .expect("with source there is a project identity")
        .to_string();
    let with_other_source = raw_bundle(
        MANIFEST,
        minimal,
        &[("source/lib.rs".to_string(), b"fn b() {}".to_vec())],
    );
    let (_, execution7, project7) = identity("source2.krate", &with_other_source);
    assert_eq!(execution7, execution0);
    assert_ne!(
        project7.as_str().expect("project"),
        project6,
        "changed source is a different project"
    );
    let with_sdk = raw_bundle(
        MANIFEST,
        minimal,
        &[
            ("source/lib.rs".to_string(), b"fn a() {}".to_vec()),
            ("sdk/krate.wit".to_string(), b"package krate:x;".to_vec()),
        ],
    );
    let (_, execution8, project8) = identity("sdk.krate", &with_sdk);
    assert_eq!(execution8, execution0, "the SDK is not part of what runs");
    assert_ne!(
        project8.as_str().expect("project"),
        project6,
        "a different SDK is a different project"
    );

    // A signature changes the file and nothing else.
    let key = dir.path().join("key.pkcs8");
    assert!(krate()
        .arg("sign")
        .arg(&packed)
        .arg("--key")
        .arg(&key)
        .args(["--generate-key", "--namespace", "layers/app"])
        .status()
        .expect("sign")
        .success());
    let signed_bytes = std::fs::read(&packed).expect("read");
    let (archive9, execution9, project9) = identity("signed-copy.krate", &signed_bytes);
    assert_ne!(archive9, archive1, "signing writes into the file");
    assert_eq!(execution9, execution0, "signing does not change what runs");
    assert!(project9.is_null(), "signing does not invent a project");
}

/// The validator and the runtime agree (IC-210, "runtime-validator parity").
///
/// One function judges a component at every door -- pack, open, and so run,
/// publish and the hub -- and this is the test that its verdicts match what
/// the engine then does. The accept side: the smallest component the
/// validator passes is one the runtime actually runs to a clean exit. The
/// refuse side: a component the validator rejects is refused by `run` and
/// by `pack` with the SAME words, before the engine is asked, and the
/// engine would have run it happily -- it exports `run` and instantiates --
/// which is exactly why the rule has to live in the validator.
#[test]
fn the_validator_and_the_runtime_agree_on_what_a_krate_component_is() {
    let dir = tempfile::tempdir().expect("tempdir");
    let minimal = include_bytes!("../../bundle/tests/fixtures/minimal-run.wasm");
    let extra_export = include_bytes!("../../bundle/tests/fixtures/extra-export.wasm");

    // Accept side: packed by our own packer, run by our own runtime.
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(&manifest, BUNDLE_MANIFEST).expect("write manifest");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(&wasm, minimal).expect("write component");
    let bundle = dir.path().join("minimal.krate");
    let packed = krate()
        .args(["pack"])
        .arg(&wasm)
        .arg("--manifest")
        .arg(&manifest)
        .arg("-o")
        .arg(&bundle)
        .output()
        .expect("run krate pack");
    assert!(
        packed.status.success(),
        "the smallest valid component must pack: {}",
        String::from_utf8_lossy(&packed.stderr)
    );
    let ran = krate()
        .arg("run")
        .arg(&bundle)
        .args(["--headless", "--auto-grant"])
        .output()
        .expect("run the minimal bundle");
    assert_eq!(
        ran.status.code(),
        Some(0),
        "what the validator accepts, the runtime runs: {}",
        String::from_utf8_lossy(&ran.stderr)
    );

    // Refuse side, at run: a hand-assembled bundle nobody's packer checked.
    let refused = dir.path().join("extra.krate");
    std::fs::write(&refused, archive_with_component(extra_export, &[])).expect("write");
    let ran = krate()
        .arg("run")
        .arg(&refused)
        .args(["--headless", "--auto-grant"])
        .output()
        .expect("run the extra-export bundle");
    let run_stderr = String::from_utf8_lossy(&ran.stderr);
    assert_ne!(ran.status.code(), Some(0), "an extra export must not run");
    assert!(
        run_stderr.contains("exports more than `run`") && run_stderr.contains("debug-hook"),
        "run must refuse with the validator's words and name the export: {run_stderr}"
    );

    // Refuse side, at pack: the same component, the same words.
    let wasm = dir.path().join("extra.wasm");
    std::fs::write(&wasm, extra_export).expect("write component");
    let out = dir.path().join("never.krate");
    let packed = krate()
        .args(["pack"])
        .arg(&wasm)
        .arg("--manifest")
        .arg(&manifest)
        .arg("-o")
        .arg(&out)
        .output()
        .expect("run krate pack");
    let pack_stderr = String::from_utf8_lossy(&packed.stderr);
    assert!(
        !packed.status.success(),
        "pack must refuse what run refuses"
    );
    assert!(
        pack_stderr.contains("exports more than `run`") && pack_stderr.contains("debug-hook"),
        "pack must refuse with the same words: {pack_stderr}"
    );
    assert!(!out.exists(), "a refused pack leaves no bundle behind");
}

/// A zip whose central directory lists `path` twice.
///
/// Assembled from raw records: the zip crate's writer refuses a literal
/// duplicate name, so a library cannot build the archive a hostile packer
/// produces. IC-713 asks for exactly this ("archives produced by multiple
/// ZIP writers").
fn archive_naming_one_path_twice(path: &str) -> Vec<u8> {
    archive_carrying(&[
        (path.to_string(), b"the reviewed copy".to_vec()),
        (path.to_string(), b"the attacker's copy".to_vec()),
    ])
}

/// A well formed archive plus whatever `extra` entries are named, verbatim.
fn archive_carrying(extra: &[(String, Vec<u8>)]) -> Vec<u8> {
    archive_with_component(
        include_bytes!("../../bundle/tests/fixtures/minimal-run.wasm"),
        extra,
    )
}

/// A well formed archive around exactly these component bytes.
/// The record set every reader resolves (IC-714, test 1477), and the
/// extension namespace and profile 2 rules through the binary people run
/// (tests 1475 and 1476).
///
/// `krate inspect --format json` is the runtime's, the signer's and the
/// hub's view: one opener. `scripts/krate-records.py` is an independent
/// reader -- Python's zipfile and the profile's rules written a second
/// time. On every committed bundle and adversarial fixture, on a bundle
/// packed with an extension and on a profile 2 archive with a stray
/// record, the two must accept the same record set or both refuse.
#[test]
fn every_reader_resolves_one_record_set_and_the_extension_namespace_holds() {
    let dir = tempfile::tempdir().expect("tempdir");
    let minimal = include_bytes!("../../bundle/tests/fixtures/minimal-run.wasm");
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let reader = repo.join("scripts/krate-records.py");
    assert!(reader.is_file(), "the independent reader is committed");

    // 1476 through the binary: a directory packed as a declared extension.
    let plugins = dir.path().join("plugins");
    std::fs::create_dir_all(plugins.join("nested")).unwrap();
    std::fs::write(plugins.join("a.toml"), b"[a]\n").unwrap();
    std::fs::write(plugins.join("nested/b.bin"), b"bytes").unwrap();
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"com.example.ext\"\nname = \"Ext\"\nversion = \"1.0.0\"\n\
         entry = \"code.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n",
    )
    .unwrap();
    let component = dir.path().join("code.wasm");
    std::fs::write(&component, minimal).unwrap();
    let packed = dir.path().join("with-extension.krate");
    let output = krate()
        .arg("pack")
        .arg(&component)
        .arg("--manifest")
        .arg(&manifest)
        .arg("-o")
        .arg(&packed)
        .arg("--extension")
        .arg(format!("acme/plugins@3={}", plugins.display()))
        .output()
        .expect("pack");
    assert!(
        output.status.success(),
        "pack with an extension: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let inspected = krate()
        .args(["inspect", "--format", "json"])
        .arg(&packed)
        .output()
        .expect("inspect");
    assert!(
        inspected.status.success(),
        "{}",
        String::from_utf8_lossy(&inspected.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&inspected.stdout).expect("json");
    let ext = &report["extensions"][0];
    assert_eq!(ext["owner"], "acme", "{report}");
    assert_eq!(ext["name"], "plugins");
    assert_eq!(ext["version"], "3");
    assert_eq!(ext["size"], 4 + 5);
    assert_eq!(
        ext["digest"].as_str().map(str::len),
        Some(64),
        "a sha256 in hex"
    );
    assert_eq!(
        report["profile"], 2,
        "this build writes container profile 2"
    );
    assert!(
        report["identity"]["project"].is_string(),
        "an extension is project material, so there is a project identity: {report}"
    );
    let classes: Vec<(String, String)> = report["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["name"].as_str().unwrap().to_string(),
                r["class"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert!(
        classes.contains(&(
            "ext/acme/plugins/a.toml".to_string(),
            "extension".to_string()
        )),
        "{classes:?}"
    );
    assert!(
        classes.contains(&("extensions.json".to_string(), "extensions".to_string())),
        "{classes:?}"
    );
    let ran = krate()
        .arg("run")
        .arg(&packed)
        .args(["--headless", "--auto-grant"])
        .output()
        .expect("run");
    assert!(
        ran.status.success(),
        "an app with a declared extension still runs: {}",
        String::from_utf8_lossy(&ran.stderr)
    );
    let bad = krate()
        .arg("pack")
        .arg(&component)
        .arg("--manifest")
        .arg(&manifest)
        .arg("-o")
        .arg(dir.path().join("bad.krate"))
        .args(["--extension", "nonsense"])
        .output()
        .expect("pack");
    assert!(!bad.status.success());
    assert!(
        String::from_utf8_lossy(&bad.stderr).contains("OWNER/NAME@VERSION=DIR"),
        "a malformed --extension says the shape: {}",
        String::from_utf8_lossy(&bad.stderr)
    );

    // Refusals through the binary: a stray record under profile 2 (1475),
    // an extension that lies, and one nobody declared (1476).
    let declaration = |digest: &str| {
        format!(
            "{{\"schema\":\"krate.bundle.extensions.v1\",\"extensions\":[{{\"owner\":\"acme\",\
             \"name\":\"plugins\",\"version\":\"1\",\"size\":4,\"digest\":\"{digest}\"}}]}}"
        )
    };
    type Refusal<'a> = (&'a str, Vec<(String, Vec<u8>)>, &'a [&'a str]);
    let refusals: [Refusal; 3] = [
        (
            "a stray record under profile 2",
            vec![
                ("krate-profile".to_string(), b"2".to_vec()),
                ("notes.txt".to_string(), b"skipped by every reader".to_vec()),
            ],
            &["notes.txt", "ext/<owner>/<name>/"],
        ),
        (
            "an extension that is not what it declares",
            vec![
                (
                    "extensions.json".to_string(),
                    declaration(&"0".repeat(64)).into_bytes(),
                ),
                ("ext/acme/plugins/a.toml".to_string(), b"[a]\n".to_vec()),
            ],
            &["acme/plugins", "not what its declaration says"],
        ),
        (
            "an extension nobody declared",
            vec![
                ("krate-profile".to_string(), b"2".to_vec()),
                ("ext/evil/thing/x".to_string(), b"smuggled".to_vec()),
            ],
            &["evil/thing", "without declaring it"],
        ),
    ];
    let mut corpus: Vec<(String, PathBuf)> =
        vec![("packed with an extension".to_string(), packed.clone())];
    for (what, extra, words) in &refusals {
        let path = dir.path().join(format!("{}.krate", what.replace(' ', "-")));
        std::fs::write(&path, archive_with_component(minimal, extra)).unwrap();
        let output = krate()
            .arg("run")
            .arg(&path)
            .args(["--headless", "--auto-grant"])
            .output()
            .expect("run");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_ne!(
            output.status.code(),
            Some(0),
            "{what} must be refused: {stderr}"
        );
        for word in *words {
            assert!(
                stderr.contains(word),
                "{what}: the refusal must say {word:?}: {stderr}"
            );
        }
        corpus.push((what.to_string(), path));
    }

    // 1477: the independent reader against the binary, on everything.
    for entry in std::fs::read_dir(repo.join("evidence/ported")).expect("ported bundles") {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "krate") {
            corpus.push((
                format!("ported {}", path.file_name().unwrap().to_string_lossy()),
                path,
            ));
        }
    }
    for fixture in [
        "crates/bundle/tests/fixtures/duplicate-from-another-writer.krate",
        "crates/bundle/tests/fixtures/locked-entry.krate",
        "crates/krate-hub/tests/fixtures/duplicate-source-path.krate",
        "crates/krate-hub/tests/fixtures/forged-size-bomb.krate",
    ] {
        corpus.push((fixture.to_string(), repo.join(fixture)));
    }
    let mut accepted = 0;
    let mut refused = 0;
    for (what, path) in &corpus {
        let ours = krate()
            .args(["inspect", "--format", "json"])
            .arg(path)
            .output()
            .expect("inspect");
        let theirs = Command::new("python3")
            .arg(&reader)
            .arg(path)
            .output()
            .expect("python3 must be on PATH: the independent reader is part of this test");
        let theirs_json: serde_json::Value =
            serde_json::from_slice(&theirs.stdout).unwrap_or_else(|err| {
                panic!(
                    "{what}: the reader printed no JSON ({err}): {}",
                    String::from_utf8_lossy(&theirs.stderr)
                )
            });
        match (ours.status.success(), theirs.status.success()) {
            (true, true) => {
                let ours_json: serde_json::Value = serde_json::from_slice(&ours.stdout).unwrap();
                for key in ["profile", "records", "extensions", "closure"] {
                    assert_eq!(
                        ours_json[key], theirs_json[key],
                        "{what}: the two readers resolve a different {key} (1477)"
                    );
                }
                accepted += 1;
            }
            (false, false) => refused += 1,
            (ours_ok, _) => panic!(
                "{what}: one reader accepts what the other refuses (1477): krate {}: {} / reader: {}",
                if ours_ok { "accepted" } else { "refused" },
                String::from_utf8_lossy(&ours.stderr),
                theirs_json["refused"]
            ),
        }
    }
    assert!(
        accepted >= 8,
        "the corpus must carry bundles both accept: {accepted}"
    );
    assert!(refused >= 6, "and bundles both refuse: {refused}");
}

/// An app's saved data leaves and comes back as one file, to the app it
/// came from, checked against its own record, and never to another app by
/// accident (IC-736, test 1520; K-298's export bullet).
#[test]
fn saved_data_exports_and_imports_to_the_app_it_came_from() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = tempfile::tempdir().expect("home");
    let elsewhere = tempfile::tempdir().expect("second home");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(
        &wasm,
        include_bytes!("../../bundle/tests/fixtures/minimal-run.wasm"),
    )
    .unwrap();
    let pack = |id: &str| {
        let manifest = dir.path().join(format!("{id}.toml"));
        std::fs::write(
            &manifest,
            format!(
                "[app]\nid = \"{id}\"\nname = \"{id}\"\nversion = \"1.0.0\"\n\
                 entry = \"code.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n"
            ),
        )
        .unwrap();
        let bundle = dir.path().join(format!("{id}.krate"));
        assert!(krate()
            .arg("pack")
            .arg(&wasm)
            .arg("--manifest")
            .arg(&manifest)
            .arg("-o")
            .arg(&bundle)
            .status()
            .unwrap()
            .success());
        bundle
    };
    let notes = pack("dev.krate.notes");
    let other = pack("dev.krate.other");

    // Data the person made, where the runtime keeps an unsigned app's.
    let store = home.path().join(".krate/store");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::write(store.join("dev.krate.notes.kv"), b"what the person wrote").unwrap();
    std::fs::write(store.join("dev.krate.notes.sqlite"), b"and their table").unwrap();
    std::fs::write(store.join("dev.krate.notes.shared.json"), b"{}").unwrap();

    // Nothing to export is said, not written.
    let none = dir.path().join("none.krate-data");
    let out = krate()
        .args(["data", "export"])
        .arg(&other)
        .arg("-o")
        .arg(&none)
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(!none.exists(), "no file for no data");
    assert!(String::from_utf8_lossy(&out.stderr).contains("no saved data"));

    // Export: one file, a record that names every part with its digest.
    let copy = dir.path().join("notes.krate-data");
    let out = krate()
        .args(["data", "export"])
        .arg(&notes)
        .arg("-o")
        .arg(&copy)
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(
        said.contains("3 files") && said.contains("dev.krate.notes"),
        "{said}"
    );
    let mut archive = zip::ZipArchive::new(std::fs::File::open(&copy).unwrap()).unwrap();
    let record: serde_json::Value = {
        let mut text = String::new();
        std::io::Read::read_to_string(&mut archive.by_name("export.json").unwrap(), &mut text)
            .unwrap();
        serde_json::from_str(&text).unwrap()
    };
    assert_eq!(record["schema"], "krate.data-export.v1");
    assert_eq!(record["app"]["id"], "dev.krate.notes");
    assert_eq!(record["principal"]["kind"], "unverified");
    let files = record["files"].as_array().unwrap();
    assert_eq!(files.len(), 3, "{record}");
    let kv = files
        .iter()
        .find(|f| f["kind"] == "kv")
        .expect("the kv file is recorded");
    assert_eq!(kv["bytes"], 21);
    assert_eq!(kv["sha256"].as_str().map(str::len), Some(64));
    assert_eq!(kv["machine_bound"], false);
    let mut body = Vec::new();
    std::io::Read::read_to_end(
        &mut archive.by_name("data/dev.krate.notes.kv").unwrap(),
        &mut body,
    )
    .unwrap();
    assert_eq!(body, b"what the person wrote");
    drop(archive);

    // Import on another machine: the same files, where the app looks.
    let import = |home: &std::path::Path, into: &std::path::Path, extra: &[&str]| {
        krate()
            .args(["data", "import"])
            .arg(&copy)
            .arg("--into")
            .arg(into)
            .args(extra)
            .env("HOME", home)
            .output()
            .unwrap()
    };
    let out = import(elsewhere.path(), &notes, &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let restored = elsewhere.path().join(".krate/store");
    for (name, body) in [
        ("dev.krate.notes.kv", b"what the person wrote".as_slice()),
        ("dev.krate.notes.sqlite", b"and their table"),
        ("dev.krate.notes.shared.json", b"{}"),
    ] {
        assert_eq!(std::fs::read(restored.join(name)).unwrap(), body, "{name}");
    }
    assert!(
        !restored.join("dev.krate.notes.partial").exists(),
        "no staging litter"
    );

    // Existing data is not overwritten by accident.
    std::fs::write(restored.join("dev.krate.notes.kv"), b"newer notes").unwrap();
    let out = import(elsewhere.path(), &notes, &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--replace"));
    assert_eq!(
        std::fs::read(restored.join("dev.krate.notes.kv")).unwrap(),
        b"newer notes"
    );
    let out = import(elsewhere.path(), &notes, &["--replace"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read(restored.join("dev.krate.notes.kv")).unwrap(),
        b"what the person wrote"
    );

    // Another app is another namespace: refused unless said on purpose.
    let out = import(elsewhere.path(), &other, &[]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("dev.krate.other") && stderr.contains("--accept-different-app"),
        "{stderr}"
    );
    assert!(!restored.join("dev.krate.other.kv").exists());
    let out = import(elsewhere.path(), &other, &["--accept-different-app"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read(restored.join("dev.krate.other.kv")).unwrap(),
        b"what the person wrote"
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("because you said so"));

    // A damaged copy is refused whole: nothing restored, not even the
    // parts that were fine.
    let damaged = dir.path().join("damaged.krate-data");
    {
        let mut source = zip::ZipArchive::new(std::fs::File::open(&copy).unwrap()).unwrap();
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&damaged).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        for index in 0..source.len() {
            let mut entry = source.by_index(index).unwrap();
            let name = entry.name().to_string();
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut bytes).unwrap();
            if name == "data/dev.krate.notes.sqlite" {
                bytes = b"and their tabl3".to_vec();
            }
            writer.start_file(name, options).unwrap();
            std::io::Write::write_all(&mut writer, &bytes).unwrap();
        }
        writer.finish().unwrap();
    }
    let third = tempfile::tempdir().expect("third home");
    let out = krate()
        .args(["data", "import"])
        .arg(&damaged)
        .arg("--into")
        .arg(&notes)
        .env("HOME", third.path())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("not what the record says"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !third
            .path()
            .join(".krate/store/dev.krate.notes.kv")
            .exists(),
        "a damaged copy must not be restored by halves"
    );

    // Uninstall takes the copy first, and deletes nothing when the copy
    // cannot be written. Only where installing is supported.
    let prefix = tempfile::tempdir().expect("prefix");
    let installed = krate()
        .arg("install")
        .arg(&notes)
        .arg("--prefix")
        .arg(prefix.path())
        .env("HOME", home.path())
        .output()
        .unwrap();
    if cfg!(windows) {
        // Installing apps is not supported on Windows (the installer does
        // the file associations there), so the uninstall half has nothing
        // to test. Said with the platform in the sentence: the test report
        // reads "on windows" as a deliberate platform gap, and anything
        // else as a fixture that failed to build -- which is how the first
        // Windows lane to get past K-240 failed with every test green.
        eprintln!("skipping the uninstall half: installing apps is not supported on windows");
        return;
    }
    // Everywhere else installing is supported, so a refusal here is a real
    // failure and not a platform to skip.
    assert!(
        installed.status.success(),
        "install refused on a platform that supports it: {}",
        String::from_utf8_lossy(&installed.stderr)
    );
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, b"a file where a directory would need to be").unwrap();
    let out = krate()
        .args(["uninstall", "dev.krate.notes", "--delete-data", "--export"])
        .arg(blocker.join("copy.krate-data"))
        .arg("--prefix")
        .arg(prefix.path())
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "an export that cannot be written stops the uninstall"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("nothing was deleted"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        store.join("dev.krate.notes.kv").exists(),
        "the data is still there"
    );
    let listed = krate()
        .arg("installed")
        .arg("--prefix")
        .arg(prefix.path())
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&listed.stdout).contains("dev.krate.notes"),
        "and so is the app"
    );
    let taken = dir.path().join("taken.krate-data");
    let out = krate()
        .args(["uninstall", "dev.krate.notes", "--delete-data", "--export"])
        .arg(&taken)
        .arg("--prefix")
        .arg(prefix.path())
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(taken.is_file(), "the copy exists");
    assert!(
        !store.join("dev.krate.notes.kv").exists(),
        "and then the data is gone"
    );
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(
        said.contains("Copied the saved data") && said.contains("deleted"),
        "{said}"
    );
    // And the copy is a real one: it restores.
    let fourth = tempfile::tempdir().expect("fourth home");
    let out = krate()
        .args(["data", "import"])
        .arg(&taken)
        .arg("--into")
        .arg(&notes)
        .env("HOME", fourth.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read(fourth.path().join(".krate/store/dev.krate.notes.kv")).unwrap(),
        b"what the person wrote"
    );
}

/// A wasm binary with every `name` custom section removed, at the
/// component level and inside each core module. Symbol names carry a hash
/// of the crate's metadata, which includes WHERE a path dependency sits on
/// disk, so a rebuild from a bundle differs from the original in exactly
/// those sections and nowhere else (measured on krate-hello-gui: 456 bytes
/// of mangled-symbol hashes, identical everywhere else).
fn without_name_sections(bytes: &[u8]) -> Vec<u8> {
    fn leb(bytes: &[u8], mut at: usize) -> (usize, usize) {
        let (mut value, mut shift) = (0usize, 0u32);
        loop {
            let byte = bytes[at];
            at += 1;
            value |= ((byte & 0x7f) as usize) << shift;
            shift += 7;
            if byte < 0x80 {
                return (value, at);
            }
        }
    }
    fn encode(mut n: usize, out: &mut Vec<u8>) {
        loop {
            let byte = (n & 0x7f) as u8;
            n >>= 7;
            if n == 0 {
                out.push(byte);
                return;
            }
            out.push(byte | 0x80);
        }
    }
    fn strip(bytes: &[u8], nested: bool) -> Vec<u8> {
        let mut out = bytes[..8].to_vec();
        let mut at = 8;
        while at < bytes.len() {
            let id = bytes[at];
            let (size, start) = leb(bytes, at + 1);
            let payload = &bytes[start..start + size];
            at = start + size;
            if id == 0 {
                let (len, name_at) = leb(payload, 0);
                if &payload[name_at..name_at + len] == b"name" {
                    continue;
                }
            }
            let payload = if id == 1 && !nested && payload.starts_with(b"\0asm") {
                strip(payload, true)
            } else {
                payload.to_vec()
            };
            out.push(id);
            encode(payload.len(), &mut out);
            out.extend_from_slice(&payload);
        }
        out
    }
    strip(bytes, false)
}

/// An editable bundle carries its whole closure, and rebuilds from it on
/// another machine (CP1: "Editable bundles carry the full closure: source,
/// Cargo.lock, bindings, toolchain identity, SDK -- and provably rebuild on
/// another machine"; K-339, K-340).
///
/// krate-hello-gui is built in the checkout, packed with its source and
/// the SDK, unpacked somewhere that is not the checkout, and rebuilt with
/// `--locked` against nothing but what the bundle holds. The rebuilt
/// component is the original, byte for byte, once the symbol-name sections
/// are set aside -- they hash the SDK's path, which is the one thing that
/// legitimately differs between two machines.
#[test]
fn an_editable_bundle_rebuilds_locked_from_what_it_carries() {
    if !has_cargo_component() {
        eprintln!("skipping the rebuild proof: cargo-component is not on PATH");
        return;
    }
    // Two cargo-component builds at once make the other one skip
    // componentizing its artifact (measured: the skeleton test beside this
    // one packed a core module on macOS CI while this test was building).
    let _build_lock = cargo_build_guard();
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let app = repo.join("apps/krate-hello-gui");
    let toolchain = repo.join("rust-toolchain.toml");

    // The original, built in the checkout the way CI builds it.
    let mut build_cmd = Command::new("cargo-component");
    build_cmd.args(["build", "--release"]).current_dir(&app);
    // 15 minutes: a cold cargo cache on CI is slow, a hang is unbounded.
    let built = output_bounded(build_cmd, 15 * 60)
        .expect("cargo-component build did not finish within 15 minutes (K-240)");
    assert!(
        built.status.success(),
        "building krate-hello-gui: {}",
        String::from_utf8_lossy(&built.stderr)
    );
    let original = app.join("target/wasm32-wasip1/release/krate_hello_gui.wasm");
    let original_bytes = std::fs::read(&original).expect("the built component");

    // Packed with source and SDK, as `krate pack` does for a crate.
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = dir.path().join("hello.krate");
    let packed = krate()
        .arg("pack")
        .arg(&original)
        .arg("--manifest")
        .arg(app.join("manifest.toml"))
        .arg("-o")
        .arg(&bundle)
        .output()
        .expect("pack");
    assert!(
        packed.status.success(),
        "{}",
        String::from_utf8_lossy(&packed.stderr)
    );

    // What the bundle says it carries.
    let inspected = krate()
        .args(["inspect", "--format", "json"])
        .arg(&bundle)
        .output()
        .expect("inspect");
    let report: serde_json::Value = serde_json::from_slice(&inspected.stdout).expect("json");
    let closure = &report["closure"];
    assert_eq!(closure["locked"], true, "Cargo.lock travels: {closure}");
    let tools: Vec<String> = closure["processed_by"]
        .as_array()
        .expect("producers")
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_string())
        .collect();
    assert!(
        tools.contains(&"rustc".to_string()) && tools.contains(&"wit-component".to_string()),
        "the toolchain is measured from the component itself: {tools:?}"
    );
    assert!(
        closure["sdk"]["digest"].is_string(),
        "the SDK is recorded: {closure}"
    );

    // Unpacked elsewhere: only the bundle's own contents, and the
    // placeholder pointed at the SDK it carries.
    let elsewhere = dir.path().join("elsewhere");
    {
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&bundle).unwrap()).expect("zip");
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).unwrap();
            let name = entry.name().to_string();
            if !(name.starts_with("source/") || name.starts_with("sdk/")) {
                continue;
            }
            let path = elsewhere.join(&name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut bytes).unwrap();
            std::fs::write(&path, bytes).unwrap();
        }
    }
    let source = elsewhere.join("source");
    assert!(
        source.join("Cargo.lock").is_file(),
        "the lock is in the bundle"
    );
    let manifest_text = std::fs::read_to_string(source.join("Cargo.toml")).unwrap();
    assert!(
        !manifest_text.contains("../../"),
        "no path into the checkout survives packing: {manifest_text}"
    );
    let sdk = elsewhere.join("sdk");
    std::fs::write(
        source.join("Cargo.toml"),
        manifest_text.replace("{KRATE_SDK}", &sdk.to_string_lossy().replace('\\', "/")),
    )
    .unwrap();
    std::fs::copy(&toolchain, source.join("rust-toolchain.toml")).unwrap();

    let mut rebuild_cmd = Command::new("cargo-component");
    rebuild_cmd
        .args(["build", "--release", "--locked"])
        .current_dir(&source);
    let rebuilt = output_bounded(rebuild_cmd, 15 * 60)
        .expect("cargo-component rebuild did not finish within 15 minutes (K-240)");
    assert!(
        rebuilt.status.success(),
        "the rebuild must resolve to the lock the bundle carries, against the SDK it carries: {}",
        String::from_utf8_lossy(&rebuilt.stderr)
    );
    let rebuilt_bytes =
        std::fs::read(source.join("target/wasm32-wasip1/release/krate_hello_gui.wasm")).unwrap();
    assert_eq!(
        without_name_sections(&rebuilt_bytes),
        without_name_sections(&original_bytes),
        "the rebuilt component must be the original, symbol names aside"
    );
    assert_ne!(
        without_name_sections(&original_bytes).len(),
        original_bytes.len(),
        "the stripping must have removed something, or it proved nothing"
    );
}

fn archive_with_component(component: &[u8], extra: &[(String, Vec<u8>)]) -> Vec<u8> {
    const MANIFEST: &str = "[app]\nid = \"com.example.adversarial\"\nname = \"Adversarial\"\n\
                            version = \"1.0.0\"\nentry = \"code.wasm\"\n\
                            world = \"krate:app/cli@0.1.0\"\n";
    raw_bundle(MANIFEST, component, extra)
}

/// A hand-assembled archive (every entry stored, no compression) around
/// exactly these bytes: the shape another writer, or an attacker, produces.
fn raw_bundle(manifest: &str, component: &[u8], extra: &[(String, Vec<u8>)]) -> Vec<u8> {
    let entries: Vec<(String, Vec<u8>)> = [
        ("manifest.toml".to_string(), manifest.as_bytes().to_vec()),
        // A real component with a `run` export. `\0asm\x01\0\0\0` is a
        // core MODULE, which is what almost every fixture used until pack
        // learned to tell them apart (K-272); a bare component header is a
        // component that could never run, which open refuses since it
        // validates the component (IC-210). These archives are assembled by
        // hand rather than packed, so nothing forced either correction --
        // but a fixture that is not the thing it claims to be is worth
        // fixing on sight.
        ("code.wasm".to_string(), component.to_vec()),
    ]
    .into_iter()
    .chain(extra.iter().cloned())
    .collect();

    let mut out = Vec::new();
    let mut offsets = Vec::new();
    for (name, body) in &entries {
        offsets.push(out.len() as u32);
        out.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04]);
        out.extend_from_slice(&[10, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        out.extend_from_slice(&zip_crc32(body).to_le_bytes());
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(body);
    }

    let cd_start = out.len() as u32;
    for ((name, body), offset) in entries.iter().zip(&offsets) {
        out.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02]);
        out.extend_from_slice(&[10, 0, 10, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        out.extend_from_slice(&zip_crc32(body).to_le_bytes());
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
    }
    let cd_len = out.len() as u32 - cd_start;

    out.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06]);
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&cd_len.to_le_bytes());
    out.extend_from_slice(&cd_start.to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out
}

fn zip_crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in bytes {
        crc ^= *byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

fn configured_hello_component() -> Option<PathBuf> {
    configured_component_from_env("KRATE_HELLO_WASM", "hello component test")
}

fn configured_phase2_smoke_component() -> Option<PathBuf> {
    configured_component_from_env("KRATE_PHASE2_SMOKE_WASM", "Phase 2 smoke component test")
}

fn configured_krate_clock_component() -> Option<PathBuf> {
    configured_component_from_env("KRATE_CLOCK_WASM", "krate-clock component test")
}

fn configured_krate_cat_component() -> Option<PathBuf> {
    configured_component_from_env("KRATE_CAT_WASM", "krate-cat component test")
}

fn configured_krate_curl_component() -> Option<PathBuf> {
    configured_component_from_env("KRATE_CURL_WASM", "krate-curl component test")
}

fn configured_go_component(env: &str, label: &str, filename: &str) -> Option<PathBuf> {
    configured_component_from_env_or_paths(
        env,
        label,
        &[format!("test/integration/language-variants/{filename}")],
    )
}

fn configured_ts_component(env: &str, label: &str, filename: &str) -> Option<PathBuf> {
    configured_component_from_env_or_paths(
        env,
        label,
        &[format!("test/integration/language-variants/{filename}")],
    )
}

fn configured_component_from_env(env: &str, label: &str) -> Option<PathBuf> {
    configured_component_from_env_or_paths(env, label, &[])
}

fn configured_component_from_env_or_paths(
    env: &str,
    label: &str,
    fallback_paths: &[String],
) -> Option<PathBuf> {
    let Some(path) = std::env::var_os(env) else {
        for fallback in fallback_paths {
            let fallback = workspace_path(PathBuf::from(fallback));
            if fallback.exists() {
                return Some(fallback);
            }
        }
        eprintln!("skipping {label}: {env} is not set");
        return None;
    };

    Some(workspace_path(PathBuf::from(path)))
}

/// The usability report carries a keyboard observation, through the real
/// binary on a real bundle (IC-743, test 1548).
///
/// bounce.krate draws its own controls on a canvas, so the honest keyboard
/// verdict is "unobserved" with the reason that there is no widget to focus.
/// That is the case this can prove without building a widget-tree app; the
/// held and broke verdicts are proven by the host's own tests, and the
/// hello-gui sample was driven by hand: its list row answered the pointer
/// and not Enter until the runtime made Enter a press.
#[test]
fn the_usability_report_says_what_the_keyboard_did() {
    let bundle = workspace_path(PathBuf::from("evidence/ported/bounce.krate"));
    if !bundle.exists() {
        eprintln!("skipping: {} is not present", bundle.display());
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let report = dir.path().join("usability.json");
    let output = Command::new(env!("CARGO_BIN_EXE_krate"))
        .args(["run", "--headless", "--auto-grant", "--usability-report"])
        .arg(&report)
        .arg(&bundle)
        .env("KRATE_NO_USAGE", "1")
        .output()
        .expect("run krate");
    assert!(
        report.exists(),
        "no usability report was written; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = std::fs::read_to_string(&report).expect("read report");
    let json: serde_json::Value = serde_json::from_str(&text).expect("report is json");
    assert_eq!(
        json["opened_window"],
        serde_json::Value::Bool(true),
        "{text}"
    );
    let keyboard = &json["keyboard"];
    assert!(
        !keyboard.is_null(),
        "the report must say what the keyboard did, even when it could not be tried: {text}"
    );
    assert_eq!(
        keyboard["outcome"], "unobserved",
        "a canvas app has nothing to focus: {text}"
    );
    assert!(
        keyboard["reason"]
            .as_str()
            .unwrap_or("")
            .contains("no widget to focus"),
        "the reason must say why: {text}"
    );
}

fn bind_local_fixture_listener(label: &str) -> Option<TcpListener> {
    match TcpListener::bind("127.0.0.1:0") {
        Ok(listener) => Some(listener),
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
            eprintln!("skipping {label}: localhost bind is blocked ({err})");
            None
        }
        Err(err) => panic!("bind {label}: {err}"),
    }
}

/// An HTTP fixture that answers every request with a redirect elsewhere.
///
/// Used to prove the client does not follow it. `net.connect` is granted per
/// host, so a client that followed redirects itself would let one granted host
/// send the app's request anywhere, while the person's prompt named only the
/// first.
fn spawn_redirect_fixture(
    location: &'static str,
) -> Option<(SocketAddr, thread::JoinHandle<bool>)> {
    let listener = bind_local_fixture_listener("redirect fixture")?;
    listener
        .set_nonblocking(true)
        .expect("set redirect fixture nonblocking");
    let addr = listener
        .local_addr()
        .expect("read redirect fixture address");
    let handle = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return false;
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(err) => panic!("accept redirect fixture connection: {err}"),
            }
        };
        stream
            .set_nonblocking(false)
            .expect("set redirect fixture stream blocking");
        let mut request = [0_u8; 1024];
        let _ = stream.read(&mut request).expect("read redirect request");
        let wrote = write!(
            stream,
            "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        !http_fixture_client_closed(wrote, "redirect headers")
    });
    Some((addr, handle))
}

fn spawn_http_fixture(body: &'static [u8]) -> Option<(SocketAddr, thread::JoinHandle<bool>)> {
    let listener = bind_local_fixture_listener("HTTP fixture")?;
    listener
        .set_nonblocking(true)
        .expect("set HTTP fixture nonblocking");
    let addr = listener.local_addr().expect("read HTTP fixture address");
    let handle = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return false;
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(err) => panic!("accept HTTP fixture connection: {err}"),
            }
        };
        stream
            .set_nonblocking(false)
            .expect("set HTTP fixture stream blocking");
        let mut request = [0_u8; 1024];
        let _ = stream
            .read(&mut request)
            .expect("read HTTP fixture request");
        let headers_result = write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n",
            body.len()
        );
        if http_fixture_client_closed(headers_result, "headers") {
            return true;
        }
        if http_fixture_client_closed(stream.write_all(body), "response body") {
            return true;
        }
        true
    });

    Some((addr, handle))
}

fn http_fixture_client_closed(result: std::io::Result<()>, label: &str) -> bool {
    match result {
        Ok(()) => false,
        Err(err)
            if matches!(
                err.kind(),
                ErrorKind::BrokenPipe | ErrorKind::ConnectionAborted | ErrorKind::ConnectionReset
            ) =>
        {
            eprintln!("HTTP fixture client closed while writing {label}: {err}");
            true
        }
        Err(err) => panic!("write HTTP fixture {label}: {err}"),
    }
}

fn reserve_unused_local_addr() -> Option<SocketAddr> {
    let listener = bind_local_fixture_listener("local address probe")?;
    let addr = listener
        .local_addr()
        .expect("read local address probe port");
    drop(listener);
    Some(addr)
}

fn spawn_malformed_http_fixture(
    payload: &'static [u8],
) -> Option<(SocketAddr, thread::JoinHandle<bool>)> {
    let listener = bind_local_fixture_listener("malformed HTTP fixture")?;
    listener
        .set_nonblocking(true)
        .expect("set malformed HTTP fixture nonblocking");
    let addr = listener
        .local_addr()
        .expect("read malformed HTTP fixture address");
    let handle = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return false;
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(err) => panic!("accept malformed HTTP fixture connection: {err}"),
            }
        };
        stream
            .set_nonblocking(false)
            .expect("set malformed HTTP fixture stream blocking");
        let mut request = [0_u8; 1024];
        let _ = stream
            .read(&mut request)
            .expect("read malformed HTTP fixture request");
        stream
            .write_all(payload)
            .expect("write malformed HTTP fixture response");
        true
    });
    Some((addr, handle))
}

fn spawn_stalling_http_fixture(wait: Duration) -> Option<(SocketAddr, thread::JoinHandle<bool>)> {
    let listener = bind_local_fixture_listener("stalling HTTP fixture")?;
    listener
        .set_nonblocking(true)
        .expect("set stalling HTTP fixture nonblocking");
    let addr = listener
        .local_addr()
        .expect("read stalling HTTP fixture address");
    let handle = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return false;
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(err) => panic!("accept stalling HTTP fixture connection: {err}"),
            }
        };
        stream
            .set_nonblocking(false)
            .expect("set stalling HTTP fixture stream blocking");
        let mut request = [0_u8; 1024];
        let _ = stream
            .read(&mut request)
            .expect("read stalling HTTP fixture request");
        thread::sleep(wait);
        let _ = stream.flush();
        true
    });
    Some((addr, handle))
}

fn expected_hello_hash() -> Option<String> {
    std::env::var("KRATE_HELLO_SHA256")
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
}

// ---------------------------------------------------------------------------
// .krate bundles (P3-SHARE-01)
//
// The property under test is that packaging a component changes how it is
// delivered and nothing about what it is allowed to do. Every assertion below
// compares a bundle against the sidecar-manifest path it must match exactly.
// ---------------------------------------------------------------------------

const BUNDLE_MANIFEST: &str = r#"
[app]
id = "com.example.bundle"
name = "Bundle Demo"
version = "0.1.0"
entry = "code.wasm"
world = "krate:app/cli@0.1.0"

[[capabilities]]
cap = "io.stdout"
rationale = "print"
required = true
"#;

/// Minimal valid component: the phase 2 smoke fixture built by CI.
fn smoke_component() -> Option<PathBuf> {
    // The lane that builds the fixture hands it over in
    // KRATE_PHASE2_SMOKE_WASM; a developer's checkout has it at the build
    // path below. Only the second was looked at, so the six gift-and-wrap
    // tests behind this helper skipped in EVERY CI run while the lane's
    // count said they passed -- the categorised report found them (IC-706).
    // No skip line here: each caller prints its own, and one test printing
    // two would break the report's reconciliation.
    if let Some(path) = std::env::var_os("KRATE_PHASE2_SMOKE_WASM") {
        let path = workspace_path(PathBuf::from(path));
        if path.exists() {
            return Some(path);
        }
    }
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../test/integration/phase2-smoke/target/wasm32-wasip1/release/phase2_smoke.wasm");
    path.exists().then_some(path)
}

fn pack_fixture(dir: &std::path::Path) -> Option<PathBuf> {
    let component = smoke_component()?;
    let manifest = dir.join("manifest.toml");
    std::fs::write(&manifest, BUNDLE_MANIFEST).expect("write manifest");
    let wasm = dir.join("code.wasm");
    std::fs::copy(&component, &wasm).expect("copy component");
    let bundle = dir.join("demo.krate");

    let status = krate()
        .args(["pack"])
        .arg(&wasm)
        .arg("--manifest")
        .arg(&manifest)
        .arg("-o")
        .arg(&bundle)
        .status()
        .expect("run krate pack");
    assert!(status.success(), "pack should succeed");
    Some(bundle)
}

/// The gift a friend without Krate receives. Three properties, and every
/// one of them has already been wrong at least once:
///
/// 1. The Mac gift is a FOLDER, not a script. A downloaded `.command`
///    cannot pass Gatekeeper -- signing gets it as far as "Unnotarized
///    Developer ID" and it can never go further, because `stapler` refuses
///    the format outright (K-211). An `.app` is a shape Apple will notarize.
/// 2. The payload sits BESIDE the opener, never inside it. Dropping it into
///    a signed bundle's Resources breaks the seal, which is what killed the
///    notarize-once-parameterize-later design.
/// 3. The copied bundle still opens as the same app.
#[test]
fn a_mac_gift_is_a_notarizable_app_with_the_payload_beside_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(bundle) = pack_fixture(dir.path()) else {
        eprintln!("skipping: phase2 smoke fixture not built");
        return;
    };
    let out = dir.path().join("gift");
    let status = krate()
        .args(["wrap", "--for", "mac"])
        .arg(&bundle)
        .arg("-o")
        .arg(&out)
        .status()
        .expect("run krate wrap");
    assert!(status.success(), "wrap should succeed");

    // The opener is a real bundle: an executable Launch Services can start,
    // and a plist naming it.
    let opener: PathBuf = std::fs::read_dir(&out)
        .expect("read gift dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.extension().map(|e| e == "app").unwrap_or(false))
        .expect("the gift should contain a .app opener");
    let exe = opener.join("Contents/MacOS/open");
    assert!(exe.is_file(), "the opener needs an executable");
    assert!(
        opener.join("Contents/Info.plist").is_file(),
        "the opener needs an Info.plist or Launch Services will not run it",
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&exe)
            .expect("opener metadata")
            .permissions()
            .mode();
        assert!(mode & 0o111 != 0, "the opener must be executable");
    }

    // The payload is a sibling, not a sealed resource.
    let payload: PathBuf = std::fs::read_dir(&out)
        .expect("read gift dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.extension().map(|e| e == "krate").unwrap_or(false))
        .expect("the app file should sit beside the opener");
    assert!(
        !payload.starts_with(&opener),
        "the payload must not live inside the signed bundle: that breaks the seal",
    );

    // The opener must NOT name a particular app. One opener is notarized per
    // release and copied into every gift, so a script naming this app could
    // only ever be signed for this app -- and the sender's laptop has no
    // certificate. It finds the .krate beside it instead.
    let script = std::fs::read_to_string(&exe).expect("read opener");
    let name = payload.file_name().unwrap().to_string_lossy().to_string();
    assert!(
        !script.contains(&name),
        "the opener must not bake in {name}: it has to serve every gift",
    );
    assert!(
        script.contains("*.krate"),
        "the opener should find the app file beside it",
    );
    // A Mac without Krate installs the player the gift carries -- only if
    // Krate's own Developer ID signed it, and never by running something
    // fetched from the network (IC-381).
    assert!(
        script.contains("DBYD4AW5Q8") && script.contains("signed_by_krate \"$work/krate\""),
        "the opener must check Krate signed the player, on the copy it runs",
    );
    assert!(
        script.contains("player-install"),
        "the opener installs the carried player"
    );
    assert!(
        !script.contains("curl") && !script.contains("| sh"),
        "the opener must never run what a server sends",
    );

    // And the copy is still a readable app: --dump-caps opens the bundle
    // and reports its identity without running it.
    let read = krate()
        .arg("run")
        .arg(&payload)
        .arg("--dump-caps")
        .output()
        .expect("run krate run --dump-caps");
    assert!(
        read.status.success(),
        "the gifted bundle should still be readable: {}",
        String::from_utf8_lossy(&read.stderr),
    );
}

/// The other two platforms keep the single self-installing file, and it has
/// to stay a readable bundle behind its script prefix -- `krate run` opens
/// the wrap itself. This is the property the concatenation trick rests on,
/// and nothing tested it before.
#[test]
fn a_linux_wrap_is_one_file_that_is_still_a_bundle() {
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(bundle) = pack_fixture(dir.path()) else {
        eprintln!("skipping: phase2 smoke fixture not built");
        return;
    };
    let out = dir.path().join("gift.sh");
    let status = krate()
        .args(["wrap", "--for", "linux"])
        .arg(&bundle)
        .arg("-o")
        .arg(&out)
        .status()
        .expect("run krate wrap");
    assert!(status.success(), "wrap should succeed");

    let bytes = std::fs::read(&out).expect("read wrap");
    assert!(
        bytes.starts_with(b"#!/bin/sh"),
        "a wrap must run as a script"
    );
    assert!(
        bytes.windows(4).any(|w| w == b"PK\x03\x04"),
        "the bundle must still be in there behind the prefix",
    );
    let read = krate()
        .arg("run")
        .arg(&out)
        .arg("--dump-caps")
        .output()
        .expect("run krate run --dump-caps");
    assert!(
        read.status.success(),
        "krate must still read the app out of its own wrap: {}",
        String::from_utf8_lossy(&read.stderr),
    );
}

/// `krate player-install` puts the player where the person owns it and
/// declares `.krate` for it -- the step a gift's first open runs. A second
/// run keeps what is there.
#[test]
fn player_install_puts_a_registered_player_in_the_home_folder() {
    let home = tempfile::tempdir().expect("tempdir");
    let run = || {
        krate()
            .arg("player-install")
            .arg("--home")
            .arg(home.path())
            .env("KRATE_PLAYER_NO_REGISTER", "1")
            .output()
            .expect("run krate player-install")
    };
    let out = run();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    let engine = text
        .lines()
        .rev()
        .find_map(|l| l.strip_prefix("Player: "))
        .expect("it says where the player is");
    let engine = std::path::Path::new(engine.trim());
    assert!(
        engine.starts_with(home.path()),
        "inside the home folder: {}",
        engine.display()
    );
    assert!(engine.is_file(), "the player is there");
    #[cfg(target_os = "macos")]
    {
        let app = home.path().join("Applications/Krate Player.app");
        let plist = std::fs::read_to_string(app.join("Contents/Info.plist")).expect("plist");
        assert!(
            plist.contains("dev.krate.bundle"),
            "the player declares .krate"
        );
        assert!(
            plist.contains("<string>Krate</string>"),
            "and starts through its shim"
        );
        assert!(app.join("Contents/MacOS/Krate").is_file());
    }
    #[cfg(target_os = "linux")]
    {
        let data = home.path().join(".local/share");
        assert!(
            data.join("mime/packages/krate.xml").is_file(),
            "the .krate MIME type"
        );
        let desktop = std::fs::read_to_string(data.join("applications/dev.krate.open.desktop"))
            .expect("the launcher");
        assert!(desktop.contains("MimeType=application/x-krate;"));
        assert!(
            desktop.contains(&engine.display().to_string()),
            "it runs the installed player"
        );
    }
    let again = run();
    assert!(again.status.success());
    assert!(String::from_utf8_lossy(&again.stdout).contains(&engine.display().to_string()));
}

/// The Mac gift is ONE file: a disk image with the opener, the app and the
/// player inside (IC-383). Mounting it shows exactly that.
#[cfg(target_os = "macos")]
#[test]
fn a_mac_gift_is_one_disk_image_carrying_the_player() {
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(bundle) = pack_fixture(dir.path()) else {
        eprintln!("skipping: phase2 smoke fixture not built");
        return;
    };
    let dmg = dir.path().join("gift.dmg");
    let out = krate()
        .args(["wrap", "--for", "mac"])
        .arg(&bundle)
        .arg("-o")
        .arg(&dmg)
        .env("KRATE_GIFT_ANY_PLAYER", "1")
        .output()
        .expect("run krate wrap");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(dmg.is_file(), "one file, not a folder");
    let mnt = dir.path().join("mnt");
    std::fs::create_dir_all(&mnt).unwrap();
    let attached = std::process::Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-readonly", "-quiet", "-mountpoint"])
        .arg(&mnt)
        .arg(&dmg)
        .status()
        .expect("hdiutil attach");
    assert!(attached.success());
    let names: Vec<String> = std::fs::read_dir(&mnt)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().to_string()))
        .collect();
    let player = std::fs::read(mnt.join(".player/krate"));
    let _ = std::process::Command::new("hdiutil")
        .args(["detach", "-quiet"])
        .arg(&mnt)
        .status();
    assert!(
        names.iter().any(|n| n.ends_with(".app")),
        "the opener: {names:?}"
    );
    assert!(
        names.iter().any(|n| n.ends_with(".krate")),
        "the app: {names:?}"
    );
    let engine = std::fs::read(env!("CARGO_BIN_EXE_krate")).expect("this krate");
    assert!(
        player.expect("the player rides inside") == engine,
        "the player is this engine, byte for byte"
    );
}

/// The Mac opener runs the NEWEST Krate on the machine, and a run that ends
/// badly says why in a window. A Mac held a months-old `krate` on its PATH
/// beside a current Krate; the opener took the first one found, which could
/// not run an app made that day, and with `exec` and no console the app just
/// vanished after the permission window (2026-10-07).
///
/// Driven with fake engines (each records that it ran) and osascript stubbed
/// to record what the person would be shown.
#[cfg(unix)]
#[test]
fn the_mac_opener_runs_the_newest_krate_and_never_fails_silently() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tempdir");
    let script = krate().arg("gift-opener").output().expect("gift-opener");
    let gift = dir.path().join("gift");
    let macos = gift.join("Open Demo.app/Contents/MacOS");
    std::fs::create_dir_all(&macos).unwrap();
    std::fs::write(macos.join("open"), &script.stdout).unwrap();
    std::fs::write(gift.join("Demo.krate"), b"app").unwrap();
    let exe = |path: &std::path::Path, body: &str| {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    };
    let log = dir.path().join("ran.log");
    let dialogs = dir.path().join("dialogs.log");
    let stub = dir.path().join("stub");
    exe(
        &stub.join("osascript"),
        &format!("#!/bin/sh\nargs=\"$*\"; cat >/dev/null; echo \"$args\" >> '{}'\necho 'button returned:Not now'\n", dialogs.display()),
    );
    let engine = |version: &str, run: &str| {
        format!("#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'krate v{version}'; exit 0; fi\necho '{version}' >> '{}'\n{run}\n", log.display())
    };
    let open = |home: &std::path::Path| {
        let _ = std::fs::remove_file(&log);
        let _ = std::fs::remove_file(&dialogs);
        std::process::Command::new("sh")
            .arg(macos.join("open"))
            .env_clear()
            .env("HOME", home)
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", stub.display()),
            )
            .env("TMPDIR", dir.path())
            .output()
            .expect("run the opener")
    };

    // An old krate first on the search path, a newer player after it.
    let home = dir.path().join("home");
    exe(&home.join(".local/bin/krate"), &engine("0.1.58", "exit 1"));
    exe(
        &home.join("Applications/Krate Player.app/Contents/MacOS/krate-cli"),
        &engine("9.9.9", "exit 0"),
    );
    open(&home);
    let ran = std::fs::read_to_string(&log).unwrap_or_default();
    assert_eq!(
        ran.trim(),
        "9.9.9",
        "the newest Krate runs the app, not the first found"
    );
    assert!(!dialogs.exists(), "a good run shows nothing");

    // The newest one fails: the person is told why, in a window.
    let home2 = dir.path().join("home2");
    exe(
        &home2.join("Applications/Krate Player.app/Contents/MacOS/krate-cli"),
        &engine(
            "9.9.9",
            "echo 'error: built against different versions of the app interface' >&2\nexit 1",
        ),
    );
    open(&home2);
    let said = std::fs::read_to_string(&dialogs).unwrap_or_default();
    assert!(
        said.contains("Demo could not start"),
        "the failure is said: {said}"
    );
    assert!(
        said.contains("different versions of the app interface"),
        "with the engine's reason: {said}"
    );
    assert!(!said.contains("curl"), "and no terminal command: {said}");
}

/// The build service makes Mac gifts on Linux, from the release's notarized
/// opener and its two signed Mac engines: one .zip, one folder inside, the
/// opener runnable, one player per architecture named for it -- and a file
/// that is not a Mac program is refused rather than handed to a Mac.
#[test]
fn a_mac_gift_from_named_parts_is_one_zip_with_a_player_per_architecture() {
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(bundle) = pack_fixture(dir.path()) else {
        eprintln!("skipping: phase2 smoke fixture not built");
        return;
    };
    let opener = dir.path().join("Krate Opener.app");
    std::fs::create_dir_all(opener.join("Contents/MacOS")).unwrap();
    std::fs::write(opener.join("Contents/MacOS/open"), "#!/bin/sh\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            opener.join("Contents/MacOS/open"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    let macho = |cpu: u32| {
        let mut b = vec![0xCF, 0xFA, 0xED, 0xFE];
        b.extend_from_slice(&cpu.to_le_bytes());
        b.extend_from_slice(&[0u8; 64]);
        b
    };
    let arm = dir.path().join("arm64-krate");
    let intel = dir.path().join("x86-krate");
    std::fs::write(&arm, macho(0x0100_000C)).unwrap();
    std::fs::write(&intel, macho(0x0100_0007)).unwrap();
    let zip_path = dir.path().join("gift.zip");
    let out = krate()
        .args(["wrap", "--for", "mac"])
        .arg(&bundle)
        .arg("-o")
        .arg(&zip_path)
        .arg("--opener")
        .arg(&opener)
        .arg("--player")
        .arg(&arm)
        .arg("--player")
        .arg(&intel)
        .output()
        .expect("run krate wrap");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mut zip = zip::ZipArchive::new(std::fs::File::open(&zip_path).unwrap()).expect("a zip");
    let names: Vec<String> = (0..zip.len())
        .map(|i| zip.by_index(i).unwrap().name().to_string())
        .collect();
    let top: std::collections::BTreeSet<&str> =
        names.iter().map(|n| n.split('/').next().unwrap()).collect();
    assert_eq!(top.len(), 1, "one folder inside: {top:?}");
    let has = |suffix: &str| names.iter().find(|n| n.ends_with(suffix)).cloned();
    let open = has(".app/Contents/MacOS/open").expect("the opener");
    assert!(
        open.contains("/Open "),
        "the opener is named for the app: {open}"
    );
    let mode = zip.by_name(&open).unwrap().unix_mode().unwrap_or(0);
    assert!(mode & 0o111 != 0, "the opener stays runnable");
    for player in ["/.player/krate-arm64", "/.player/krate-x86_64"] {
        let name = has(player).unwrap_or_else(|| panic!("{player} in {names:?}"));
        assert!(
            zip.by_name(&name).unwrap().unix_mode().unwrap_or(0) & 0o111 != 0,
            "{player} runnable"
        );
    }
    assert!(has(".krate").is_some(), "the app");

    // A Linux engine handed to a Mac gift: refused.
    let elf = dir.path().join("linux-krate");
    std::fs::write(&elf, b"\x7fELF\x02\x01\x01\x00padding").unwrap();
    let refused = krate()
        .args(["wrap", "--for", "mac"])
        .arg(&bundle)
        .arg("-o")
        .arg(dir.path().join("bad.zip"))
        .arg("--opener")
        .arg(&opener)
        .arg("--player")
        .arg(&elf)
        .output()
        .unwrap();
    assert!(
        !refused.status.success(),
        "a Linux program is not a Mac player"
    );
    assert!(String::from_utf8_lossy(&refused.stderr).contains("not a Mac program"));
}

/// The Linux gift carries the player too: run on a machine with no Krate,
/// it installs the player into the home folder and opens the app; run again
/// it just opens; a damaged copy installs nothing. And it is still a bundle
/// to anyone who already has Krate.
#[cfg(unix)]
#[test]
fn a_linux_gift_installs_the_player_it_carries_then_opens_the_app() {
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(bundle) = pack_fixture(dir.path()) else {
        eprintln!("skipping: phase2 smoke fixture not built");
        return;
    };
    let gift = dir.path().join("gift.sh");
    let out = krate()
        .args(["wrap", "--for", "linux"])
        .arg(&bundle)
        .arg("-o")
        .arg(&gift)
        .env("KRATE_GIFT_ANY_PLAYER", "1")
        .output()
        .expect("run krate wrap");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let read = krate()
        .arg("run")
        .arg(&gift)
        .arg("--dump-caps")
        .output()
        .unwrap();
    assert!(
        read.status.success(),
        "krate still reads the app out of the gift: {}",
        String::from_utf8_lossy(&read.stderr)
    );

    // The test opens the app with --dump-caps instead of the consent window,
    // keeping every byte offset the same length.
    let mut bytes = std::fs::read(&gift).unwrap();
    let swap = |bytes: &mut Vec<u8>, from: &[u8], to: &[u8]| {
        let at = bytes
            .windows(from.len())
            .position(|w| w == from)
            .expect("marker in the script");
        bytes.splice(at..at + from.len(), to.iter().copied());
    };
    swap(&mut bytes, b"\" --consent\n", b"\" --dump-caps\n");
    swap(&mut bytes, b" a script ----\n", b" a script --\n");
    let test_gift = dir.path().join("test-gift.sh");
    std::fs::write(&test_gift, &bytes).unwrap();

    let home = dir.path().join("home");
    let tmp = dir.path().join("tmp");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&tmp).unwrap();
    let open = |script: &std::path::Path, home: &std::path::Path| {
        std::process::Command::new("sh")
            .arg(script)
            .env_clear()
            .env("HOME", home)
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("TMPDIR", &tmp)
            .env("KRATE_PLAYER_NO_REGISTER", "1")
            .stdin(std::process::Stdio::null())
            .output()
            .expect("run the gift")
    };
    let first = open(&test_gift, &home);
    let said = String::from_utf8_lossy(&first.stdout).to_string()
        + &String::from_utf8_lossy(&first.stderr);
    assert!(first.status.success(), "the first open: {said}");
    assert!(said.contains("ui.window"), "the app opened: {said}");
    let installed = if cfg!(target_os = "macos") {
        home.join("Applications/Krate Player.app/Contents/MacOS/krate-cli")
    } else {
        home.join(".local/share/krate/bin/krate")
    };
    assert!(
        installed.is_file(),
        "the player was installed at {}",
        installed.display()
    );
    assert_eq!(
        std::fs::read_dir(&tmp).unwrap().count(),
        0,
        "nothing left in the temp folder"
    );
    let second = open(&test_gift, &home);
    assert!(second.status.success(), "the second open");

    // One flipped byte inside the player: refused, nothing installed.
    let start: usize = String::from_utf8_lossy(&bytes[..4096])
        .lines()
        .find_map(|l| {
            l.strip_prefix("player_start=")
                .map(|d| d.trim().parse().unwrap())
        })
        .expect("player_start");
    let mut bad = bytes.clone();
    bad[start + 4000] ^= 0xff;
    let bad_gift = dir.path().join("bad.sh");
    std::fs::write(&bad_gift, &bad).unwrap();
    let fresh = dir.path().join("fresh-home");
    std::fs::create_dir_all(&fresh).unwrap();
    let refused = open(&bad_gift, &fresh);
    assert!(!refused.status.success(), "a damaged gift must not open");
    assert_eq!(
        std::fs::read_dir(&fresh).unwrap().count(),
        0,
        "and must install nothing"
    );

    // An intact player that is not the one the header names: only the
    // digest can catch this (gzip's own check passes), so it bites the
    // digest check itself.
    let mut wrong = bytes.clone();
    let key = b"player_sha256=";
    let at = wrong
        .windows(key.len())
        .position(|w| w == key)
        .expect("digest line")
        + key.len();
    wrong[at] = if wrong[at] == b'0' { b'1' } else { b'0' };
    let wrong_gift = dir.path().join("wrong.sh");
    std::fs::write(&wrong_gift, &wrong).unwrap();
    let other = dir.path().join("other-home");
    std::fs::create_dir_all(&other).unwrap();
    let refused = open(&wrong_gift, &other);
    assert!(
        !refused.status.success(),
        "a player that does not match its digest must not install"
    );
    assert_eq!(
        std::fs::read_dir(&other).unwrap().count(),
        0,
        "and must install nothing"
    );
}

#[test]
fn pack_writes_a_single_bundle_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(bundle) = pack_fixture(dir.path()) else {
        eprintln!("skipping: phase2 smoke fixture not built");
        return;
    };
    let size = std::fs::metadata(&bundle).expect("bundle metadata").len();
    assert!(size > 0, "bundle should not be empty");
}

#[test]
fn a_bundle_grants_exactly_what_the_sidecar_manifest_grants() {
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(bundle) = pack_fixture(dir.path()) else {
        eprintln!("skipping: phase2 smoke fixture not built");
        return;
    };

    let from_sidecar = krate()
        .arg("run")
        .arg(dir.path().join("code.wasm"))
        .arg("--manifest")
        .arg(dir.path().join("manifest.toml"))
        .arg("--dump-caps")
        .output()
        .expect("run with sidecar manifest");
    let from_bundle = krate()
        .arg("run")
        .arg(&bundle)
        .arg("--dump-caps")
        .output()
        .expect("run bundle");

    // Compare the capability lists rather than the whole screen: a packaged
    // bundle also reports its content identity, which a loose .wasm has no way
    // to have. What must not change is the authority the app ends up with.
    let capabilities = |output: &[u8]| -> Vec<String> {
        String::from_utf8_lossy(output)
            .lines()
            .skip_while(|line| !line.starts_with("Effective capabilities"))
            .map(str::to_string)
            .collect()
    };
    assert_eq!(
        capabilities(&from_sidecar.stdout),
        capabilities(&from_bundle.stdout),
        "packaging must not change the effective capability set"
    );
}

#[test]
fn a_bundle_refuses_an_external_manifest() {
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(bundle) = pack_fixture(dir.path()) else {
        eprintln!("skipping: phase2 smoke fixture not built");
        return;
    };

    // Otherwise a caller could hand a bundle a wider manifest than the one its
    // author shipped, which would defeat the point of packaging them together.
    let output = krate()
        .arg("run")
        .arg(&bundle)
        .arg("--manifest")
        .arg(dir.path().join("manifest.toml"))
        .output()
        .expect("run bundle with external manifest");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("carries its own manifest"));
}

#[test]
fn fetching_a_bundle_over_plain_http_is_refused_by_default() {
    let output = krate()
        .arg("run")
        .arg("http://127.0.0.1:1/app.krate")
        .output()
        .expect("run http url");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("refusing to fetch over plain HTTP"),
        "expected an https refusal, got: {stderr}"
    );
}

#[test]
fn a_denial_tells_you_how_to_grant_and_names_what_you_ran() {
    let dir = tempfile::tempdir().expect("tempdir");
    let Some(bundle) = pack_fixture(dir.path()) else {
        eprintln!("skipping: phase2 smoke fixture not built");
        return;
    };

    // Non-interactive runs cannot prompt, so the denial has to carry the way
    // out with it, echoing the target the user actually typed.
    let output = krate()
        .arg("run")
        .arg(&bundle)
        .output()
        .expect("run bundle without grants");
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("needs permission it was not given") {
        assert!(
            stderr.contains("--grant"),
            "should suggest --grant: {stderr}"
        );
        assert!(
            stderr.contains(bundle.to_str().expect("bundle path is utf8")),
            "should echo the target that was run: {stderr}"
        );
    }
}

#[test]
fn a_failure_report_is_shown_in_full_and_never_sent_on_its_own() {
    // The privacy guarantee, as a test. A failure report can hold someone's
    // source and paths, so `krate report` must show the whole file and stop.
    // If this ever starts uploading, this test is what should fail first.
    let dir = tempfile::tempdir().expect("tempdir");
    let report = dir.path().join("FAILURE-REPORT.md");
    let secret_line = "let api_key = \"sk-do-not-transmit\";";
    std::fs::write(
        &report,
        format!(
            "# Krate port failure report\n\n\
             - What kind: unknown API\n\n\
             ## The full error\n\n```\n{secret_line}\n```\n"
        ),
    )
    .expect("write report");

    let output = krate()
        .arg("report")
        .arg(&report)
        .output()
        .expect("run report");
    assert!(output.status.success(), "report command should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);

    // The whole file, so a person can find anything they would rather not send.
    assert!(
        stdout.contains(secret_line),
        "the report must be shown in full before anything is offered: {stdout}"
    );
    // And it must say plainly that nothing has left the machine.
    assert!(
        stdout.contains("only on your computer") || stdout.contains("has not been sent"),
        "the report must say it was not sent: {stdout}"
    );
    // Sending is the person's own action, in their own browser.
    assert!(
        stdout.contains("will not upload it for you")
            || stdout.contains("will not upload this report for you"),
        "the report must not imply Krate transmits it: {stdout}"
    );
}

#[test]
fn a_failed_port_says_what_kind_of_failure_it_was() {
    // A port that fails should leave the person knowing whether this is our
    // gap or their code, and roughly how long. The classification is what makes
    // an honest promise possible, so it has to reach the terminal.
    //
    // Unix only. The test drives the port through `--author-cmd`, which hands
    // the command to a shell, and a Windows temp path is full of backslashes
    // that bash reads as escapes: `C:\Users\RUNNER~1\...` arrives as
    // `C:UsersRUNNER~1...` and the script is never found. That is this
    // fixture's problem, not the pipeline's -- the real port path resolves its
    // shell through `author_shell()` and is covered on Windows elsewhere. The
    // other --author-cmd tests pass an inline command rather than a script
    // path, which is why they run everywhere; this one needs a file because it
    // has to append to the candidate.
    if cfg!(windows) {
        eprintln!("skipping the script-author check on Windows: it needs a shell script the agent seam can append to");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let source = dir.path().join("src-project");
    std::fs::create_dir_all(source.join("src")).expect("mkdir");
    std::fs::write(
        source.join("Cargo.toml"),
        "[package]\nname = \"reportcase\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("write Cargo.toml");
    std::fs::write(
        source.join("src/main.rs"),
        "fn main() { let d = std::fs::read(\"in.bin\").unwrap(); println!(\"{}\", d.len()); }\n",
    )
    .expect("write main.rs");

    // An agent that invents a function, which is the failure this classifies.
    let agent = dir.path().join("agent.sh");
    std::fs::write(
        &agent,
        "#!/bin/sh\n\
         f=\"$KRATE_PORT_CANDIDATE/src/lib.rs\"\n\
         grep -v 'starting point' \"$f\" > \"$f.edited\" && mv \"$f.edited\" \"$f\"\n\
         printf 'fn never_compiles() { stdio::write_hex(b\"x\"); }\\n' >> \"$f\"\n",
    )
    .expect("write agent");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o755))
            .expect("chmod agent");
    }

    let output = krate()
        .arg("port")
        .arg(&source)
        .arg("--prepare")
        .arg(dir.path().join("ws"))
        .arg("--author-cmd")
        .arg(&agent)
        .arg("--to")
        .arg(dir.path().join("out.krate"))
        .output()
        .expect("run port");

    assert!(!output.status.success(), "this port is meant to fail");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        stderr.contains("This port failed:"),
        "a failed port must say what kind of failure it was: {stderr}"
    );
    assert!(
        stderr.contains("stdio::write_hex"),
        "it must name the API that does not exist: {stderr}"
    );
    // The report exists locally and the person is told where, not that it went
    // anywhere.
    assert!(
        stderr.contains("has not been sent anywhere"),
        "it must say the report stayed local: {stderr}"
    );
}

/// `--agent` must drive THIS binary, never whatever `krate` PATH happens to
/// resolve to.
///
/// The regression this guards: `--agent claude` used to expand to the bare
/// string `krate author-agent claude`. Running a freshly built krate would
/// then hand authoring to an older installed krate -- a different prompt, a
/// different check-app, a different everything -- while appearing to work.
/// The same class of trap as double-clicking a stale installed app.
#[test]
fn the_agent_seam_invokes_this_binary_not_one_from_path() {
    if cfg!(windows) {
        // Three independent reasons the decoy can never run there: it is
        // written as `krate` not `krate.exe`, the chmod is cfg(unix), and
        // PATH is joined with `:`. So on Windows this test passed without
        // testing anything -- the exact K-269 shape, which the skip-count
        // report exists to surface. Said out loud instead of silent.
        eprintln!("skipping the decoy-on-PATH check on Windows: the decoy is a Unix script and PATH is colon-joined");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    // A decoy `krate` earlier on PATH. If the seam resolves by name, this is
    // what would run, and it fails loudly.
    let decoy_dir = dir.path().join("decoy");
    std::fs::create_dir_all(&decoy_dir).expect("decoy dir");
    let decoy = decoy_dir.join("krate");
    std::fs::write(
        &decoy,
        "#!/bin/sh\necho 'DECOY KRATE FROM PATH RAN' >&2\nexit 42\n",
    )
    .expect("write decoy");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&decoy, std::fs::Permissions::from_mode(0o755))
            .expect("chmod decoy");
    }

    let path = format!(
        "{}:{}",
        decoy_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    // `author-agent` with no agent environment fails fast; what matters is
    // WHICH binary produced the failure, not that it succeeded.
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_krate"))
        .args(["create", "a test app", "--output"])
        .arg(dir.path().join("out.krate"))
        .args(["--agent", "claude"])
        .env("PATH", &path)
        .env("KRATE_AUTHOR_TIMEOUT_SECS", "1")
        .output()
        .expect("run create");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("DECOY KRATE FROM PATH RAN"),
        "--agent resolved `krate` through PATH instead of using the running \
         binary, so an older install would silently drive authoring: {stderr}"
    );
}

/// `krate create` with no agent must produce a working app.
///
/// The regression this guards: the GUI template wrote `extern crate krate`
/// into src/lib.rs but never wrote the matching `krate` dependency into
/// Cargo.toml, so EVERY app made this way failed to build. It survived because
/// the only create test drove the agent seam, which uses a different
/// skeleton -- the plain template path, which is what the website tells people
/// to run, had no test at all.
///
/// Covers both templates: a list-shaped request routes to the GUI checklist,
/// a file-reading request routes to the CLI reporter.
#[test]
fn create_without_an_agent_builds_a_real_app_from_a_template() {
    if !has_cargo_component() {
        eprintln!("skipping: cargo-component not installed");
        return;
    }
    let _build_lock = cargo_build_guard();

    for (request, label) in [
        ("a grocery list app", "gui template"),
        ("read a file and count the words", "cli template"),
    ] {
        let work = tempfile::tempdir().expect("temp dir");
        let out = work.path().join("made.krate");
        let inspect = work.path().join("inspect");
        let output = krate()
            .arg("create")
            .arg(request)
            .arg("--output")
            .arg(&out)
            .arg("--work-dir")
            .arg(&inspect)
            .output()
            .expect("run create");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "`krate create \"{request}\"` ({label}) must build without an agent. \
             This is the command the website tells people to run.\n{stderr}"
        );
        assert!(out.is_file(), "{label}: the .krate was written");

        // The dependency whose absence caused the original failure.
        let app_dir = std::fs::read_dir(&inspect)
            .expect("work dir")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| path.join("Cargo.toml").is_file())
            .expect("an app directory");
        let cargo = std::fs::read_to_string(app_dir.join("Cargo.toml")).expect("Cargo.toml");
        assert!(
            cargo
                .lines()
                .map(str::trim_start)
                .any(|line| line.starts_with("krate ") || line.starts_with("krate=")),
            "{label}: Cargo.toml must declare the `krate` dependency; the SDK owns \
             the guest's allocator and panic handler:\n{cargo}"
        );
    }
}

/// An unrecognized `--agent` must list the providers that do exist.
///
/// The failure this prevents is a dead end: someone tries the AI they actually
/// have, gets told the value is invalid, and has no way to discover what Krate
/// supports without reading the source.
#[test]
fn an_unknown_agent_name_lists_the_supported_providers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_krate"))
        .args(["create", "a test app", "--output"])
        .arg(dir.path().join("out.krate"))
        .args(["--agent", "definitely-not-an-ai"])
        .output()
        .expect("run create");

    assert!(
        !output.status.success(),
        "an unknown agent must not succeed"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("definitely-not-an-ai"),
        "it must quote the name that was given: {stderr}"
    );
    assert!(
        stderr.contains("Available providers:") && stderr.contains("claude"),
        "it must list the providers that work: {stderr}"
    );
    assert!(
        stderr.contains("--author-cmd"),
        "it must point at the escape hatch for other tools: {stderr}"
    );
}

/// `account login --json` must be accepted, in both flag positions.
///
/// Krate Studio sends `account login --json`. The flag was defined only on the
/// parent `account` command, so the engine answered "unexpected argument
/// '--json' found" and exited -- signing in was impossible in every shipped
/// build, and the studio reported it as a BUILD failure because the gate
/// reused that wording. Nobody could get past the first screen.
///
/// Checked with --help rather than by signing in, so the test needs no network
/// and no GitHub.
#[test]
fn account_login_accepts_json_in_either_position() {
    for args in [
        ["account", "login", "--json", "--help"],
        ["account", "--json", "login", "--help"],
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_krate"))
            .args(args)
            .output()
            .expect("run account login");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output.status.success() && !text.contains("unexpected argument"),
            "krate {args:?} must be accepted, got: {text}"
        );
    }
}

/// A supported provider whose CLI is not installed must say so, and say how to
/// install it -- never fail with a raw spawn error naming a program the person
/// never typed.
#[test]
fn a_supported_agent_with_no_cli_installed_explains_how_to_install_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    // An empty PATH guarantees `claude` cannot be found, whatever this machine
    // happens to have installed.
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_krate"))
        .args(["create", "a test app", "--output"])
        .arg(dir.path().join("out.krate"))
        .args(["--agent", "claude"])
        .env("PATH", "")
        .output()
        .expect("run create");

    assert!(!output.status.success(), "a missing CLI must not succeed");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("`claude` command is not installed"),
        "it must name the missing command in plain words: {stderr}"
    );
    assert!(
        stderr.contains("claude.com/claude-code"),
        "it must say where to get it: {stderr}"
    );
    assert!(
        !stderr.contains("No such file or directory") && !stderr.contains("os error 2"),
        "it must not surface a raw spawn error: {stderr}"
    );
}

/// `krate ai` must tell someone what they can author with.
///
/// This is the "connect your AI" step. It is a lookup rather than a login: the
/// AI tools each own their own sign-in, and Krate holding a copy of anyone's
/// credentials would be strictly worse. The command must therefore work
/// offline, touch no credential, and still be useful when nothing is
/// installed.
#[test]
fn ai_lists_what_this_machine_can_author_with() {
    let output = krate().arg("ai").output().expect("run krate ai");
    assert!(output.status.success(), "krate ai must not fail");
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Every provider is accounted for, either as ready or as installable.
    for provider in ["claude", "codex", "gemini", "copilot", "grok"] {
        assert!(
            stdout.contains(provider),
            "krate ai must mention {provider}: {stdout}"
        );
    }
    // It must end with something the person can actually do next -- which is
    // "krate create" when a tool is ready, and a way to fix one when none
    // is. Asserting only the first made this test depend on the machine
    // running it being signed in to an AI, so it began failing the moment
    // the readiness probe started telling the truth about the confined home
    // (K-190). What matters is that the person is never left at a dead end.
    assert!(
        stdout.contains("krate create") || stdout.contains("run `krate ai` again"),
        "krate ai must show a next step, either a command to run or a fix: {stdout}"
    );
}

/// `krate connect` must set up an AI app without anyone editing JSON, and must
/// be careful with a file it did not write.
///
/// The reason this command exists: the alternative was a page telling a
/// moderately technical person to hand-edit JSON in a file whose path differs
/// per operating system, after first explaining what MCP is. That is a wall in
/// front of the product for exactly the people we most want.
///
/// The three things it must never get wrong are all checked here: it must keep
/// servers somebody else configured, it must not overwrite a file it cannot
/// parse, and running it twice must be safe.
#[test]
fn connect_sets_up_an_ai_app_without_touching_anything_else() {
    let home = tempfile::tempdir().expect("tempdir");
    let config = home.path().join(".cursor/mcp.json");
    std::fs::create_dir_all(config.parent().expect("parent")).expect("mkdir");
    std::fs::write(
        &config,
        r#"{"mcpServers":{"github":{"command":"npx","args":["-y","server-github"]}}}"#,
    )
    .expect("seed config");

    let output = krate()
        .args(["connect", "cursor", "--yes"])
        .env("HOME", home.path())
        .output()
        .expect("run connect");
    assert!(
        output.status.success(),
        "connect failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config).expect("read")).expect("valid json");
    assert!(
        written.pointer("/mcpServers/github").is_some(),
        "connect deleted a server somebody else configured: {written}"
    );
    assert_eq!(
        written.pointer("/mcpServers/krate/args"),
        Some(&serde_json::json!(["mcp"])),
        "krate must be wired to `krate mcp`: {written}"
    );
    // The command must be a real path, not a bare name: a client launches the
    // server with its own environment, where PATH may not have our install dir,
    // and a bare name would find an older installed Krate anyway.
    let command = written
        .pointer("/mcpServers/krate/command")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    assert!(
        command.contains('/') || command.contains('\\'),
        "the command must be an absolute path, got {command:?}"
    );

    // Running it again is safe and says so rather than writing again.
    let again = krate()
        .args(["connect", "cursor"])
        .env("HOME", home.path())
        .output()
        .expect("run connect again");
    assert!(again.status.success());
    assert!(
        String::from_utf8_lossy(&again.stdout).contains("already set up"),
        "a second run should report it is already done"
    );
}

/// A config file that is not valid JSON must be left exactly as it was.
#[test]
fn connect_refuses_to_overwrite_a_file_it_cannot_read() {
    let home = tempfile::tempdir().expect("tempdir");
    let config = home.path().join(".cursor/mcp.json");
    std::fs::create_dir_all(config.parent().expect("parent")).expect("mkdir");
    let original = "{ this is not json";
    std::fs::write(&config, original).expect("seed config");

    let output = krate()
        .args(["connect", "cursor", "--yes"])
        .env("HOME", home.path())
        .output()
        .expect("run connect");
    assert!(!output.status.success(), "it must refuse, not guess");
    assert_eq!(
        std::fs::read_to_string(&config).expect("read"),
        original,
        "the file must be untouched"
    );
}

/// A person's sign-off is bound to the exact build, and it cannot stand in for
/// a check that failed.
///
/// The refusals are the point. A record that says "approved" over a functional
/// or security failure is worse than no record at all, because the next person
/// reads it and believes it. This drives the real binary, because the guard
/// that matters is the one a person actually hits.
#[test]
fn a_sign_off_names_the_build_and_refuses_to_cover_a_real_failure() {
    let work = tempfile::tempdir().expect("temp dir");
    let bundle = work.path().join("app.krate");
    std::fs::write(&bundle, b"pretend this is a built app").expect("seed");

    // A complete verdict is recorded, and it names the bytes it is about.
    let out = krate()
        .arg("accept")
        .arg(&bundle)
        .arg("--reviewer")
        .arg("yashraj")
        .arg("--role")
        .arg("the person who asked")
        .arg("--notes")
        .arg("opened it, added three items, reopened, still there")
        .arg("--json")
        .output()
        .expect("run accept");
    assert!(
        out.status.success(),
        "a complete verdict should record: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let record: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("the record is one JSON object");
    assert_eq!(record["schema"], "krate.accept.v1");
    assert_eq!(record["decision"], "accepted");
    assert_eq!(record["reviewer_role"], "the person who asked");
    let digest = record["artifact_digest"]
        .as_str()
        .expect("a digest")
        .to_string();
    assert!(digest.starts_with("sha256:"), "digest was {digest}");
    assert!(
        !record["at"].as_str().unwrap_or_default().is_empty(),
        "a verdict with no date cannot be read later"
    );

    // Change one byte and the digest changes, so the old verdict cannot be
    // read as covering the new build. This is the whole reason it is recorded.
    std::fs::write(&bundle, b"pretend this is a REBUILT app").expect("rebuild");
    let after = krate()
        .arg("accept")
        .arg(&bundle)
        .arg("--reviewer")
        .arg("yashraj")
        .arg("--json")
        .output()
        .expect("run accept");
    let second: serde_json::Value = serde_json::from_slice(&after.stdout).expect("json");
    assert_ne!(
        second["artifact_digest"], digest,
        "a rebuilt app must not carry the previous build's approval"
    );

    // A waiver with no reason is refused: it cannot be told apart from nobody
    // having looked.
    let no_reason = krate()
        .arg("accept")
        .arg(&bundle)
        .arg("--reviewer")
        .arg("yashraj")
        .arg("--waive-preference")
        .arg("req-3:")
        .output()
        .expect("run accept");
    assert!(
        !no_reason.status.success(),
        "a blank waiver must be refused"
    );
    assert!(
        String::from_utf8_lossy(&no_reason.stderr).contains("needs a reason"),
        "the refusal must say what is missing: {}",
        String::from_utf8_lossy(&no_reason.stderr)
    );

    // A verdict that cannot say who decided it is refused too.
    let nameless = krate()
        .arg("accept")
        .arg(&bundle)
        .arg("--reviewer")
        .arg("")
        .output()
        .expect("run accept");
    assert!(!nameless.status.success(), "an unsigned verdict is not one");

    // And the only waiver the CLI offers is for a preference. There is no
    // surface here for waiving a functional, security or portability failure,
    // which is the rule this whole command exists to hold.
    let help = krate().arg("accept").arg("--help").output().expect("help");
    let text = String::from_utf8_lossy(&help.stdout);
    assert!(text.contains("--waive-preference"), "{text}");
    assert!(
        !text.contains("--waive-security") && !text.contains("--waive-functional"),
        "there must be no flag that waives a real failure: {text}"
    );
}

/// Test 1691, the E5 case: an invoice application ported into something else
/// must not pass, however clean the build was.
///
/// The original reads quantities from a file, multiplies their sum by a unit
/// price of 10, and prints `total=100`. The candidate here is a real, working,
/// compiling Krate app -- and it counts lines. It builds, imports only
/// `krate:*`, runs, and refuses without its capability. Four mechanical checks,
/// all green, on an app that is not the one that was ported.
///
/// That combination is what E5 caught being reported as a finished port, while
/// the same command's own journey record said `primary-task: not-verified`. A
/// permission-denial pass is not a primary-task pass (test 1697), so the
/// pipeline now reads its own record before deciding.
#[test]
fn a_port_that_does_something_else_is_not_a_finished_port() {
    if !has_cargo_component() {
        eprintln!("skipping: cargo-component is not installed");
        return;
    }
    let _build_lock = cargo_build_guard();
    let root = tempfile::tempdir().expect("temp dir");

    // The E5 invoice fixture, exactly: 2 + 3 + 5, times 10, is 100.
    let source = root.path().join("invoice");
    std::fs::create_dir_all(source.join("src")).expect("create source");
    std::fs::write(
        source.join("Cargo.toml"),
        "[package]\nname = \"invoice\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("write Cargo.toml");
    std::fs::write(
        source.join("src/main.rs"),
        r#"use std::{env, fs};
fn main() {
    let path = env::args().nth(1).unwrap_or_else(|| "qty.txt".to_string());
    let text = fs::read_to_string(&path).expect("read quantities");
    let sum: i64 = text.split_whitespace().filter_map(|w| w.parse::<i64>().ok()).sum();
    println!("total={}", sum * 10);
}
"#,
    )
    .expect("write source");
    std::fs::write(source.join("qty.txt"), "2\n3\n5\n").expect("write fixture input");

    let bundle = root.path().join("ported.krate");
    let workspace = root.path().join("port-work");

    // An author command that replaces the body with a different working app.
    // Deliberately not a no-op and not a comment-only edit: those are already
    // refused, and refusing them is not the same as checking behaviour.
    let author = "printf '//! A line counter.\\n#![no_std]\\nextern crate alloc;\\n\
                  use krate::{io::stdio, Guest};\\nstruct Component;\\n\
                  impl Guest for Component {\\n    fn run() -> i32 {\\n        \
                  let _ = stdio::print(\"lines=3\\\\n\");\\n        0\\n    }\\n}\\n\
                  krate::export!(Component);\\n' > \"$KRATE_PORT_CANDIDATE/src/lib.rs\"";

    let output = krate()
        .arg("port")
        .arg(&source)
        .arg("--prepare")
        .arg(&workspace)
        .arg("--author-cmd")
        .arg(author)
        .arg("--to")
        .arg(&bundle)
        .arg("--no-install")
        .env("CARGO_NET_OFFLINE", "true")
        .output()
        .expect("run port pipeline");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        output.status.code(),
        Some(7),
        "a line counter must not pass as a port of an invoice app\nstdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    // And the refusal has to be about behaviour, not a build failure -- the
    // whole point is that everything mechanical passed.
    assert!(
        stdout.contains("primary-task"),
        "the unfinished journey must be named: {stdout}"
    );
    assert!(
        bundle.is_file(),
        "the bundle is still written: it built and runs, it is just not proven \
         to be the same app"
    );

    // The record and the verdict must agree. Before this, the record said
    // not-verified and the verdict said success in the same breath.
    let results: serde_json::Value = serde_json::from_slice(
        &std::fs::read(workspace.join("journey-results.json")).expect("journey results"),
    )
    .expect("parse");
    let primary = results["results"]
        .as_array()
        .expect("results")
        .iter()
        .find(|r| r["id"] == "primary-task")
        .expect("a primary-task journey");
    assert_eq!(
        primary["status"], "not-verified",
        "the record must still say what it always said"
    );

    // Permission denial passing is exactly what must NOT stand in for it.
    let denial = results["results"]
        .as_array()
        .expect("results")
        .iter()
        .find(|r| r["id"] == "permission-denial");
    if let Some(denial) = denial {
        assert_eq!(denial["status"], "passed");
    }
}

/// IC-004: an agent must not be able to edit the file that judges its work.
///
/// A model under pressure to succeed will reach for the test. The port agent
/// works in `candidate/` and has no business touching the criteria beside it,
/// and until this guard nothing stopped it -- an author command could rewrite
/// `journeys.json` and the port carried on without a word.
///
/// The five shapes the register names are all here: delete, invert, weaken,
/// replace, and editing the task itself. Each one must stop the port before
/// packaging, and an honest agent must be unaffected.
#[test]
fn an_agent_cannot_edit_the_criteria_that_judge_it() {
    if !has_cargo_component() {
        eprintln!("skipping: cargo-component is not installed");
        return;
    }
    let _build_lock = cargo_build_guard();
    let root = tempfile::tempdir().expect("temp dir");
    let source = root.path().join("tiny");
    std::fs::create_dir_all(source.join("src")).expect("create source");
    std::fs::write(
        source.join("Cargo.toml"),
        "[package]\nname = \"tiny\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("write Cargo.toml");
    std::fs::write(
        source.join("src/main.rs"),
        "fn main() { println!(\"tiny\"); }\n",
    )
    .expect("write source");

    // A body replacement that builds, so nothing else in the pipeline objects
    // and the only thing under test is the criteria guard.
    let write_app = "printf '//! ported\\n#![no_std]\\nextern crate alloc;\\n\
                     use krate::{io::stdio, Guest};\\nstruct Component;\\n\
                     impl Guest for Component {\\n    fn run() -> i32 {\\n        \
                     let _ = stdio::print(\"ok\\\\n\");\\n        0\\n    }\\n}\\n\
                     krate::export!(Component);\\n' > \"$KRATE_PORT_CANDIDATE/src/lib.rs\"";

    let attacks = [
        (
            "delete",
            "rm -f \"$(dirname \"$KRATE_PORT_CANDIDATE\")/journeys.json\"",
        ),
        (
            "invert",
            "sed -i.bak 's/Expected: The port preserves/Expected: The port need not preserve/' \
             \"$(dirname \"$KRATE_PORT_CANDIDATE\")/JOURNEYS.md\"",
        ),
        (
            "weaken",
            "sed -i.bak 's/Complete the source/Open the/' \
             \"$(dirname \"$KRATE_PORT_CANDIDATE\")/journeys.json\"",
        ),
        (
            "replace",
            "echo '{\"journeys\":[]}' > \"$(dirname \"$KRATE_PORT_CANDIDATE\")/journeys.json\"",
        ),
        (
            "rewrite-the-task",
            "echo 'do whatever' > \"$(dirname \"$KRATE_PORT_CANDIDATE\")/AGENT_TASK.md\"",
        ),
    ];

    for (name, attack) in attacks {
        let bundle = root.path().join(format!("{name}.krate"));
        let workspace = root.path().join(format!("ws-{name}"));
        let output = krate()
            .arg("port")
            .arg(&source)
            .arg("--prepare")
            .arg(&workspace)
            .arg("--author-cmd")
            .arg(format!("{write_app} && {attack}"))
            .arg("--to")
            .arg(&bundle)
            .arg("--no-install")
            .env("CARGO_NET_OFFLINE", "true")
            .output()
            .expect("run port");

        assert!(
            !output.status.success(),
            "the {name} attack was not detected"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("marking its own exam"),
            "the {name} attack must be refused as criteria tampering, got: {stderr}"
        );
        assert!(
            !bundle.is_file(),
            "the {name} attack must stop before packaging, but a bundle was written"
        );
    }

    // And an agent that only writes the app is unaffected. Exit 7, because the
    // primary task is still unverified -- not blocked by this guard.
    let honest_bundle = root.path().join("honest.krate");
    let honest = krate()
        .arg("port")
        .arg(&source)
        .arg("--prepare")
        .arg(root.path().join("ws-honest"))
        .arg("--author-cmd")
        .arg(write_app)
        .arg("--to")
        .arg(&honest_bundle)
        .arg("--no-install")
        .env("CARGO_NET_OFFLINE", "true")
        .output()
        .expect("run port");
    assert_eq!(
        honest.status.code(),
        Some(7),
        "an honest agent must not be caught by the criteria guard: {}",
        String::from_utf8_lossy(&honest.stderr)
    );
    assert!(honest_bundle.is_file());
}

/// `--dump-caps` must never answer about a file it could not read.
///
/// It is the one command whose whole job is to let a stranger decide
/// whether a file is safe to open, and Studio's app_info parses its output
/// verbatim to fill that sheet. A DIRECTORY used to reach the printer with
/// no manifest, fall through to the defaults, and print fifteen effective
/// capabilities with exit 0 -- so the sheet said "Nothing beyond drawing
/// its own window" about a folder nobody had opened. A safety claim about
/// unread bytes is worse than no answer at all (K-769).
#[test]
fn dump_caps_refuses_what_it_could_not_read() {
    let dir = tempfile::tempdir().expect("temp dir");

    // A folder.
    let out = krate()
        .args(["run", "--dump-caps"])
        .arg(dir.path())
        .output()
        .expect("run --dump-caps on a folder");
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !out.status.success(),
        "a folder must not report capabilities: {said}"
    );
    assert!(
        !said.contains("Effective capabilities"),
        "a folder must not print a capability list: {said}"
    );
    assert!(
        said.contains("folder"),
        "and it must say what is wrong in words: {said}"
    );

    // A file that is not a bundle.
    let junk = dir.path().join("fake.krate");
    std::fs::write(&junk, b"not a krate at all").expect("write the fake");
    let out = krate()
        .args(["run", "--dump-caps"])
        .arg(&junk)
        .output()
        .expect("run --dump-caps on junk");
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !out.status.success(),
        "a damaged file must not report capabilities: {said}"
    );
    assert!(
        !said.contains("Effective capabilities"),
        "a damaged file must not print a capability list: {said}"
    );
}

/// A refusal with stderr piped must return at once, on every platform.
///
/// This is the test that would have failed on the Windows lane for three
/// full runs. `krate run` on a file it refuses printed the refusal to
/// stderr and then, on Windows, showed a native modal dialog -- because
/// `GetConsoleWindow()` is null for a headless child exactly as it is for a
/// double-clicked app -- and waited for a click that a CI runner can never
/// give. Every error-path test hung for the watchdog's full ceiling, and
/// the lane burned 46 of its 50 minutes on thirteen of them (K-240).
///
/// A pipe on stderr means somebody is reading it. That is the case here,
/// and the process must exit the moment it has said its piece.
#[test]
fn a_refusal_with_piped_stderr_returns_at_once() {
    let dir = tempfile::tempdir().expect("temp dir");
    let junk = dir.path().join("junk.krate");
    std::fs::write(&junk, b"not a krate").expect("write junk");

    let started = std::time::Instant::now();
    let out = krate()
        .arg("run")
        .arg(&junk)
        .args(["--headless", "--auto-grant"])
        .output()
        .expect("run junk");
    let took = started.elapsed();

    assert!(!out.status.success(), "junk must be refused");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not a Krate app"),
        "the refusal must reach stderr: {stderr}"
    );
    // Generous, so a loaded runner cannot fail it; a hung dialog is the
    // watchdog ceiling, ninety seconds, not fifteen.
    assert!(
        took < std::time::Duration::from_secs(15),
        "a refusal with a reader on stderr must not wait on a dialog: took {took:?}"
    );
}

/// The second app on a machine compiles its own crate and nothing else.
///
/// Every app builds in its own target leaf (K-771), and a leaf starts
/// empty, so the first cut of that design compiled the whole dependency
/// graph -- wit-bindgen-rt, dlmalloc, the SDK crate -- for EVERY app: 13 s
/// alone, 30 s under load, on a build that should take one crate (K-774).
/// A fresh leaf is now seeded from a finished one. Measured with a fresh
/// HOME so the shared cache starts empty: the first build compiles the SDK
/// crate, the second must not.
#[test]
fn a_second_app_reuses_the_first_apps_compiled_dependencies() {
    if !has_cargo_component() {
        eprintln!("skipping: cargo-component not installed");
        return;
    }
    let _build_lock = cargo_build_guard();

    // A fresh HOME empties ~/.cache/krate, so the shared build root starts
    // with no leaf at all. CARGO_HOME and RUSTUP_HOME stay real: the point
    // is an empty Krate cache, not a machine with no Rust on it.
    let home = tempfile::tempdir().expect("home");
    let real_home = std::env::var_os("HOME").expect("HOME is set");
    let cargo_home = std::env::var_os("CARGO_HOME")
        .unwrap_or_else(|| std::path::Path::new(&real_home).join(".cargo").into());
    let rustup_home = std::env::var_os("RUSTUP_HOME")
        .unwrap_or_else(|| std::path::Path::new(&real_home).join(".rustup").into());
    let work = tempfile::tempdir().expect("work");
    let build = |i: u32| {
        krate()
            .arg("create")
            // The built-in generator, not an agent: the agent path warms
            // the cache in a silent background build, and this test needs
            // to read cargo's own words from the build that counts.
            .arg("a grocery list app")
            .arg("--output")
            .arg(work.path().join(format!("out{i}.krate")))
            .arg("--work-dir")
            .arg(work.path().join(format!("w{i}")))
            .env("HOME", home.path())
            .env("CARGO_HOME", &cargo_home)
            .env("RUSTUP_HOME", &rustup_home)
            .env_remove("CARGO_TARGET_DIR")
            // CI sets CARGO_TERM_COLOR=always, and cargo then prints
            // "\x1b[1m\x1b[92m   Compiling\x1b[0m krate v0.5.1": the escape
            // codes sit between the word and the crate, and the plain
            // substring below never matches. Passed locally, failed on the
            // first CI run.
            .env("CARGO_TERM_COLOR", "never")
            .output()
            .expect("run krate create")
    };
    let text = |out: &std::process::Output| {
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    };

    let first = build(0);
    let first_text = text(&first);
    assert!(first.status.success(), "first build: {first_text}");
    // Prove the cache really was empty, or the second assertion is hollow.
    assert!(
        first_text.contains("Compiling krate v"),
        "the first build should have compiled the SDK crate from an empty cache: {first_text}"
    );

    let second = build(1);
    let second_text = text(&second);
    assert!(second.status.success(), "second build: {second_text}");
    assert!(
        !second_text.contains("Compiling krate v"),
        "the second app recompiled the SDK crate instead of reusing the first app's build: \
         {second_text}"
    );
    assert!(
        second_text.contains("Compiling "),
        "the second app's own crate must still be compiled: {second_text}"
    );
}

/// A picture big enough to BE the app gets a word, and an ordinary one does
/// not (K-852).
///
/// A user's Weather app published at 848,626 bytes against a 35-85 KB norm,
/// because its icon was a 1024x1024 photograph. The number is public -- it
/// is on the gallery card and the download page -- and nothing told them.
///
/// Both halves are asserted together because the danger is a warning that
/// fires on everything: an author who sees a note on every ordinary app
/// stops reading notes, and the one that mattered is lost with the rest.
/// The silent case is the half that keeps it worth printing.
#[test]
fn pack_says_when_one_picture_is_most_of_the_app() {
    fn pack_with_icon(label: &str, icon: &[u8]) -> String {
        let dir = tempfile::tempdir().expect("tempdir");
        let assets = dir.path().join("assets");
        std::fs::create_dir_all(&assets).expect("assets dir");
        std::fs::write(assets.join("icon.png"), icon).expect("icon");
        let manifest = dir.path().join("manifest.toml");
        std::fs::write(
            &manifest,
            "[app]\nid = \"dev.krate.icons\"\nname = \"Icons\"\nversion = \"1.0.0\"\n\
             entry = \"code.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n",
        )
        .expect("write manifest");
        let component = dir.path().join("code.wasm");
        std::fs::write(
            &component,
            include_bytes!("../../bundle/tests/fixtures/minimal-run.wasm"),
        )
        .expect("component");

        let packed = krate()
            .args(["pack"])
            .arg(&component)
            .arg("--manifest")
            .arg(&manifest)
            .arg("--output")
            .arg(dir.path().join("app.krate"))
            .output()
            .expect("run pack");
        assert!(
            packed.status.success(),
            "{label} pack: {}",
            String::from_utf8_lossy(&packed.stderr)
        );
        String::from_utf8_lossy(&packed.stdout).into_owned()
    }

    // A photograph used as an icon: big, and far larger than it is ever
    // drawn. This is the real shape, not a synthetic one -- a 960x720 PNG
    // whose IDAT barely compresses, like the user's did.
    let photo = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/krate-nova2/assets/nebula.png"),
    )
    .expect("a real large PNG to stand in for the user's icon");
    assert!(
        photo.len() > 300 * 1024,
        "the fixture has to actually be heavy, or this proves nothing: {} bytes",
        photo.len(),
    );

    let loud = pack_with_icon("photo", &photo);
    assert!(
        loud.contains("icon.png") && loud.contains("% of it"),
        "an icon that is nearly the whole app has to be mentioned: {loud}",
    );
    assert!(
        loud.contains("960x720") && loud.contains("256px"),
        "and the note has to be actionable -- the size it is against the size \
         it is drawn at: {loud}",
    );

    // An ordinary icon, small and near the size it is drawn at.
    let mut ordinary = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut ordinary, 128, 128);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("png header");
        writer
            .write_image_data(&vec![0x40u8; 128 * 128 * 4])
            .expect("png body");
    }
    let quiet = pack_with_icon("ordinary", &ordinary);
    assert!(
        !quiet.contains("note:"),
        "an ordinary icon must say nothing, or the note stops being read: {quiet}",
    );
}

/// K-973: `krate revise` takes a project folder as well as a .krate -- the
/// IDE asks for a change to source that may never have built -- and it
/// writes the change to --output, never into the folder.
#[test]
fn revise_takes_a_project_folder_and_needs_an_output() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
    std::fs::write(dir.path().join("manifest.toml"), "[app]\n").unwrap();
    let out = krate()
        .args(["revise", "--agent", "claude", "--"])
        .arg(dir.path())
        .arg("make it blue")
        .output()
        .expect("run revise");
    assert!(!out.status.success());
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(
        said.contains("--output"),
        "names the missing --output: {said}"
    );

    let not_a_project = tempfile::tempdir().unwrap();
    let out = krate()
        .args(["revise", "--agent", "claude", "--output"])
        .arg(not_a_project.path().join("out.krate"))
        .arg("--")
        .arg(not_a_project.path())
        .arg("make it blue")
        .output()
        .expect("run revise");
    assert!(!out.status.success());
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(
        said.contains("is not a Krate project"),
        "says what is missing: {said}"
    );
}

/// IC-905 M4: the editor's knowledge of Krate comes from the engine, so it
/// says what the AI is taught: real SDK functions, real capability names,
/// and the std modules that are refused.
#[test]
fn sdk_reference_lists_functions_capabilities_and_leaks() {
    let out = krate()
        .arg("sdk-reference")
        .output()
        .expect("run sdk-reference");
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON");
    let funcs = v["functions"].as_array().unwrap();
    assert!(funcs.len() > 20, "the SDK has more than a handful of calls");
    assert!(funcs
        .iter()
        .any(|f| f["signature"].as_str().unwrap_or("").contains("println")));
    let caps = v["capabilities"].as_array().unwrap();
    assert!(caps.iter().any(|c| c == "store.kv"), "{caps:?}");
    let leaks = v["leaks"].as_array().unwrap();
    assert!(leaks.iter().any(|l| l["pattern"] == "std::fs"));
}

/// An app plays a video from its own assets and the platform decodes it
/// (krate:gfx/video, IC-922).
///
/// `video-probe.wasm` is apps/krate-video-probe built ("Clip");
/// `video-sample.mp4` is a 4 s, 320x180 H.264 clip with sound. Packed with
/// the clip, a Mac plays it headless: it loads, reports its own length and
/// size, and its clock moves while the app draws it. Packed without the
/// clip, the app is told the file is not there instead of being shown
/// black. Another system says it cannot play video yet.
#[test]
fn an_app_plays_a_video_from_its_assets() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = |name: &str, with_clip: bool| -> std::path::PathBuf {
        let src = dir.path().join(name);
        std::fs::create_dir_all(&src).expect("app dir");
        std::fs::write(
            src.join("code.wasm"),
            include_bytes!("fixtures/video-probe.wasm"),
        )
        .expect("probe");
        std::fs::write(
            src.join("manifest.toml"),
            "[app]\nid = \"dev.krate.video-probe\"\nname = \"Clip\"\nversion = \"1.0.0\"\n\
             entry = \"code.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n\n\
             [[capabilities]]\ncap = \"ui.window:create\"\nrationale = \"window\"\nrequired = true\n\n\
             [[capabilities]]\ncap = \"io.stdout\"\nrationale = \"print\"\nrequired = true\n\n\
             [[capabilities]]\ncap = \"io.args\"\nrationale = \"args\"\nrequired = true\n",
        )
        .expect("manifest");
        let clips = src.join("assets").join("clips");
        std::fs::create_dir_all(&clips).expect("assets");
        if with_clip {
            std::fs::write(
                clips.join("sample.mp4"),
                include_bytes!("fixtures/video-sample.mp4"),
            )
            .expect("clip");
        } else {
            std::fs::write(clips.join("readme.txt"), "no clip here").expect("other asset");
        }
        let bundle = dir.path().join(format!("{name}.krate"));
        let packed = krate()
            .arg("pack")
            .arg(src.join("code.wasm"))
            .arg("--manifest")
            .arg(src.join("manifest.toml"))
            .arg("-o")
            .arg(&bundle)
            .output()
            .expect("pack");
        assert!(
            packed.status.success(),
            "{}",
            String::from_utf8_lossy(&packed.stderr)
        );
        bundle
    };
    let run = |bundle: &std::path::Path| -> String {
        let out = krate()
            .arg("run")
            .arg("--headless")
            .arg(bundle)
            .arg("--auto-grant")
            .arg("--")
            .arg("quick")
            .output()
            .expect("run");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };

    let missing = run(&app("without", false));
    assert!(
        missing.contains("video:problem=") && missing.contains("there is no clips/sample.mp4"),
        "a clip the app does not carry is named, not shown black: {missing}"
    );

    let played = run(&app("with", true));
    if cfg!(target_os = "macos") {
        assert!(
            played
                .lines()
                .any(|l| l.starts_with("video:ready=yes duration=4000 size=320x180 moved=yes")),
            "a Mac loads the clip, knows its length and size, and plays it: {played}"
        );
    } else {
        assert!(
            played.contains("video playback is not on this system yet"),
            "another system says it cannot play video yet: {played}"
        );
    }
}

/// An app reaches the GPU through the binary a person runs (krate:gpu,
/// IC-540, IC-916).
///
/// `gpu-probe.wasm` asks for a device and runs one check per line: a WGSL
/// shader with a type error is refused by validation with the line it is on
/// and never reaches the driver (the shader door, F-311); a triangle is drawn
/// and its pixel read back; a compute pass doubles four numbers; a handle that
/// was never given out and an impossible buffer are refused with the app still
/// running; a device lost on purpose says so on the next call and a fresh one
/// works. A machine with no GPU at all says so and the test stands down -- it
/// is the only case where these lines do not appear.
#[test]
fn an_app_draws_and_computes_on_the_gpu_and_a_bad_shader_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(&wasm, include_bytes!("fixtures/gpu-probe.wasm")).expect("probe");
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"com.acme.gpu-probe\"\nname = \"gpu-probe\"\nversion = \"1.0.0\"\n\
         entry = \"code.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n\n\
         [[capabilities]]\ncap = \"ui.window:create\"\nrationale = \"window\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"io.stdout\"\nrationale = \"print\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"io.args\"\nrationale = \"args\"\nrequired = true\n",
    )
    .expect("manifest");
    let out = krate()
        .arg("run")
        .arg("--headless")
        .arg(&wasm)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--auto-grant")
        .arg("--")
        .arg("check")
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if stdout.starts_with("adapter=Unsupported") {
        eprintln!("skipping: this machine has no GPU: {stdout}");
        return;
    }
    let line = |key: &str| -> String {
        stdout
            .lines()
            .find(|l| l.starts_with(key))
            .unwrap_or_else(|| panic!("no {key} line in: {stdout}{stderr}"))
            .to_string()
    };
    let shader = line("shader=");
    assert!(
        shader.starts_with("shader=refused line=4 column=9 "),
        "the broken shader's type error is on line 4: {shader}"
    );
    assert!(
        shader.contains("expected to be `vec3<f32>`, but got `f32`"),
        "the refusal says what is wrong: {shader}"
    );
    assert_eq!(line("pixel="), "pixel=255,0,0,255", "the triangle is red");
    assert_eq!(
        line("compute="),
        "compute=2,4,6,8",
        "the compute pass doubled"
    );
    assert_eq!(
        line("invalid-handle="),
        "invalid-handle=refused oversized-buffer=refused"
    );
    assert_eq!(line("lost="), "lost=yes recovered=yes");
    assert!(out.status.success(), "{stdout}{stderr}");
}

/// A threaded app (IC-929) through the binary a person runs: a core module
/// over a shared memory, built for wasm32-wasip1-threads, whose `std::thread`,
/// scoped threads and rayon pool are real OS threads sharing its memory, and
/// whose processor count comes from the host.
#[test]
fn a_threaded_app_runs_its_threads_and_rayon_pool_on_real_cores() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wasm = dir.path().join("app.wasm");
    std::fs::write(&wasm, include_bytes!("fixtures/threads-probe.wasm")).expect("probe");
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"com.acme.threads\"\nname = \"threads\"\nversion = \"1.0.0\"\n\
         entry = \"app.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n\n\
         [[capabilities]]\ncap = \"io.stdout\"\nrationale = \"print\"\nrequired = true\n",
    )
    .expect("manifest");
    let out = krate()
        .arg("run")
        .arg(&wasm)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--auto-grant")
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "{stdout}{stderr}");
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    assert!(
        stdout.contains(&format!("available_parallelism: Ok({cores})")),
        "the app is told this machine's {cores} cores: {stdout}"
    );
    assert!(
        stdout.contains("std::thread ThreadId(2) returned"),
        "a std::thread ran and was joined: {stdout}"
    );
    assert!(
        stdout.contains("4 scoped threads counted 400000"),
        "four scoped threads shared one counter: {stdout}"
    );
    assert!(
        stdout.contains("parking_lot: 8 threads took their turns (8) and counted 400000"),
        "parking_lot's Mutex and Condvar parked and woke eight threads through \
         make-parking-lot.py's std parker (IC-939): {stdout}"
    );
    assert!(
        stdout.contains(&format!("rayon on {cores} threads"))
            && stdout.contains("same answer: true"),
        "rayon sized its pool from the cores and agreed with one thread: {stdout}"
    );
}

/// A threaded app packs into a .krate, passes the door, and runs from it
/// (IC-938): the same threads probe, through `krate pack` and `krate run
/// app.krate` -- the path a double-click takes.
#[test]
fn a_threaded_app_packs_into_a_krate_and_runs_from_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wasm = dir.path().join("app.wasm");
    std::fs::write(&wasm, include_bytes!("fixtures/threads-probe.wasm")).expect("probe");
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"com.acme.threads\"\nname = \"threads\"\nversion = \"1.0.0\"\n\
         entry = \"app.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n\n\
         [[capabilities]]\ncap = \"io.stdout\"\nrationale = \"print\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"random.bytes\"\nrationale = \"seed\"\nrequired = true\n",
    )
    .expect("manifest");
    let bundle = dir.path().join("threads.krate");
    let packed = krate()
        .arg("pack")
        .arg(&wasm)
        .arg("--manifest")
        .arg(&manifest)
        .arg("-o")
        .arg(&bundle)
        .output()
        .expect("pack");
    assert!(
        packed.status.success(),
        "{}",
        String::from_utf8_lossy(&packed.stderr)
    );
    let out = krate()
        .arg("run")
        .arg(&bundle)
        .arg("--auto-grant")
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("same answer: true"), "{stdout}");
    assert!(
        stdout.contains("4 scoped threads counted 400000")
            && stdout.contains("parking_lot: 8 threads took their turns (8)"),
        "{stdout}"
    );
    // The pool is really parallel from the .krate, built on stable Rust
    // (K-1055): two or more of its threads were working at the same moment.
    // Counted, not timed -- a speed-up says how busy the machine was (1.01x
    // on a 4-core CI runner beside 250 other tests, 7x on a quiet Mac), and
    // only overlap says the threads are real. Asked of two cores or more.
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    if cores >= 2 {
        let at_once: usize = stdout
            .split("at most ")
            .nth(1)
            .and_then(|rest| rest.split(' ').next())
            .and_then(|n| n.parse().ok())
            .unwrap_or(0);
        assert!(
            at_once >= 2,
            "rayon on {cores} cores must run two threads at once: {stdout}"
        );
    }
}

/// A controller reaches a gilrs program (IC-936): `gamepad-probe.wasm` is
/// apps/krate-gamepad-probe built, gilrs' event loop over gilrs-core's Krate
/// backend. A driven headless run plugs in the stand-in pad, holds south and
/// pushes the left stick, and gilrs reports each as its own events.
#[test]
fn a_gilrs_program_sees_a_stand_in_controller() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wasm = dir.path().join("app.wasm");
    std::fs::write(&wasm, include_bytes!("fixtures/gamepad-probe.wasm")).expect("probe");
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"com.acme.pad\"\nname = \"pad\"\nversion = \"1.0.0\"\n\
         entry = \"app.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n\n\
         [[capabilities]]\ncap = \"ui.window:create\"\nrationale = \"window\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"io.stdout\"\nrationale = \"print\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"random.bytes\"\nrationale = \"seed\"\nrequired = true\n",
    )
    .expect("manifest");
    let task = dir.path().join("task.toml");
    std::fs::write(
        &task,
        "name = \"press south and push the stick\"\n\n[[step]]\nwait_ms = 300\n\n\
         [[step]]\npad = \"south 300\"\n\n[[step]]\nstick = \"left-y 0.5 300\"\n\n\
         [[step]]\nexpect_no_text = \"a crash dialog\"\n",
    )
    .expect("task");
    let out = krate()
        .env("KRATE_HEADLESS_SECS", "40")
        .arg("run")
        .arg(&wasm)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--auto-grant")
        .arg("--headless")
        .arg("--usability-report")
        .arg(dir.path().join("report.json"))
        .arg("--task")
        .arg(&task)
        .arg("--")
        .arg("25")
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    for want in [
        "gilrs: started; 0 pad(s) known",
        "Connected",
        "ButtonPressed(South",
        "ButtonReleased(South",
        "AxisChanged(LeftStickY, 0.4",
        "AxisChanged(LeftStickY, 0.0",
    ] {
        assert!(
            stdout.contains(want),
            "{want:?} missing from: {stdout}{stderr}"
        );
    }
}

/// An app signs the person in through the system's sheet and checks what
/// comes back (krate:ui/sign-in, IC-923).
///
/// `sign-in-probe.wasm` is apps/krate-sign-in-probe built ("Sign In").
/// Headless, KRATE_SIGN_IN stands in for the person: the code the service
/// sends back reaches the app with the app's own `state`, beside a 43-
/// character PKCE verifier; `cancel` is the person closing the sheet; with
/// no stand-in there is no sheet and the app is told so. An http address
/// is refused before any sheet opens.
#[test]
fn an_app_signs_in_through_the_system_sheet() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(&wasm, include_bytes!("fixtures/sign-in-probe.wasm")).expect("probe");
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"dev.krate.sign-in-probe\"\nname = \"Sign In\"\nversion = \"1.0.0\"\n\
         entry = \"code.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n\n\
         [[capabilities]]\ncap = \"ui.window:create\"\nrationale = \"window\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"random.bytes\"\nrationale = \"state\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"io.stdout\"\nrationale = \"print\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"io.args\"\nrationale = \"args\"\nrequired = true\n",
    )
    .expect("manifest");
    let run = |answer: Option<&str>| -> String {
        let mut cmd = krate();
        cmd.arg("run")
            .arg("--headless")
            .arg(&wasm)
            .arg("--manifest")
            .arg(&manifest)
            .arg("--auto-grant")
            .arg("--")
            .arg("quick")
            .env_remove("KRATE_SIGN_IN");
        if let Some(answer) = answer {
            cmd.env("KRATE_SIGN_IN", answer);
        }
        let out = cmd.output().expect("run");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let signed = run(Some("c0de42"));
    assert!(
        signed
            .lines()
            .any(|l| l == "sign-in:code=c0de42 state=ok pkce=43"),
        "the code comes back with the app's own state, beside a full verifier: {signed}"
    );
    assert!(
        signed.lines().any(|l| l == "sign-in:http-refused"),
        "an http address never reaches the sheet: {signed}"
    );
    let closed = run(Some("cancel"));
    assert!(
        closed.lines().any(|l| l == "sign-in:cancelled"),
        "closing the sheet is a cancel, not an error: {closed}"
    );
    let none = run(None);
    assert!(
        none.contains("sign-in:problem=SignInError::Unavailable"),
        "with nothing on screen there is no sheet, and the app is told: {none}"
    );
}

/// A vault opens only for its owner (krate:ui/unlock, IC-924).
///
/// `unlock-probe.wasm` is apps/krate-unlock-probe built ("Vault").
/// Headless, KRATE_UNLOCK stands in for the person at the system's Face ID
/// or Touch ID sheet: a yes opens the vault and its three entries are read
/// from the secret store; a no keeps it locked; a device that cannot check
/// says so and keeps it locked too.
#[test]
fn a_vault_opens_only_for_its_owner() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(&wasm, include_bytes!("fixtures/unlock-probe.wasm")).expect("probe");
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"dev.krate.unlock-probe\"\nname = \"Vault\"\nversion = \"1.0.0\"\n\
         entry = \"code.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n\n\
         [[capabilities]]\ncap = \"ui.window:create\"\nrationale = \"window\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"store.secret\"\nrationale = \"vault\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"io.stdout\"\nrationale = \"print\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"io.args\"\nrationale = \"args\"\nrequired = true\n",
    )
    .expect("manifest");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).expect("home");
    let run = |answer: &str| -> String {
        let out = krate()
            .arg("run")
            .arg("--headless")
            .arg(&wasm)
            .arg("--manifest")
            .arg(&manifest)
            .arg("--auto-grant")
            .arg("--")
            .arg("quick")
            .env("HOME", &home)
            .env("KRATE_UNLOCK", answer)
            .output()
            .expect("run");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let owner = run("yes");
    assert!(
        owner
            .lines()
            .any(|l| l == "unlock:available=yes answer=yes entries=3"),
        "the owner opens the vault and reads its entries: {owner}"
    );
    let stranger = run("no");
    assert!(
        stranger
            .lines()
            .any(|l| l == "unlock:available=yes answer=no entries=hidden"),
        "a no keeps the vault locked: {stranger}"
    );
    let cannot = run("none");
    assert!(
        cannot
            .lines()
            .any(|l| l == "unlock:available=no answer=unavailable entries=hidden"),
        "a device that cannot check keeps it locked and says so: {cannot}"
    );
}

/// The person's dark mode, contrast and text size reach an app
/// (krate:ui/appearance, IC-925).
///
/// `look-probe.wasm` is apps/krate-look-probe built ("Look"). Headless,
/// KRATE_APPEARANCE stands in for the phone's settings; without it the
/// look is the default: light, no extra contrast, text at 1.
#[test]
fn the_persons_look_reaches_the_app() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(&wasm, include_bytes!("fixtures/look-probe.wasm")).expect("probe");
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"dev.krate.look-probe\"\nname = \"Look\"\nversion = \"1.0.0\"\n\
         entry = \"code.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n\n\
         [[capabilities]]\ncap = \"ui.window:create\"\nrationale = \"window\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"io.stdout\"\nrationale = \"print\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"io.args\"\nrationale = \"args\"\nrequired = true\n",
    )
    .expect("manifest");
    let run = |look: Option<&str>| -> String {
        let mut cmd = krate();
        cmd.arg("run")
            .arg("--headless")
            .arg(&wasm)
            .arg("--manifest")
            .arg(&manifest)
            .arg("--auto-grant")
            .arg("--")
            .arg("quick")
            .env_remove("KRATE_APPEARANCE");
        if let Some(look) = look {
            cmd.env("KRATE_APPEARANCE", look);
        }
        let out = cmd.output().expect("run");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let plain = run(None);
    assert!(
        plain
            .lines()
            .any(|l| l == "look:dark=no contrast=no text=1.00"),
        "{plain}"
    );
    let dark = run(Some("dark,contrast,text=1.35"));
    assert!(
        dark.lines()
            .any(|l| l == "look:dark=yes contrast=yes text=1.35"),
        "dark mode, contrast and text size reach the app whole: {dark}"
    );
}

/// Krate draws a widget app's own widgets in the person's look (IC-925):
/// in dark mode the window, fields and text go dark without the app doing
/// anything, and at a larger text size its labels take more room.
///
/// `timer-widgets.wasm` is apps/krate-timer built: a title, a time, a
/// progress bar and three buttons, with no colours of its own.
#[test]
fn a_widget_app_is_drawn_in_the_persons_look() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(&wasm, include_bytes!("fixtures/timer-widgets.wasm")).expect("timer");
    let manifest = dir.path().join("manifest.toml");
    std::fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../apps/krate-timer/manifest.toml"
        ),
        &manifest,
    )
    .expect("manifest");
    let text = std::fs::read_to_string(&manifest).expect("read manifest");
    let entry = text
        .lines()
        .find(|l| l.starts_with("entry"))
        .expect("entry line")
        .to_string();
    std::fs::write(&manifest, text.replace(&entry, "entry = \"code.wasm\"")).expect("manifest");
    let shoot = |look: &str| -> f64 {
        let png = dir
            .path()
            .join(format!("{}.png", look.replace([',', '='], "-")));
        let out = krate()
            .arg("run")
            .arg(&wasm)
            .arg("--manifest")
            .arg(&manifest)
            .arg("--auto-grant")
            .arg("--shoot")
            .arg(&png)
            .arg("--shoot-scale")
            .arg("1")
            .arg("--")
            .arg("quick")
            .env("KRATE_SCREEN", "390x844")
            .env("KRATE_APPEARANCE", look)
            .output()
            .expect("run");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let decoder = png::Decoder::new(std::io::BufReader::new(
            std::fs::File::open(&png).expect("shot"),
        ));
        let mut reader = decoder.read_info().expect("png");
        let mut buf = vec![0; reader.output_buffer_size().expect("size")];
        let info = reader.next_frame(&mut buf).expect("frame");
        let channels = info.color_type.samples();
        let px = &buf[..info.buffer_size()];
        let sum: f64 = px
            .chunks_exact(channels)
            .map(|p| (p[0] as f64 + p[1] as f64 + p[2] as f64) / (3.0 * 255.0))
            .sum();
        sum / (px.len() / channels) as f64
    };
    let light = shoot("light");
    let dark = shoot("dark,text=1.35");
    assert!(light > 0.8, "a light window by default: {light:.2}");
    assert!(dark < 0.3, "the same app goes dark in dark mode: {dark:.2}");
}

/// An app that draws itself tells the screen reader what it drew
/// (krate:ui/accessibility, IC-926).
///
/// `look-probe.wasm` describes its page with the layout kit's `Said`: the
/// heading, the line saying the look, and each paragraph, with the rects it
/// drew them in. Headless, `KRATE_ACCESS_DUMP` writes what the host was
/// told, so this reads what VoiceOver and TalkBack would.
#[test]
fn an_app_tells_the_screen_reader_what_it_drew() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(&wasm, include_bytes!("fixtures/look-probe.wasm")).expect("probe");
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"dev.krate.look-probe\"\nname = \"Look\"\nversion = \"1.0.0\"\n\
         entry = \"code.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n\n\
         [[capabilities]]\ncap = \"ui.window:create\"\nrationale = \"window\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"io.stdout\"\nrationale = \"print\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"io.args\"\nrationale = \"args\"\nrequired = true\n",
    )
    .expect("manifest");
    let dump = dir.path().join("said.json");
    let out = krate()
        .arg("run")
        .arg("--headless")
        .arg(&wasm)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--auto-grant")
        .arg("--")
        .arg("quick")
        .env("KRATE_ACCESS_DUMP", &dump)
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let said: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&dump).expect("the app described its screen"),
    )
    .expect("json");
    let said = said.as_array().expect("a list of elements");
    assert_eq!(
        said.len(),
        5,
        "a heading, the look line and three paragraphs: {said:?}"
    );
    assert_eq!(said[0]["role"], "heading");
    assert_eq!(said[0]["label"], "Look");
    assert!(said[1]["label"]
        .as_str()
        .unwrap_or("")
        .starts_with("Light mode"));
    assert!(said
        .iter()
        .skip(2)
        .all(|e| e["role"] == "text" && e["frame"][2].as_f64().unwrap_or(0.0) > 100.0));
}

/// A web page inside an app goes only where the app may go
/// (krate:ui/web-page, IC-927).
///
/// `web-probe.wasm` is apps/krate-web-probe built ("Help"), granted
/// net.connect to krate.tech only. Its page opens on krate.tech; a `go` to
/// example.com, an `open` of it, and a plain http address are all refused,
/// and the refusal is said in the page's `problem`.
#[test]
fn a_web_page_inside_an_app_goes_only_where_the_app_may() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wasm = dir.path().join("code.wasm");
    std::fs::write(&wasm, include_bytes!("fixtures/web-probe.wasm")).expect("probe");
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"dev.krate.web-probe\"\nname = \"Help\"\nversion = \"1.0.0\"\n\
         entry = \"code.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n\n\
         [[capabilities]]\ncap = \"ui.window:create\"\nrationale = \"window\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"net.connect:krate.tech:443\"\nrationale = \"help\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"io.stdout\"\nrationale = \"print\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"io.args\"\nrationale = \"args\"\nrequired = true\n",
    )
    .expect("manifest");
    let out = krate()
        .arg("run")
        .arg("--headless")
        .arg(&wasm)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--auto-grant")
        .arg("--")
        .arg("quick")
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(
        said.lines().any(|l| l == "web:open=ok"),
        "a granted host opens: {said}"
    );
    assert!(
        said.lines().any(|l| l
            == "web:go-ungranted=refused problem=example.com is not a place this app may open"),
        "a host it was never granted is refused, and said: {said}"
    );
    assert!(
        said.lines().any(|l| l == "web:open-ungranted=refused"),
        "{said}"
    );
    assert!(
        said.lines().any(|l| l == "web:open-http=refused"),
        "plain http never: {said}"
    );
}

/// A tokio task woken from another thread runs promptly in a threaded app
/// (K-1069). On WASI tokio cannot wake its I/O driver from another thread, so
/// with `enable_all` and one worker such a task waited for the driver's next
/// timer: 28.6 s in this probe, which is how Veloren's QUIC connection
/// stalled. krate-c-runtime now caps an event loop's wait at 1 ms.
#[test]
fn a_tokio_task_woken_from_another_thread_runs_within_milliseconds() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wasm = dir.path().join("app.wasm");
    std::fs::write(&wasm, include_bytes!("fixtures/sockets-probe.wasm")).expect("probe");
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"com.acme.sockets\"\nname = \"sockets\"\nversion = \"1.0.0\"\n\
         entry = \"app.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n\n\
         [[capabilities]]\ncap = \"io.stdout\"\nrationale = \"print\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"random.bytes\"\nrationale = \"seed\"\nrequired = true\n",
    )
    .expect("manifest");
    let out = krate()
        .arg("run")
        .arg(&wasm)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--auto-grant")
        .arg("--")
        .arg("wake")
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let ms: f64 = stdout
        .split("waited at most ")
        .nth(1)
        .and_then(|rest| rest.split(' ').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| {
            panic!(
                "no wait reported: {stdout}{}",
                String::from_utf8_lossy(&out.stderr)
            )
        });
    assert!(
        ms < 250.0,
        "a woken task waited {ms} ms for tokio's driver: {stdout}"
    );
}

/// Raw sockets inside a Krate app (IC-935): `sockets-probe.wasm` is
/// apps/krate-sockets-probe built. Its TCP, UDP, lookup and tokio jobs reach
/// echo servers this test runs, under a `net.connect` grant; without the
/// grant every one is refused before a socket exists.
#[test]
fn an_app_reaches_tcp_and_udp_servers_it_was_granted_and_no_others() {
    use std::io::{Read, Write};
    let tcp = std::net::TcpListener::bind("127.0.0.1:0").expect("tcp");
    let tcp_port = tcp.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in tcp.incoming().flatten() {
            let mut s = stream;
            let mut buf = [0u8; 11];
            if s.read_exact(&mut buf).is_ok() {
                let _ = s.write_all(&buf);
            }
        }
    });
    let udp = std::net::UdpSocket::bind("127.0.0.1:0").expect("udp");
    let udp_port = udp.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let mut buf = [0u8; 64];
        while let Ok((n, from)) = udp.recv_from(&mut buf) {
            let mut out = b"pong:".to_vec();
            out.extend_from_slice(&buf[..n]);
            let _ = udp.send_to(&out, from);
        }
    });
    let dir = tempfile::tempdir().expect("tempdir");
    let wasm = dir.path().join("app.wasm");
    std::fs::write(&wasm, include_bytes!("fixtures/sockets-probe.wasm")).expect("probe");
    let manifest = |grant: &str| {
        let path = dir.path().join("manifest.toml");
        std::fs::write(
            &path,
            format!(
                "[app]\nid = \"com.acme.sockets\"\nname = \"sockets\"\nversion = \"1.0.0\"\n\
                 entry = \"app.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n\n\
                 [[capabilities]]\ncap = \"io.stdout\"\nrationale = \"print\"\nrequired = true\n\n\
                 [[capabilities]]\ncap = \"random.bytes\"\nrationale = \"seed\"\nrequired = true\n\n\
                 [[capabilities]]\ncap = \"{grant}\"\nrationale = \"the echo servers\"\nrequired = true\n"
            ),
        )
        .expect("manifest");
        path
    };
    let run = |manifest: &std::path::Path, job: &[String]| {
        let out = krate()
            .arg("run")
            .arg(&wasm)
            .arg("--manifest")
            .arg(manifest)
            .arg("--auto-grant")
            .arg("--")
            .args(job)
            .output()
            .expect("run");
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    };
    let job = |words: &str| words.split(' ').map(String::from).collect::<Vec<_>>();
    let granted = manifest("net.connect:localhost:*");
    let out = run(&granted, &job("lookup localhost"));
    assert!(out.contains("127.0.0.1"), "{out}");
    let out = run(&granted, &job(&format!("tcp localhost {tcp_port}")));
    assert!(out.contains("got back \"hello krate\""), "{out}");
    let out = run(&granted, &job(&format!("udp localhost {udp_port}")));
    assert!(out.contains("got \"pong:ping\""), "{out}");
    let out = run(&granted, &job(&format!("tokio localhost {tcp_port}")));
    assert!(out.contains("got back \"hello krate\""), "{out}");

    // A grant for another host covers none of it: refused, no socket made.
    let other = manifest("net.connect:play.example.com:*");
    for words in [
        "lookup localhost".to_string(),
        format!("tcp localhost {tcp_port}"),
        format!("udp localhost {udp_port}"),
        format!("tcp 127.0.0.1 {tcp_port}"),
    ] {
        let out = run(&other, &job(&words));
        assert!(
            out.contains("refused or failed: permission denied"),
            "{words}: {out}"
        );
    }
}

/// A tokio multi-thread runtime inside a Krate app (IC-929): tasks spread over
/// worker threads, a channel between tasks, a timer and spawn_blocking.
#[test]
fn a_tokio_multi_thread_runtime_runs_inside_an_app() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wasm = dir.path().join("app.wasm");
    std::fs::write(&wasm, include_bytes!("fixtures/tokio-probe.wasm")).expect("probe");
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"com.acme.tokio\"\nname = \"tokio\"\nversion = \"1.0.0\"\n\
         entry = \"app.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n\n\
         [[capabilities]]\ncap = \"io.stdout\"\nrationale = \"print\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"random.bytes\"\nrationale = \"seed\"\nrequired = true\n",
    )
    .expect("manifest");
    let out = krate()
        .arg("run")
        .arg(&wasm)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--auto-grant")
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "{stdout}{stderr}");
    let workers: usize = stdout
        .split(" worker threads")
        .next()
        .and_then(|s| s.rsplit(' ').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no worker count in: {stdout}{stderr}"));
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    assert!(
        workers > 1 || cores == 1,
        "tasks ran on more than one worker: {stdout}"
    );
    assert!(stdout.contains("same answer: true"), "{stdout}");
    assert!(stdout.contains("channel got [0, 1, 2, 3, 4]"), "{stdout}");
    let timer: u64 = stdout
        .split("a 100 ms timer took ")
        .nth(1)
        .and_then(|s| s.split(' ').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no timer line in: {stdout}"));
    assert!(
        (100..2000).contains(&timer),
        "the timer waited its time: {timer} ms"
    );
}

/// winit's own `control_flow` example, its source unchanged, runs as a Krate
/// app (IC-930): built with winit patched by crates/winit-krate/make-winit.py
/// for wasm32-wasip1-threads, it starts at `_start` like any program. Keys
/// pressed by the usability driver arrive as winit `KeyboardInput` with
/// physical keys; "3" switches it to Poll mode, "r" turns on redraw requests,
/// and Escape ends the event loop and the app with exit code 0.
#[test]
fn an_unmodified_winit_example_runs_and_hears_its_keys() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wasm = dir.path().join("app.wasm");
    std::fs::write(&wasm, include_bytes!("fixtures/winit-control-flow.wasm")).expect("example");
    let manifest = dir.path().join("manifest.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"com.acme.winit\"\nname = \"winit\"\nversion = \"1.0.0\"\n\
         entry = \"app.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n\n\
         [[capabilities]]\ncap = \"ui.window:create\"\nrationale = \"window\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"io.stdout\"\nrationale = \"print\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"random.bytes\"\nrationale = \"seed\"\nrequired = true\n",
    )
    .expect("manifest");
    let task = dir.path().join("task.toml");
    std::fs::write(
        &task,
        "name = \"switch modes and close\"\n\n[[step]]\nwait_ms = 400\n\n[[step]]\nkey = \"3\"\n\n\
         [[step]]\nwait_ms = 300\n\n[[step]]\nkey = \"r\"\n\n[[step]]\nwait_ms = 300\n\n\
         [[step]]\nkey = \"Escape\"\n\n[[step]]\nwait_ms = 300\n\n\
         [[step]]\nexpect_no_text = \"a crash dialog\"\n",
    )
    .expect("task");
    let out = krate()
        .arg("run")
        .arg(&wasm)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--auto-grant")
        .arg("--usability-report")
        .arg(dir.path().join("report.json"))
        .arg("--task")
        .arg(&task)
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "{stdout}{stderr}");
    for want in [
        "Resized(PhysicalSize",
        "physical_key: Code(Digit3), logical_key: Character(\"3\")",
        "mode: Poll",
        "physical_key: Code(KeyR)",
        "request_redraw: true",
        "RedrawRequested",
        "physical_key: Code(Escape), logical_key: Named(Escape)",
    ] {
        assert!(stdout.contains(want), "no {want:?} in: {stdout}{stderr}");
    }
}

/// A from-scratch app whose crate compiles C (IC-941, K-1072): `cbits`'s
/// build script compiles `answer.c` with cc-rs, which includes <string.h>,
/// as SQLite and ring do. The app prints what the C returns.
fn c_app(dir: &std::path::Path) {
    let sdk = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../bindings-rust");
    let crt = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../c-runtime");
    let write = |path: &str, text: &str| {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write(
        "Cargo.toml",
        &format!(
            "[package]\nname = \"c-answer\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
             [workspace]\n\n[lib]\ncrate-type = [\"cdylib\"]\n\n[dependencies]\n\
             cbits = {{ path = \"cbits\" }}\n\
             krate = {{ path = {sdk:?}, features = [\"std\"] }}\n\
             krate-c-runtime = {{ path = {crt:?} }}\n\
             wit-bindgen-rt = {{ version = \"0.44.0\", features = [\"bitflags\"] }}\n\n\
             [profile.release]\npanic = \"abort\"\n"
        ),
    );
    write(
        "src/lib.rs",
        "struct App;\nimpl krate::Guest for App {\n    fn run() -> i32 {\n        \
         krate_c_runtime::start();\n        println!(\"the C says {}\", cbits::answer());\n        \
         krate_c_runtime::finish();\n        0\n    }\n}\nkrate::export!(App);\n",
    );
    write(
        "manifest.toml",
        "[app]\nid = \"dev.krate.c-answer\"\nname = \"C answer\"\nversion = \"0.1.0\"\n\
         entry = \"target/wasm32-wasip1/release/c_answer.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n\n\
         [[capabilities]]\ncap = \"io.stdout\"\nrationale = \"print\"\nrequired = true\n",
    );
    write(
        "cbits/Cargo.toml",
        "[package]\nname = \"cbits\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [build-dependencies]\ncc = \"1\"\n",
    );
    write(
        "cbits/build.rs",
        "fn main() { cc::Build::new().file(\"answer.c\").compile(\"answer\"); }\n",
    );
    write(
        "cbits/answer.c",
        "#include <string.h>\nint krate_answer(void) { return (int)strlen(\"forty-two chars of C, compiled for WASI!!\"); }\n",
    );
    write(
        "cbits/src/lib.rs",
        "extern \"C\" { fn krate_answer() -> i32; }\n\
         pub fn answer() -> i32 { unsafe { krate_answer() } }\n",
    );
}

/// An app builds on whichever Rust rustup selects, including stable 1.99
/// and later (K-1078).
///
/// From 1.99, rust-lld loads libLLVM.dylib by an rpath that resolves only
/// under the library path rustup's shims set. Krate put the toolchain's own
/// bin first on the build's PATH, so cargo ran outside the shims and no wasm
/// could link: "Library not loaded: @rpath/libLLVM.dylib". Stands down when
/// this machine has no stable toolchain with the wasm32-wasip1 target.
#[test]
fn an_app_builds_on_the_stable_rust_rustup_selects() {
    if !has_cargo_component() {
        eprintln!("skipping: cargo-component is not installed");
        return;
    }
    let rustup = |args: &[&str]| {
        Command::new("rustup")
            .args(args)
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let Some(sysroot) = rustup(&["run", "stable", "rustc", "--print", "sysroot"]) else {
        eprintln!("skipping: no stable toolchain");
        return;
    };
    if !std::path::Path::new(&sysroot)
        .join("lib/rustlib/wasm32-wasip1")
        .is_dir()
    {
        eprintln!("skipping: stable has no wasm32-wasip1 target");
        return;
    }
    let version = rustup(&["run", "stable", "rustc", "--version"]).unwrap_or_default();
    // "rustc 1.99.0 (b940084d7 2026-09-28)" -> "1.99.0"
    let number = version
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_string();

    let dir = tempfile::tempdir().expect("tempdir");
    let sdk = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../bindings-rust");
    let write = |path: &str, text: &str| {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write(
        "Cargo.toml",
        &format!(
            "[package]\nname = \"on-stable\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
             [workspace]\n\n[lib]\ncrate-type = [\"cdylib\"]\n\n[dependencies]\n\
             krate = {{ path = {sdk:?} }}\n\
             wit-bindgen-rt = {{ version = \"0.44.0\", features = [\"bitflags\"] }}\n\n\
             [profile.release]\npanic = \"abort\"\n"
        ),
    );
    write(
        "src/lib.rs",
        "#![no_std]\nextern crate alloc;\nstruct App;\nimpl krate::Guest for App {\n    fn run() -> i32 { 0 }\n}\n\
         krate::export!(App);\n",
    );
    write(
        "manifest.toml",
        "[app]\nid = \"dev.krate.on-stable\"\nname = \"On stable\"\nversion = \"0.1.0\"\n\
         entry = \"target/wasm32-wasip1/release/on_stable.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n",
    );
    let out = krate()
        .arg("check-app")
        .arg(dir.path())
        .arg("--no-run")
        .env("RUSTUP_TOOLCHAIN", "stable")
        .output()
        .expect("check-app");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !text.contains("libLLVM"),
        "the build ran stable's cargo outside rustup's shims: {text}"
    );
    assert!(out.status.success(), "the app builds on {version}: {text}");
    let built = std::fs::read(
        dir.path()
            .join("target/wasm32-wasip1/release/on_stable.wasm"),
    )
    .unwrap_or_default();
    let said = String::from_utf8_lossy(&built);
    assert!(
        !number.is_empty() && said.contains(&number),
        "the app was compiled by the selected {version}"
    );
}

/// With no WASI C compiler anywhere Krate looks, an app whose crates compile
/// C fails its build with the crate named and the one step that fixes it --
/// not "fix the compiler errors above" about code its author never wrote
/// (K-1072).
#[test]
fn an_app_whose_crate_compiles_c_says_how_to_get_a_compiler_when_there_is_none() {
    if !has_cargo_component() {
        eprintln!("skipping: cargo-component is not installed");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    c_app(dir.path());
    let empty = tempfile::tempdir().expect("no toolchains");
    let out = krate()
        .arg("check-app")
        .arg(dir.path())
        .args(["--no-run", "--json"])
        .env("KRATE_TOOLCHAINS_DIR", empty.path())
        .env_remove("WASI_SDK_PATH")
        .env_remove("WASI_SDK")
        .env_remove("CC_wasm32_wasip1")
        .env_remove("CFLAGS_wasm32_wasip1")
        .output()
        .expect("check-app");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let report: serde_json::Value =
        serde_json::from_str(stdout.trim()).unwrap_or_else(|_| panic!("one JSON object: {stdout}"));
    assert_eq!(report["stage"], "build", "{report}");
    let fix = report["fix"].as_str().unwrap_or_default();
    assert!(
        fix.starts_with("cbits v0.1.0") && fix.contains("compiles C"),
        "the fix names the crate that compiles C: {fix}"
    );
    assert!(
        fix.contains("krate setup wasi-sdk") && fix.contains("WASI_SDK_PATH"),
        "the fix is the step that gets a compiler: {fix}"
    );
}

/// With a wasi-sdk found (WASI_SDK_PATH, Krate's folder or PATH), the same
/// app builds and its C runs inside Krate. Stands down on a machine with no
/// wasi-sdk, as the GPU tests do on one with no GPU.
#[test]
fn an_app_whose_crate_compiles_c_builds_and_runs_with_wasi_sdk() {
    if !has_cargo_component() {
        eprintln!("skipping: cargo-component is not installed");
        return;
    }
    let doctor = krate().arg("doctor").output().expect("doctor");
    let said = String::from_utf8_lossy(&doctor.stdout).into_owned();
    if !said.contains("wasi-sdk        found") {
        eprintln!("skipping: no wasi-sdk on this machine: {said}");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    c_app(dir.path());
    let out = krate()
        .arg("check-app")
        .arg(dir.path())
        .output()
        .expect("check-app");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success() && text.contains("run passed"),
        "{text}"
    );
    let wasm = dir
        .path()
        .join("target/wasm32-wasip1/release/c_answer.wasm");
    let ran = krate()
        .arg("run")
        .arg(&wasm)
        .arg("--manifest")
        .arg(dir.path().join("manifest.toml"))
        .arg("--auto-grant")
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&ran.stdout).into_owned();
    assert!(
        stdout.contains("the C says 41"),
        "strlen ran inside Krate: {stdout}{}",
        String::from_utf8_lossy(&ran.stderr)
    );
}

/// `krate setup wasi-sdk` installs a release archive into Krate's folder only
/// when it has the SHA-256 it must have, and a build then finds it. Proved
/// with a stand-in archive of the release's shape, so no test downloads one.
#[test]
fn setup_installs_a_verified_wasi_sdk_into_krates_folder_and_refuses_a_bad_one() {
    use sha2::Digest;
    let work = tempfile::tempdir().expect("tempdir");
    let toolchains = work.path().join("toolchains");
    let stem = {
        let arch = match std::env::consts::ARCH {
            "aarch64" => "arm64",
            other => other,
        };
        format!("wasi-sdk-34.0-{arch}-{}", std::env::consts::OS)
    };
    let release = work.path().join("release").join(&stem);
    std::fs::create_dir_all(release.join("bin")).unwrap();
    std::fs::create_dir_all(release.join("share/wasi-sysroot/include")).unwrap();
    let clang = if cfg!(windows) { "clang.exe" } else { "clang" };
    std::fs::write(release.join("bin").join(clang), "stand-in").unwrap();
    let archive = work.path().join(format!("{stem}.tar.gz"));
    let packed = Command::new("tar")
        .arg("-czf")
        .arg(&archive)
        .arg("-C")
        .arg(work.path().join("release"))
        .arg(&stem)
        .status()
        .expect("tar");
    assert!(packed.success());
    let digest = format!(
        "{:x}",
        sha2::Sha256::digest(std::fs::read(&archive).unwrap())
    );
    let setup = |sha: &str| {
        krate()
            .args(["setup", "wasi-sdk", "--from"])
            .arg(&archive)
            .args(["--sha256", sha])
            .env("KRATE_TOOLCHAINS_DIR", &toolchains)
            .output()
            .expect("setup")
    };

    // A digest that does not match: refused, nothing installed.
    let bad = setup(&"0".repeat(64));
    assert!(!bad.status.success());
    let said = String::from_utf8_lossy(&bad.stderr).into_owned();
    assert!(said.contains("failed its checksum"), "{said}");
    assert!(!toolchains.join(&stem).exists(), "nothing was installed");

    // The right one: installed where builds look.
    let good = setup(&digest);
    assert!(
        good.status.success(),
        "{}",
        String::from_utf8_lossy(&good.stderr)
    );
    assert!(toolchains.join(&stem).join("bin").join(clang).is_file());
    let doctor = krate()
        .arg("doctor")
        .env("KRATE_TOOLCHAINS_DIR", &toolchains)
        .env_remove("WASI_SDK_PATH")
        .env_remove("WASI_SDK")
        .output()
        .expect("doctor");
    let line = String::from_utf8_lossy(&doctor.stdout).into_owned();
    assert!(
        line.contains(&format!(
            "wasi-sdk        found ({}",
            toolchains.join(&stem).display()
        )),
        "a build now finds it: {line}"
    );
}

// ---------------------------------------------------------------------------
// The organisation pilot (IC-946), driven through the CLI against a fake hub
// that keeps Plan/org-pilot/CONTRACT.md: its routes, its JSON shapes, its
// stable reasons and its role matrix. The real hub is tested on its own
// side; this proves what the CLI sends and what a person reads.
// ---------------------------------------------------------------------------

/// One stored field of an app's team data: (app id, opaque record name,
/// opaque field name). The fake, like the hub, never sees the real ones.
type FakeDataKey = (String, String, String);

/// A sealed row as the hub keeps it.
#[derive(Clone)]
struct FakeDataRow {
    kind: String,
    ct: String,
    ver: u64,
    s: String,
    by: String,
    size: u64,
    seq: u64,
}

/// A device registered with an organisation's keyring.
#[derive(Clone)]
struct FakeDevice {
    principal: String,
    public_key: String,
    name: String,
    state: String,
}

/// An organisation's keyring as the hub keeps it (F-316): public halves,
/// opaque envelopes, the recovery envelope.
#[derive(Default)]
struct FakeKeys {
    devices: std::collections::BTreeMap<String, FakeDevice>,
    /// (version, public key, state: current | stale | old).
    versions: Vec<(u64, String, String)>,
    /// device id -> (version, envelope, granted by, granting device).
    envelopes: std::collections::BTreeMap<String, (u64, String, String, String)>,
    recovery: Option<serde_json::Value>,
}

/// The signing ring and publishers as the hub keeps them (IC-958,
/// cloud/worker/src/org-signing.js): the ring's public key and one opaque
/// envelope per owner or admin device; each release key that asked to
/// publish, and its permission slip once a holder device signed one.
#[derive(Default)]
struct FakeSigning {
    ring: Option<String>,
    envelopes: std::collections::BTreeMap<String, String>,
    recovery: Option<String>,
    publishers: std::collections::BTreeMap<String, FakePublisher>,
    by_hand: bool,
    two_person: bool,
    /// Releases the sender released alone.
    solo: std::collections::BTreeSet<String>,
}

#[derive(Clone)]
struct FakePublisher {
    principal: String,
    name: String,
    state: String,
    requested_at: u64,
    delegation: Option<serde_json::Value>,
    approved_by: Option<String>,
    since: Option<u64>,
    reason: Option<String>,
}

impl FakeOrg {
    fn may_publish(&self, who: &str) -> bool {
        matches!(self.role_of(who), Some("owner" | "developer"))
    }

    fn holds_ring_role(&self, who: &str) -> bool {
        matches!(self.role_of(who), Some("owner" | "admin"))
    }

    /// Active members other than `except` whose role may approve.
    fn approvers(&self, except: &str) -> Vec<String> {
        self.members
            .iter()
            .filter(|(p, (role, state))| {
                state == "active" && (role == "owner" || role == "reviewer") && p.as_str() != except
            })
            .map(|(p, _)| p.clone())
            .collect()
    }

    fn two_person(&self) -> bool {
        self.signing.two_person || self.approvers("").len() >= 2
    }

    fn solo_open_for(&self, who: &str) -> bool {
        !self.signing.two_person
            && self.role_of(who) == Some("owner")
            && self.approvers(who).is_empty()
    }

    fn ring_current(&self) -> bool {
        self.signing.ring.is_some() && self.signing.ring == self.root
    }

    /// The owners and admins whose devices hold the ring.
    fn ring_holders(&self) -> Vec<String> {
        if !self.ring_current() {
            return Vec::new();
        }
        let mut out: Vec<String> = Vec::new();
        for id in self.signing.envelopes.keys() {
            if let Some(d) = self.keys.devices.get(id) {
                if d.state == "active"
                    && self.holds_ring_role(&d.principal)
                    && !out.contains(&d.principal)
                {
                    out.push(d.principal.clone());
                }
            }
        }
        out
    }

    fn publishing_view(&self, who: &str) -> serde_json::Value {
        serde_json::json!({
            "publishers_by_hand": self.signing.by_hand,
            "two_person_from_start": self.signing.two_person,
            "two_person": self.two_person(),
            "solo_release": self.solo_open_for(who),
            "approvers": self.approvers("").iter().map(|p| p.split_once(':').map_or(p.as_str(), |(_, v)| v).to_string()).collect::<Vec<_>>(),
            "signing": self.ring_current(),
        })
    }

    fn publisher_view(&self, key: &str, p: &FakePublisher) -> serde_json::Value {
        let display = |p: &str| p.split_once(':').map_or(p, |(_, v)| v).to_string();
        serde_json::json!({
            "key": key, "principal": p.principal, "display": display(&p.principal), "name": p.name,
            "state": p.state, "requested_at": p.requested_at, "by_hand": self.signing.by_hand,
            "approved_by": p.approved_by,
            "delegation": if p.state == "signed" { p.delegation.clone() } else { None },
            "can_sign": self.ring_holders().iter().map(|h| display(h)).collect::<Vec<_>>(),
            "closed_reason": p.reason,
        })
    }

    /// What a removal or a lost role does to publishing, applied when next
    /// asked (as `sweep_keys` does for the keyring).
    fn sweep_signing(&mut self, now: u64) {
        let ended: Vec<(String, String, String)> = self
            .signing
            .publishers
            .iter()
            .filter(|(_, p)| matches!(p.state.as_str(), "waiting" | "approved" | "signed"))
            .filter(|(_, p)| !matches!(self.role_of(&p.principal), Some("owner" | "developer")))
            .map(|(k, p)| (k.clone(), p.principal.clone(), p.state.clone()))
            .collect();
        for (key, principal, state) in ended {
            let why = if self.was_removed(&principal) {
                "left the organisation"
            } else {
                "no longer publishes"
            };
            let p = self.signing.publishers.get_mut(&key).unwrap();
            p.reason = Some(why.into());
            if state == "signed" {
                p.state = "revoke-due".into();
                p.since = Some(now);
                self.record(
                    now,
                    "system",
                    "publisher.revoke-due",
                    &format!("release-key:{}", &key[..16]),
                    "allowed",
                    Some(why),
                );
            } else {
                p.state = "closed".into();
                self.record(
                    now,
                    "system",
                    "publisher.close",
                    &format!("release-key:{}", &key[..16]),
                    "allowed",
                    Some(why),
                );
            }
        }
        let gone: Vec<String> = self
            .signing
            .envelopes
            .keys()
            .filter(|id| {
                !self
                    .keys
                    .devices
                    .get(*id)
                    .is_some_and(|d| d.state == "active" && self.holds_ring_role(&d.principal))
            })
            .cloned()
            .collect();
        for id in gone {
            self.signing.envelopes.remove(&id);
            self.record(
                now,
                "system",
                "signing.device.remove",
                &format!("device:{id}"),
                "allowed",
                None,
            );
        }
    }

    /// The caller's device holding the current ring, or the refusal.
    fn holder_device(&self, who: &str, id: &str) -> Result<(), &'static str> {
        if !self.ring_current() {
            return Err("no-signing-key");
        }
        let mine = self
            .keys
            .devices
            .get(id)
            .is_some_and(|d| d.principal == who && d.state == "active");
        if !mine || !self.signing.envelopes.contains_key(id) {
            return Err("signing-key-not-held");
        }
        Ok(())
    }
}

fn wall_clock() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl FakeOrgHub {
    /// /orgs/<slug>/signing..., /publishers..., /settings/publishing, as
    /// cloud/worker/src/org-signing.js answers them.
    fn signing_route(
        &mut self,
        method: &str,
        slug: &str,
        rest: &[&str],
        query: &str,
        who: Option<String>,
        body: &[u8],
    ) -> FakeAnswer {
        let now = self.now;
        let Some(who) = who else {
            return fake_refuse(401, "not-signed-in");
        };
        let Some(org) = self.orgs.get_mut(slug) else {
            return fake_refuse(404, "no-such-org");
        };
        if org.was_removed(&who) && org.role_of(&who).is_none() {
            return fake_refuse(403, "removed");
        }
        let Some(role) = org.role_of(&who).map(str::to_string) else {
            return fake_refuse(403, "not-a-member");
        };
        org.sweep_keys(now);
        org.sweep_signing(now);
        let ask: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
        let holder = role == "owner" || role == "admin";
        let refuse = |org: &mut FakeOrg, action: &str, reason: &str, message: &str| {
            org.record(now, &who, action, "", "denied", Some(reason));
            let status = match reason {
                "slip-invalid" => 422,
                "signing-key-exists" | "needs-approval" | "no-signing-key" => 409,
                "not-a-publisher" | "device-unknown" => 404,
                "bad-request" => 400,
                _ => 403,
            };
            fake_refuse_with(status, reason, message)
        };
        match (method, rest) {
            ("GET", ["settings", "publishing"]) => fake_json(200, org.publishing_view(&who)),
            ("GET", ["signing"]) => {
                let id = query.strip_prefix("device=").unwrap_or("").to_string();
                let mine = org
                    .keys
                    .devices
                    .get(&id)
                    .filter(|d| d.principal == who && d.state == "active")
                    .is_some();
                let envelope = if mine && holder {
                    org.signing.envelopes.get(&id).cloned()
                } else {
                    None
                };
                let current = org.ring_current();
                let holds = envelope.is_some() && current;
                let mut out = serde_json::json!({
                    "ring": org.signing.ring.as_ref().map(|k| serde_json::json!({ "version": 1, "public_key": k, "current": current })),
                    "root_key": org.root,
                    "may_hold": holder,
                    "device": if mine { serde_json::json!({ "device_id": id, "state": "active" }) } else { serde_json::Value::Null },
                    "envelope": envelope.as_ref().map(|b| serde_json::json!({ "version": 1, "blob": b })),
                    "ring_requests": [], "publisher_requests": [], "revoke": [],
                    "recovery": if holder { org.keys.recovery.as_ref().and_then(|r| r.get("public_key").cloned()).map(|k| serde_json::json!({ "public_key": k })).unwrap_or(serde_json::Value::Null) } else { serde_json::Value::Null },
                    "holders": org.ring_holders().iter().map(|p| serde_json::json!({ "principal": p })).collect::<Vec<_>>(),
                    "settings": org.publishing_view(&who),
                });
                if holds {
                    out["ring_requests"] = serde_json::json!(org.keys.devices.iter()
                        .filter(|(id, d)| d.state == "active" && org.holds_ring_role(&d.principal) && !org.signing.envelopes.contains_key(*id))
                        .map(|(id, d)| serde_json::json!({ "device_id": id, "principal": d.principal, "public_key": d.public_key, "name": d.name }))
                        .collect::<Vec<_>>());
                    out["publisher_requests"] = serde_json::json!(org.signing.publishers.iter()
                        .filter(|(_, p)| (p.state == "approved" || (p.state == "waiting" && !org.signing.by_hand)) && org.may_publish(&p.principal))
                        .map(|(k, p)| serde_json::json!({ "key": k, "principal": p.principal, "name": p.name, "requested_at": p.requested_at }))
                        .collect::<Vec<_>>());
                    out["revoke"] = serde_json::json!(org.signing.publishers.iter()
                        .filter(|(_, p)| p.state == "revoke-due")
                        .map(|(k, p)| serde_json::json!({ "key": k, "principal": p.principal, "since": p.since, "reason": p.reason }))
                        .collect::<Vec<_>>());
                }
                fake_json(200, out)
            }
            ("POST", ["signing", "init"]) => {
                if role != "owner" {
                    return refuse(
                        org,
                        "signing.init",
                        "role-cannot",
                        "only an owner makes acme's signing key",
                    );
                }
                if org.signing.ring.is_some() {
                    return refuse(
                        org,
                        "signing.init",
                        "signing-key-exists",
                        "acme already signs its apps",
                    );
                }
                let id = ask["device_id"].as_str().unwrap_or("").to_string();
                if org.keys.devices.get(&id).is_none_or(|d| d.principal != who) {
                    return refuse(
                        org,
                        "signing.init",
                        "device-unknown",
                        "register this device first",
                    );
                }
                let public = ask["public_key"].as_str().unwrap_or("").to_string();
                if org.root.as_ref().is_some_and(|r| *r != public) {
                    return refuse(
                        org,
                        "signing.init",
                        "signing-key-exists",
                        "acme already has a signing key, registered by hand",
                    );
                }
                org.signing.ring = Some(public.clone());
                org.signing.envelopes.insert(
                    id.clone(),
                    ask["envelope"].as_str().unwrap_or("").to_string(),
                );
                org.signing.recovery = ask["recovery_blob"].as_str().map(str::to_string);
                if org.root.is_none() {
                    org.root = Some(public.clone());
                    org.record(
                        now,
                        &who,
                        "root-key.register",
                        &format!("root-key:{}", &public[..16]),
                        "allowed",
                        Some(&format!("made on {id}")),
                    );
                }
                org.record(
                    now,
                    &who,
                    "signing.init",
                    "ring:v1",
                    "allowed",
                    Some(&format!("on {id}")),
                );
                fake_json(
                    201,
                    serde_json::json!({ "version": 1, "public_key": public }),
                )
            }
            ("POST", ["signing", "grants"]) => {
                if !holder {
                    return refuse(
                        org,
                        "signing.grant",
                        "role-cannot",
                        "only owners and admins sign",
                    );
                }
                let id = ask["device_id"].as_str().unwrap_or("").to_string();
                let Some(to) = org
                    .keys
                    .devices
                    .get(&id)
                    .cloned()
                    .filter(|d| d.state == "active" && org.holds_ring_role(&d.principal))
                else {
                    return refuse(
                        org,
                        "signing.grant",
                        "device-unknown",
                        "that device is not an active owner's or admin's",
                    );
                };
                let by = ask["by_device"].as_str().unwrap_or("").to_string();
                if by == "recovery" {
                    if to.principal != who {
                        return refuse(
                            org,
                            "signing.grant",
                            "role-cannot",
                            "the recovery code restores your own device",
                        );
                    }
                } else if let Err(reason) = org.holder_device(&who, &by) {
                    return refuse(org, "signing.grant", reason, "refused");
                }
                org.signing
                    .envelopes
                    .insert(id.clone(), ask["blob"].as_str().unwrap_or("").to_string());
                let to_display = to
                    .principal
                    .split_once(':')
                    .map_or(to.principal.as_str(), |(_, v)| v)
                    .to_string();
                org.record(
                    now,
                    &who,
                    if by == "recovery" {
                        "signing.recover"
                    } else {
                        "signing.grant"
                    },
                    &format!("device:{id}"),
                    "allowed",
                    Some(&format!("v1 to {to_display} by {by}")),
                );
                fake_json(200, serde_json::json!({ "device_id": id, "version": 1 }))
            }
            ("GET", ["signing", "recovery"]) => {
                if !holder {
                    return refuse(
                        org,
                        "signing.recover",
                        "role-cannot",
                        "only owners and admins",
                    );
                }
                let Some(blob) = org.signing.recovery.clone() else {
                    return refuse(
                        org,
                        "signing.recover",
                        "no-signing-key",
                        "no recovery envelope",
                    );
                };
                org.record(
                    now,
                    &who,
                    "signing.recovery.read",
                    &format!("org:{slug}"),
                    "allowed",
                    None,
                );
                fake_json(200, serde_json::json!({ "version": 1, "blob": blob }))
            }
            ("POST", ["signing", "revocations"]) => {
                if !holder {
                    return refuse(
                        org,
                        "signing.revoke",
                        "role-cannot",
                        "only owners and admins sign",
                    );
                }
                let by = ask["by_device"].as_str().unwrap_or("").to_string();
                if let Err(reason) = org.holder_device(&who, &by) {
                    return refuse(org, "signing.revoke", reason, "refused");
                }
                let Ok(list) = serde_json::from_value::<
                    krate_bundle::delegation::SignedRevocationList,
                >(ask["list"].clone()) else {
                    return refuse(
                        org,
                        "signing.revoke",
                        "slip-invalid",
                        "not a withdrawal list",
                    );
                };
                let Ok(list) = list.verify() else {
                    return refuse(
                        org,
                        "signing.revoke",
                        "slip-invalid",
                        "the list's signature does not verify",
                    );
                };
                if Some(&list.root) != org.signing.ring.as_ref() {
                    return refuse(
                        org,
                        "signing.revoke",
                        "slip-invalid",
                        "signed by another key",
                    );
                }
                let mut added = 0;
                for r in &list.revocations {
                    if org.revoked.insert(r.key.clone()) {
                        added += 1;
                        let whose = org.signing.publishers.get(&r.key).map(|p| {
                            p.principal
                                .split_once(':')
                                .map_or(p.principal.clone(), |(_, v)| v.to_string())
                        });
                        org.record(
                            now,
                            &who,
                            "release-key.revoke",
                            &format!("release-key:{}", &r.key[..16]),
                            "allowed",
                            Some(&format!(
                                "automatic, by {by}{}",
                                whose.map(|w| format!(": {w}")).unwrap_or_default()
                            )),
                        );
                    }
                    if let Some(p) = org.signing.publishers.get_mut(&r.key) {
                        p.state = "revoked".into();
                    }
                }
                fake_json(200, serde_json::json!({ "revoked_now": added }))
            }
            ("POST", ["publishers"]) => {
                if !org.may_publish(&who) {
                    return refuse(
                        org,
                        "publisher.request",
                        "role-cannot",
                        &format!("a {role} cannot publish to {slug}"),
                    );
                }
                let key = ask["public_key"]
                    .as_str()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if key.len() != 64 {
                    return refuse(
                        org,
                        "publisher.request",
                        "bad-request",
                        "a release key is 64 hex",
                    );
                }
                if let Some(p) = org.signing.publishers.get(&key).cloned() {
                    if p.principal != who {
                        return refuse(
                            org,
                            "publisher.request",
                            "role-cannot",
                            "that release key belongs to someone else",
                        );
                    }
                    return fake_json(200, org.publisher_view(&key, &p));
                }
                let p = FakePublisher {
                    principal: who.clone(),
                    name: ask["name"].as_str().unwrap_or("").to_string(),
                    state: "waiting".into(),
                    requested_at: now,
                    delegation: None,
                    approved_by: None,
                    since: None,
                    reason: None,
                };
                org.signing.publishers.insert(key.clone(), p.clone());
                org.record(
                    now,
                    &who,
                    "publisher.request",
                    &format!("release-key:{}", &key[..16]),
                    "allowed",
                    Some(&p.name),
                );
                fake_json(201, org.publisher_view(&key, &p))
            }
            ("GET", ["publishers", key]) => {
                let Some(p) = org
                    .signing
                    .publishers
                    .get(*key)
                    .cloned()
                    .filter(|p| p.principal == who || holder)
                else {
                    return fake_refuse(404, "not-a-publisher");
                };
                fake_json(200, org.publisher_view(key, &p))
            }
            ("POST", ["publishers", key, "approve"]) => {
                if !holder {
                    return refuse(
                        org,
                        "publisher.approve",
                        "role-cannot",
                        "only owners and admins approve publishers",
                    );
                }
                let Some(p) = org.signing.publishers.get_mut(*key) else {
                    return refuse(
                        org,
                        "publisher.approve",
                        "not-a-publisher",
                        "no such request",
                    );
                };
                if p.state == "waiting" {
                    p.state = "approved".into();
                    p.approved_by = Some(who.clone());
                }
                let p = p.clone();
                org.record(
                    now,
                    &who,
                    "publisher.approve",
                    &format!("release-key:{}", &key[..16]),
                    "allowed",
                    Some("by hand"),
                );
                fake_json(200, org.publisher_view(key, &p))
            }
            ("POST", ["publishers", key, "delegation"]) => {
                if !holder {
                    return refuse(
                        org,
                        "publisher.delegate",
                        "role-cannot",
                        "only owners and admins sign",
                    );
                }
                let by = ask["by_device"].as_str().unwrap_or("").to_string();
                if let Err(reason) = org.holder_device(&who, &by) {
                    return refuse(org, "publisher.delegate", reason, "refused");
                }
                let Some(p) = org.signing.publishers.get(*key).cloned() else {
                    return refuse(
                        org,
                        "publisher.delegate",
                        "not-a-publisher",
                        "no such request",
                    );
                };
                if !org.may_publish(&p.principal) {
                    return refuse(
                        org,
                        "publisher.delegate",
                        "not-a-member",
                        "they cannot publish now",
                    );
                }
                if p.state == "waiting" && org.signing.by_hand {
                    return refuse(
                        org,
                        "publisher.delegate",
                        "needs-approval",
                        "approve by hand first",
                    );
                }
                let Ok(slip) = serde_json::from_value::<krate_bundle::delegation::SignedDelegation>(
                    ask["delegation"].clone(),
                ) else {
                    return refuse(org, "publisher.delegate", "slip-invalid", "not a slip");
                };
                let d = &slip.delegation;
                let key_bytes: Vec<u8> = (0..32)
                    .filter_map(|i| u8::from_str_radix(&key[i * 2..i * 2 + 2], 16).ok())
                    .collect();
                let verdict = krate_bundle::delegation::verify_chain(
                    &slip,
                    &format!("{slug}/app"),
                    &key_bytes,
                    d.not_before + 1,
                    &krate_bundle::delegation::RevocationState::Known(Vec::new()),
                );
                if !verdict.is_authorised()
                    || Some(&d.root) != org.signing.ring.as_ref()
                    || d.key != *key
                    || d.namespace != format!("{slug}/*")
                    || d.expires <= wall_clock()
                {
                    return refuse(
                        org,
                        "publisher.delegate",
                        "slip-invalid",
                        "that permission does not hold",
                    );
                }
                let display = p
                    .principal
                    .split_once(':')
                    .map_or(p.principal.as_str(), |(_, v)| v)
                    .to_string();
                let p = org.signing.publishers.get_mut(*key).unwrap();
                p.state = "signed".into();
                p.delegation = Some(ask["delegation"].clone());
                let p = p.clone();
                org.record(
                    now,
                    &who,
                    "publisher.delegate",
                    &format!("release-key:{}", &key[..16]),
                    "allowed",
                    Some(&format!("automatic: {display} may publish, signed by {by}")),
                );
                fake_json(200, org.publisher_view(key, &p))
            }
            _ => (404, vec![], b"not found".to_vec()),
        }
    }
}

/// One organisation in the fake hub. Its behaviour follows the real hub's
/// organisation store (cloud/worker/src/org.js): the same reasons, status
/// codes, ordering of checks, and JSON shapes.
#[derive(Default)]
struct FakeOrg {
    name: String,
    /// principal -> (role, state), state "active" or "removed". A removed
    /// member's row stays, so history keeps naming them.
    members: std::collections::BTreeMap<String, (String, String)>,
    invites: std::collections::BTreeMap<String, FakeInvite>,
    root: Option<String>,
    /// Release keys the root withdrew.
    revoked: std::collections::BTreeSet<String>,
    candidates: std::collections::BTreeMap<String, FakeCandidate>,
    audit: Vec<serde_json::Value>,
    /// Groups and their members (IC-949 R).
    groups: std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
    /// The release each app id opens, once one was chosen or approved.
    current: std::collections::BTreeMap<String, String>,
    /// The apps' team data (store.org, IC-951), sealed.
    data: std::collections::BTreeMap<FakeDataKey, FakeDataRow>,
    data_seq: u64,
    /// The keyring (F-316).
    keys: FakeKeys,
    /// The signing ring and publishers (IC-958).
    signing: FakeSigning,
    /// Company connections by name (IC-952).
    connections: std::collections::BTreeMap<String, FakeConnection>,
}

/// One company connection in the fake hub (cloud/worker/src/org-connections.js).
struct FakeConnection {
    base: String,
    methods: Vec<String>,
    paths: Vec<String>,
    /// The groups who may use it; `None` is everyone.
    audience: Option<Vec<String>>,
    /// The app ids that may use it; `None` is any.
    apps: Option<Vec<String>>,
    kind: String,
    header_name: String,
    /// The credential as the admin's machine sealed it to the organisation
    /// key (F-316), and that key's version; 0 is "enter it again".
    sealed: String,
    credential_v: u64,
    set_by: String,
    set_at: u64,
}

/// The company connections a bundle's manifest asks for.
fn fake_connections_asked(bytes: &[u8]) -> Vec<String> {
    krate_bundle::open_bytes(bytes)
        .ok()
        .and_then(|opened| opened.manifest().declared_capabilities().ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|cap| cap.module() == "net" && cap.action() == "connection")
        .filter_map(|cap| cap.resource().map(str::to_string))
        .collect()
}

struct FakeInvite {
    /// `github:<login>` or `email:<address>` (Addendum 1).
    principal: String,
    role: String,
    expires: u64,
    state: &'static str,
}

struct FakeCandidate {
    bytes: Vec<u8>,
    uploader: String,
    uploaded_at: u64,
    name: String,
    version: String,
    signer: String,
    state: &'static str,
    approver: Option<String>,
    approved_at: Option<u64>,
    /// The manifest's app id (IC-949 R: the current release is per app).
    app_id: String,
    /// Why it was withdrawn, once it was (state "withdrawn").
    withdraw_reason: Option<String>,
    /// The groups it is only for; `None` is everyone.
    audience: Option<Vec<String>>,
}

/// A big-file upload in flight (IC-945's path, carrying an organisation).
struct FakeBigUpload {
    org: String,
    who: String,
    parts: std::collections::BTreeMap<usize, Vec<u8>>,
}

#[derive(Default)]
struct FakeOrgHub {
    base: String,
    now: u64,
    orgs: std::collections::BTreeMap<String, FakeOrg>,
    /// Idempotency-Key -> (what the request was, the answer it got). Only
    /// successful answers are remembered, as on the hub.
    keys: std::collections::BTreeMap<String, (String, (u16, Vec<u8>))>,
    keyless_mutations: Vec<String>,
    /// Every request: method, path, the Authorization header if any.
    seen: Vec<(String, String, Option<String>)>,
    /// Every request body, so a test can see what left the machine.
    bodies: Vec<Vec<u8>>,
    big: Option<FakeBigUpload>,
    served: usize,
    next_invite: u32,
    /// The next request is answered with this reason, whatever it was.
    force_next: Option<(u16, String)>,
    /// The request's Accept header: `application/json` asks /a/ what an app
    /// is instead of for its bytes (Addendum 2, item 6).
    accept: String,
    /// The hub's lease key (PKCS#8), the class every lease is issued in,
    /// how far the lease clock is moved from now, and how many were issued
    /// (Addendum 3 G).
    lease_pkcs8: Vec<u8>,
    lease_class: String,
    lease_shift: i64,
    leases_issued: usize,
    /// How many times a connection's credential was handed out, and whether
    /// the credential route answers as if the hub were down (IC-952).
    connection_fetches: usize,
    connections_down: bool,
    /// The per-app data quota (store.org); 0 is the hub's 50 MiB.
    data_quota: u64,
    /// The hub's SITE_BASE: where its join links point. `None` is the
    /// hub's default, krate.tech.
    site: Option<String>,
    /// While true every request answers 503: the hub is out of reach, as
    /// on a train. A private app then opens on its lease, offline.
    down: bool,
    /// While true only the check-in and the lease answer 503, so an app
    /// opens offline on its lease while its data still reaches the hub:
    /// what the store itself refuses, mid-run, can be seen.
    checkin_down: bool,
}

type FakeAnswer = (u16, Vec<(String, String)>, Vec<u8>);

/// The hub's status for each stable reason (org.js STATUS).
fn fake_status(reason: &str) -> u16 {
    match reason {
        "not-signed-in" => 401,
        "last-owner" | "slug-taken" | "idempotency-key-reused" | "bytes-held-elsewhere" => 409,
        "digest-mismatch" | "no-such-org" => 404,
        "not-signed-for-this-org" | "not-marked-for-this-org" | "invalid-bundle" => 422,
        "invite-expired" | "invite-used" | "invite-revoked" | "release-withdrawn" => 410,
        "slug-invalid" | "bad-request" | "reason-required" | "group-invalid" => 400,
        "no-such-group" | "no-such-connection" => 404,
        "group-taken" | "connection-taken" => 409,
        "no-org-key" | "key-rotated" | "credential-needs-entry" => 409,
        "connection-invalid" => 400,
        _ => 403,
    }
}

fn fake_refuse_with(status: u16, reason: &str, message: &str) -> FakeAnswer {
    (
        status,
        vec![("content-type".into(), "application/json".into())],
        serde_json::json!({ "error": reason, "message": message })
            .to_string()
            .into_bytes(),
    )
}

fn fake_refuse(status: u16, reason: &str) -> FakeAnswer {
    fake_refuse_with(status, reason, &format!("refused: {reason}"))
}

fn fake_json(status: u16, value: serde_json::Value) -> FakeAnswer {
    (
        status,
        vec![("content-type".into(), "application/json".into())],
        value.to_string().into_bytes(),
    )
}

/// A principal as the hub's JSON carries it (Addendum 1).
fn fake_principal(principal: &str) -> serde_json::Value {
    let (kind, display) = principal.split_once(':').unwrap_or(("github", principal));
    serde_json::json!({ "principal": principal, "display": display, "kind": kind })
}

/// `%3A` and `%40` back to `:` and `@`: a member is addressed by principal,
/// URL-encoded.
fn fake_decode(segment: &str) -> String {
    let bytes = segment.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&segment[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_default()
}

fn fake_event_hash(event: &serde_json::Value, prev: &str) -> String {
    let mut body = event.clone();
    if let Some(map) = body.as_object_mut() {
        map.remove("hash");
    }
    format!("{:x}", Sha256::digest(format!("{}{prev}", body).as_bytes()))
}

fn fake_chain_ok(events: &[serde_json::Value]) -> bool {
    let mut prev = String::new();
    for event in events {
        if event["prev_hash"].as_str() != Some(prev.as_str()) {
            return false;
        }
        let hash = fake_event_hash(event, &prev);
        if event["hash"].as_str() != Some(hash.as_str()) {
            return false;
        }
        prev = hash;
    }
    true
}

impl FakeOrg {
    /// What a removal does to the keyring, applied when next asked: the
    /// removed member's devices lose their envelopes and the key is stale.
    fn sweep_keys(&mut self, now: u64) {
        let gone: Vec<String> = self
            .keys
            .devices
            .iter()
            .filter(|(_, d)| d.state == "active" && self.role_of(&d.principal).is_none())
            .map(|(id, _)| id.clone())
            .collect();
        for id in &gone {
            if let Some(d) = self.keys.devices.get_mut(id) {
                d.state = "removed".into();
            }
            self.keys.envelopes.remove(id);
            self.record(
                now,
                "system",
                "key.device.remove",
                &format!("device:{id}"),
                "allowed",
                Some("member removed"),
            );
        }
        if !gone.is_empty() {
            if let Some(last) = self.keys.versions.last_mut() {
                if last.2 == "current" {
                    last.2 = "stale".into();
                }
            }
        }
    }

    fn role_of(&self, who: &str) -> Option<&str> {
        self.members
            .get(who)
            .filter(|(_, state)| state == "active")
            .map(|(role, _)| role.as_str())
    }

    fn was_removed(&self, who: &str) -> bool {
        self.members
            .get(who)
            .is_some_and(|(_, state)| state == "removed")
    }

    fn owners(&self) -> usize {
        self.members
            .values()
            .filter(|(role, state)| role == "owner" && state == "active")
            .count()
    }

    fn record(
        &mut self,
        at: u64,
        actor: &str,
        action: &str,
        target: &str,
        result: &str,
        reason: Option<&str>,
    ) {
        let prev = self
            .audit
            .last()
            .and_then(|e| e["hash"].as_str())
            .unwrap_or("")
            .to_string();
        let mut event = serde_json::json!({
            "seq": self.audit.len() + 1,
            "at": at,
            "actor": actor,
            "action": action,
            "target": target,
            "result": result,
            "reason": reason,
            "prev_hash": prev,
        });
        let hash = fake_event_hash(&event, &prev);
        event["hash"] = serde_json::json!(hash);
        self.audit.push(event);
    }
}

/// The candidate judge: why these bytes are not signed for the
/// organisation, or the app's name, version and signing key.
fn fake_signed_for(
    bytes: &[u8],
    root: Option<&str>,
    revoked: &std::collections::BTreeSet<String>,
) -> Result<(String, String, String), String> {
    let opened = krate_bundle::open_bytes(bytes).map_err(|err| format!("{err}"))?;
    let envelope = opened
        .signature_envelope()
        .map_err(|err| format!("{err}"))?
        .ok_or("it is not signed")?;
    let verdict = opened
        .full_verdict(&krate_bundle::delegation::RevocationState::Known(Vec::new()))
        .map_err(|err| format!("{err}"))?
        .ok_or("it is not signed")?;
    let authority = opened
        .signing_authority()
        .map_err(|err| format!("{err}"))?
        .ok_or("it is not signed")?;
    if !verdict.is_trustworthy() {
        return Err("its signature does not verify".into());
    }
    if Some(authority.as_str()) != root {
        return Err(
            "it needs the organisation's root key or a release key that root delegated".into(),
        );
    }
    if revoked.contains(&envelope.public_key) {
        return Err("its release key was withdrawn by the organisation's root".into());
    }
    let app = &opened.manifest().app;
    Ok((app.name.clone(), app.version.clone(), envelope.public_key))
}

/// The manifest's app id.
fn fake_app_id(bytes: &[u8]) -> String {
    krate_bundle::open_bytes(bytes)
        .map(|opened| opened.manifest().app.id.to_ascii_lowercase())
        .unwrap_or_default()
}

impl FakeOrg {
    /// What an app's link opens: the chosen release while it is released,
    /// else the newest approval.
    fn current_of(&self, app: &str) -> Option<String> {
        if let Some(d) = self.current.get(app) {
            if self
                .candidates
                .get(d)
                .is_some_and(|c| c.state == "released")
            {
                return Some(d.clone());
            }
        }
        self.candidates
            .iter()
            .filter(|(_, c)| c.state == "released" && c.app_id == app)
            .max_by_key(|(_, c)| c.approved_at)
            .map(|(d, _)| d.clone())
    }

    /// Whether `who` (with `role`) may open a release with this audience:
    /// owners, admins and reviewers always may.
    fn in_audience(&self, who: &str, role: &str, audience: &Option<Vec<String>>) -> bool {
        match audience {
            None => true,
            Some(_) if matches!(role, "owner" | "admin" | "reviewer") => true,
            Some(groups) => groups
                .iter()
                .any(|g| self.groups.get(g).is_some_and(|m| m.contains(who))),
        }
    }
}

/// Addendum 5: a candidate carries a mark naming its organisation, under
/// the signature `fake_signed_for` has already found to hold.
fn fake_marked_for(bytes: &[u8], slug: &str) -> bool {
    krate_bundle::open_bytes(bytes)
        .ok()
        .and_then(|opened| opened.org_mark().ok().flatten())
        .as_deref()
        == Some(slug)
}

impl FakeOrgHub {
    /// The organisation keyring routes, as cloud/worker/src/org-keys.js
    /// answers them: devices under principals, one envelope per device
    /// (opaque here), grants by holders only, stale after a removal.
    fn keys_route(
        &mut self,
        method: &str,
        slug: &str,
        rest: &[&str],
        query: &str,
        who: Option<String>,
        body: &[u8],
    ) -> FakeAnswer {
        let now = self.now;
        let Some(who) = who else {
            return fake_refuse(401, "not-signed-in");
        };
        let Some(org) = self.orgs.get_mut(slug) else {
            return fake_refuse(404, "no-such-org");
        };
        if org.was_removed(&who) {
            return fake_refuse(403, "removed");
        }
        let Some(role) = org.role_of(&who).map(str::to_string) else {
            return fake_refuse(403, "not-a-member");
        };
        org.sweep_keys(now);
        let ask: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
        let current = org.keys.versions.last().cloned();
        let mine = |org: &FakeOrg, id: &str| {
            org.keys
                .devices
                .get(id)
                .filter(|d| d.principal == who && d.state == "active")
                .cloned()
        };
        match (method, rest) {
            ("GET", []) => {
                let id = query.strip_prefix("device=").unwrap_or("");
                let me = mine(org, id);
                let envelope = me
                    .as_ref()
                    .and_then(|_| org.keys.envelopes.get(id).cloned());
                let holds = matches!((&current, &envelope), (Some(c), Some(e)) if c.0 == e.0);
                let live: Vec<(String, FakeDevice)> = org
                    .keys
                    .devices
                    .iter()
                    .filter(|(_, d)| d.state == "active" && org.role_of(&d.principal).is_some())
                    .map(|(id, d)| (id.clone(), d.clone()))
                    .collect();
                let mut out = serde_json::json!({
                    "current": current.as_ref().map(|c| serde_json::json!({ "version": c.0, "public_key": c.1, "state": c.2 })),
                    "device": me.as_ref().map(|_| serde_json::json!({ "device_id": id, "state": "active" })),
                    "envelope": envelope.as_ref().map(|e| serde_json::json!({ "version": e.0, "blob": e.1, "granted_by": e.2, "granted_device": e.3 })),
                    "requests": [], "seal_to": [], "recovery": null,
                    "devices": org.keys.devices.iter().filter(|(_, d)| role == "owner" || role == "admin" || d.principal == who).map(|(id, d)| serde_json::json!({
                        "device_id": id, "principal": d.principal,
                        "display": d.principal.split_once(':').map_or(d.principal.as_str(), |(_, v)| v),
                        "name": d.name, "state": d.state,
                        "holds": org.keys.envelopes.get(id).map(|e| e.0),
                    })).collect::<Vec<_>>(),
                });
                if holds {
                    let c = current.clone().unwrap();
                    out["requests"] = serde_json::json!(live
                        .iter()
                        .filter(|(id, _)| org.keys.envelopes.get(id).is_none_or(|e| e.0 != c.0))
                        .map(|(id, d)| serde_json::json!({ "device_id": id, "principal": d.principal, "public_key": d.public_key, "name": d.name }))
                        .collect::<Vec<_>>());
                    if c.2 == "stale" {
                        out["seal_to"] = serde_json::json!(live
                            .iter()
                            .map(|(id, d)| serde_json::json!({ "device_id": id, "principal": d.principal, "public_key": d.public_key }))
                            .collect::<Vec<_>>());
                        // Every sealed credential, to seal again (org-connections.js).
                        out["credentials"] = serde_json::json!(org
                            .connections
                            .iter()
                            .filter(|(_, x)| x.credential_v >= 1)
                            .map(|(name, x)| serde_json::json!({ "name": name, "base": x.base, "kind": x.kind, "header": x.header_name, "v": x.credential_v, "sealed": x.sealed }))
                            .collect::<Vec<_>>());
                    }
                    if let Some(r) = &org.keys.recovery {
                        out["recovery"] = serde_json::json!({ "version": r["version"], "public_key": r["public_key"], "params": r["params"] });
                    }
                }
                fake_json(200, out)
            }
            ("PUT", ["devices", id]) => {
                let public = ask["public_key"].as_str().unwrap_or("").to_string();
                let expected = format!("{:x}", Sha256::digest(public.as_bytes()));
                if public.len() != 64 || expected[..16] != **id {
                    return fake_refuse(400, "bad-request");
                }
                if let Some(d) = org.keys.devices.get(*id) {
                    if d.principal != who {
                        return fake_refuse(403, "role-cannot");
                    }
                }
                let name = ask["name"].as_str().unwrap_or("").to_string();
                if org
                    .keys
                    .devices
                    .get(*id)
                    .is_none_or(|d| d.state != "active")
                {
                    org.keys.devices.insert(
                        id.to_string(),
                        FakeDevice {
                            principal: who.clone(),
                            public_key: public,
                            name,
                            state: "active".into(),
                        },
                    );
                    org.record(
                        now,
                        &who,
                        "key.device.add",
                        &format!("device:{id}"),
                        "allowed",
                        None,
                    );
                }
                fake_json(
                    200,
                    serde_json::json!({ "device_id": id, "state": "active" }),
                )
            }
            ("POST", ["init"]) => {
                if role != "owner" {
                    return fake_refuse(403, "role-cannot");
                }
                if current.is_some() {
                    return fake_refuse(409, "key-exists");
                }
                let id = ask["device_id"].as_str().unwrap_or("").to_string();
                if mine(org, &id).is_none() {
                    return fake_refuse(404, "device-unknown");
                }
                org.keys.versions.push((
                    1,
                    ask["public_key"].as_str().unwrap_or("").to_string(),
                    "current".into(),
                ));
                org.keys.envelopes.insert(
                    id.clone(),
                    (
                        1,
                        ask["envelope"].as_str().unwrap_or("").to_string(),
                        who.clone(),
                        id.clone(),
                    ),
                );
                let mut recovery = ask["recovery"].clone();
                recovery["version"] = serde_json::json!(1);
                org.keys.recovery = Some(recovery);
                org.record(
                    now,
                    &who,
                    "key.init",
                    "key:v1",
                    "allowed",
                    Some(&format!("on {id}")),
                );
                fake_json(201, serde_json::json!({ "version": 1 }))
            }
            ("GET", ["recovery"]) => {
                if role != "owner" && role != "admin" {
                    return fake_refuse(403, "role-cannot");
                }
                let Some(r) = org.keys.recovery.clone() else {
                    return fake_refuse(409, "no-org-key");
                };
                org.record(
                    now,
                    &who,
                    "key.recovery.read",
                    &format!("org:{slug}"),
                    "allowed",
                    None,
                );
                fake_json(200, r)
            }
            ("POST", ["grants"]) => {
                let Some(c) = current else {
                    return fake_refuse(409, "no-org-key");
                };
                if ask["version"].as_u64() != Some(c.0) {
                    return fake_refuse(409, "key-rotated");
                }
                let id = ask["device_id"].as_str().unwrap_or("").to_string();
                let Some(to) = org.keys.devices.get(&id).cloned() else {
                    return fake_refuse(404, "device-unknown");
                };
                if to.state != "active" || org.role_of(&to.principal).is_none() {
                    return fake_refuse(404, "device-unknown");
                }
                let by = ask["by_device"].as_str().unwrap_or("").to_string();
                let (action, how) = if by == "recovery" {
                    if (role != "owner" && role != "admin") || to.principal != who {
                        return fake_refuse(403, "role-cannot");
                    }
                    ("key.recover", "recovery code".to_string())
                } else {
                    let holds = mine(org, &by).is_some()
                        && org.keys.envelopes.get(&by).is_some_and(|e| e.0 == c.0);
                    if !holds {
                        return fake_refuse(403, "key-not-held");
                    }
                    ("key.grant", by.clone())
                };
                org.keys.envelopes.insert(
                    id.clone(),
                    (
                        c.0,
                        ask["blob"].as_str().unwrap_or("").to_string(),
                        who.clone(),
                        how.clone(),
                    ),
                );
                org.record(
                    now,
                    &who,
                    action,
                    &format!("device:{id}"),
                    "allowed",
                    Some(&format!("v{} to {} by {how}", c.0, to.principal)),
                );
                fake_json(200, serde_json::json!({ "device_id": id, "version": c.0 }))
            }
            ("POST", ["rotate"]) => {
                let Some(c) = current else {
                    return fake_refuse(409, "no-org-key");
                };
                if c.2 != "stale" {
                    return fake_refuse(409, "key-rotated");
                }
                let by = ask["by_device"].as_str().unwrap_or("").to_string();
                if mine(org, &by).is_none()
                    || org.keys.envelopes.get(&by).is_none_or(|e| e.0 != c.0)
                {
                    return fake_refuse(403, "key-not-held");
                }
                let next = c.0 + 1;
                if ask["version"].as_u64() != Some(next) {
                    return fake_refuse(400, "bad-request");
                }
                let holders: Vec<String> = org.keys.envelopes.keys().cloned().collect();
                if holders
                    .iter()
                    .any(|id| ask["envelopes"][id].as_str().is_none())
                {
                    return fake_refuse(400, "bad-request");
                }
                // Every credential comes back sealed to the new version, or
                // marked as one that did not open there.
                let back = |x: &serde_json::Value| {
                    x["unopened"] == true
                        || (x["v"].as_u64() == Some(next) && x["sealed"].as_str().is_some())
                };
                if org
                    .connections
                    .iter()
                    .filter(|(_, x)| x.credential_v >= 1)
                    .any(|(name, _)| !back(&ask["credentials"][name]))
                {
                    return fake_refuse(400, "bad-request");
                }
                for (name, x) in org.connections.iter_mut() {
                    if x.credential_v < 1 {
                        continue;
                    }
                    let given = &ask["credentials"][name];
                    match given["sealed"].as_str() {
                        Some(sealed) if given["unopened"] != true => {
                            x.sealed = sealed.to_string();
                            x.credential_v = next;
                        }
                        _ => {
                            x.sealed.clear();
                            x.credential_v = 0;
                        }
                    }
                }
                if let Some(last) = org.keys.versions.last_mut() {
                    last.2 = "old".into();
                }
                org.keys.versions.push((
                    next,
                    ask["public_key"].as_str().unwrap_or("").to_string(),
                    "current".into(),
                ));
                let live: Vec<String> = org
                    .keys
                    .devices
                    .iter()
                    .filter(|(_, d)| d.state == "active" && org.role_of(&d.principal).is_some())
                    .map(|(id, _)| id.clone())
                    .collect();
                org.keys.envelopes.clear();
                for id in live {
                    if let Some(blob) = ask["envelopes"][&id].as_str() {
                        org.keys
                            .envelopes
                            .insert(id, (next, blob.to_string(), who.clone(), by.clone()));
                    }
                }
                if let Some(r) = org.keys.recovery.as_mut() {
                    r["blob"] = ask["recovery_blob"].clone();
                    r["version"] = serde_json::json!(next);
                }
                org.record(
                    now,
                    &who,
                    "key.rotate",
                    &format!("key:v{next}"),
                    "allowed",
                    Some(&format!("by {by}")),
                );
                fake_json(200, serde_json::json!({ "version": next }))
            }
            _ => (404, vec![], b"not found".to_vec()),
        }
    }

    /// `POST /orgs/<slug>/data/<digest>/sync`, as cloud/worker/src/org-data.js
    /// answers it: standing, a release of this organisation, not withdrawn,
    /// in its audience, writes under the current key version; then the
    /// sealed rows merge per opaque field by the shared rule (the device's
    /// own `org_merge`, which the hub's JS matches vector for vector), and
    /// what changed since the cursor comes back. Nothing here can open a
    /// row: it holds what the device sent.
    fn data_sync(
        &mut self,
        slug: &str,
        digest: &str,
        who: Option<String>,
        body: &[u8],
    ) -> FakeAnswer {
        use krate_runtime::org_merge::{self, Stamp};
        let hub_now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let quota = if self.data_quota == 0 {
            50 * 1024 * 1024
        } else {
            self.data_quota
        };
        let now = self.now;
        let Some(who) = who else {
            return fake_refuse(401, "not-signed-in");
        };
        let Some(org) = self.orgs.get_mut(slug) else {
            return fake_refuse(404, "no-such-org");
        };
        org.sweep_keys(now);
        let target = format!("release:{digest}");
        let deny = |org: &mut FakeOrg, reason: &str| {
            org.record(now, &who, "data.sync", &target, "denied", Some(reason));
            let status = match reason {
                "release-withdrawn" => 410,
                "too-large" | "over-quota" => 413,
                "key-rotated" | "no-org-key" => 409,
                _ => 403,
            };
            fake_refuse(status, reason)
        };
        if org.was_removed(&who) {
            return deny(org, "removed");
        }
        let Some(role) = org.role_of(&who).map(str::to_string) else {
            return deny(org, "not-a-member");
        };
        let Some((state, audience, app)) = org
            .candidates
            .get(digest)
            .map(|c| (c.state, c.audience.clone(), c.app_id.to_ascii_lowercase()))
        else {
            return deny(org, "not-released");
        };
        if state == "withdrawn" {
            return deny(org, "release-withdrawn");
        }
        if state != "released" {
            return deny(org, "not-released");
        }
        if !org.in_audience(&who, &role, &audience) {
            return deny(org, "not-in-audience");
        }
        let ask: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
        let writes = ask["writes"].as_array().cloned().unwrap_or_default();
        let key_version = org.keys.versions.last().map(|v| v.0);
        if !writes.is_empty() {
            let Some(current) = key_version else {
                return deny(org, "no-org-key");
            };
            if writes.iter().any(|w| w["ver"].as_u64() != Some(current)) {
                return deny(org, "key-rotated");
            }
        }
        let device_now = ask["now"].as_u64().unwrap_or(hub_now);
        let since: u64 = ask["since"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let ct_bytes = |ct: &str| krate_orgkeys::b64_decode(ct).map(|b| b.len());
        let mut data = org.data.clone();
        let mut seq = org.data_seq;
        let mut lost = Vec::new();
        let mut accepted = 0;
        for w in &writes {
            let (Some(r), Some(k), Some(kind), Some(s), Some(ct)) = (
                w["r"].as_str(),
                w["k"].as_str(),
                w["kind"].as_str(),
                w["s"].as_str().and_then(Stamp::parse),
                w["ct"].as_str(),
            ) else {
                return fake_refuse(400, "bad-request");
            };
            let Some(n) = ct_bytes(ct).filter(|n| *n >= 40) else {
                return fake_refuse(400, "bad-request");
            };
            let s = org_merge::rebase(&s, device_now, hub_now);
            let key = (app.clone(), r.to_string(), k.to_string());
            let tomb = data
                .iter()
                .filter(|(kk, row)| kk.0 == app && kk.1 == r && row.kind == "t")
                .filter_map(|(_, row)| Stamp::parse(&row.s))
                .max();
            let held = data.get(&key).and_then(|row| Stamp::parse(&row.s));
            let dead = kind != "t" && !org_merge::alive(&s, tomb.as_ref());
            if dead || !org_merge::wins(&s, held.as_ref()) {
                lost.push((r.to_string(), if dead { None } else { Some(k.to_string()) }));
                continue;
            }
            accepted += 1;
            seq += 1;
            data.insert(
                key,
                FakeDataRow {
                    kind: kind.to_string(),
                    ct: ct.to_string(),
                    ver: w["ver"].as_u64().unwrap_or(0),
                    s: s.to_string(),
                    by: who.clone(),
                    size: n as u64 + 160,
                    seq,
                },
            );
            if kind == "t" {
                data.retain(|kk, row| {
                    !(kk.0 == app && kk.1 == r && row.kind != "t")
                        || Stamp::parse(&row.s).is_some_and(|x| x > s)
                });
            }
        }
        // The sealed record bound, and the quota, refuse the whole push.
        let mut records: std::collections::BTreeMap<String, (u64, u64)> = Default::default();
        let mut used = 0u64;
        for (kk, row) in &data {
            if kk.0 != app {
                continue;
            }
            used += row.size;
            if row.kind == "v" {
                let e = records.entry(kk.1.clone()).or_default();
                e.0 += row.size - 160;
                e.1 += 1;
            }
        }
        if records
            .values()
            .any(|(bytes, fields)| *bytes > 64 * 1024 + 512 * fields)
        {
            return deny(org, "too-large");
        }
        if accepted > 0 && used > quota {
            return deny(org, "over-quota");
        }
        org.data = data;
        org.data_seq = seq;
        let view = |k: &FakeDataKey, row: &FakeDataRow| {
            let by = row.by.split_once(':').map_or(row.by.as_str(), |(_, v)| v);
            serde_json::json!({ "r": k.1, "k": k.2, "kind": row.kind, "s": row.s, "ver": row.ver, "ct": row.ct, "by": by })
        };
        let mut changed: Vec<(u64, serde_json::Value)> = org
            .data
            .iter()
            .filter(|(k, row)| k.0 == app && row.seq > since)
            .map(|(k, row)| (row.seq, view(k, row)))
            .collect();
        changed.sort_by_key(|r| r.0);
        let cursor = changed.last().map(|r| r.0).unwrap_or(since);
        let mut rows: Vec<serde_json::Value> = changed.into_iter().map(|r| r.1).collect();
        for (r, k) in lost {
            for (kk, row) in org.data.iter().filter(|(kk, row)| {
                kk.0 == app && kk.1 == r && k.as_ref().map_or(row.kind == "t", |k| &kk.2 == k)
            }) {
                rows.push(view(kk, row));
            }
        }
        let display = who
            .split_once(':')
            .map_or(who.as_str(), |(_, v)| v)
            .to_string();
        fake_json(
            200,
            serde_json::json!({
                "v": 1, "now": hub_now, "accepted": accepted, "key_version": key_version, "rows": rows,
                "cursor": cursor.to_string(), "more": false, "full": false,
                "me": { "name": display, "role": role },
                "org": { "slug": slug, "name": org.name },
                "quota": { "used": used, "limit": quota },
            }),
        )
    }

    fn handle(
        &mut self,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> FakeAnswer {
        let header = |name: &str| {
            headers
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        };
        let auth = header("authorization");
        self.accept = header("accept").unwrap_or_default();
        self.seen
            .push((method.to_string(), path.to_string(), auth.clone()));
        self.bodies.push(body.to_vec());
        // tok-<login> is a GitHub login the hub vouched for; tok-<address>
        // an inbox proven by an email or Google sign-in (Addendum 1).
        let who = auth
            .as_deref()
            .and_then(|a| a.strip_prefix("Bearer tok-"))
            .map(|name| {
                let name = name.to_ascii_lowercase();
                if name.contains('@') {
                    format!("email:{name}")
                } else {
                    format!("github:{name}")
                }
            });
        if let Some((status, reason)) = self.force_next.take() {
            return fake_refuse(status, &reason);
        }
        if self.down
            || (self.checkin_down && (path.starts_with("/a/") || path.contains("/leases/")))
        {
            return fake_refuse(503, "unavailable");
        }
        // The apps' team data (IC-951): one POST per sync, which takes no
        // Idempotency-Key -- a stamped write applied twice is applied once.
        {
            let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
            if method == "POST"
                && parts.len() == 5
                && parts[0] == "orgs"
                && parts[2] == "data"
                && parts[4] == "sync"
            {
                let (slug, digest) = (parts[1].to_string(), parts[3].to_string());
                return self.data_sync(&slug, &digest, who, body);
            }
        }
        // The keyring (F-316).
        {
            let (bare, query) = path.split_once('?').unwrap_or((path, ""));
            let parts: Vec<String> = bare
                .trim_start_matches('/')
                .split('/')
                .map(str::to_string)
                .collect();
            if parts.len() >= 3 && parts[0] == "orgs" && parts[2] == "keys" {
                let rest: Vec<&str> = parts[3..].iter().map(String::as_str).collect();
                let slug = parts[1].clone();
                return self.keys_route(method, &slug, &rest, query, who, body);
            }
            // The signing ring and publishers (IC-958).
            if parts.len() >= 3
                && parts[0] == "orgs"
                && (parts[2] == "signing"
                    || parts[2] == "publishers"
                    || (parts[2] == "settings"
                        && parts.get(3).map(String::as_str) == Some("publishing")))
            {
                let rest: Vec<&str> = parts[2..].iter().map(String::as_str).collect();
                let slug = parts[1].clone();
                return self.signing_route(method, &slug, &rest, query, who, body);
            }
        }
        let mutation = method != "GET";
        if mutation && path.starts_with("/orgs") {
            let Some(key) = header("idempotency-key") else {
                self.keyless_mutations.push(format!("{method} {path}"));
                return self.route(method, path, who, body);
            };
            let fingerprint = format!("{method} {path} {:x}", Sha256::digest(body));
            if let Some((was, (status, answer))) = self.keys.get(&key) {
                if *was != fingerprint {
                    return fake_refuse(409, "idempotency-key-reused");
                }
                return (*status, vec![], answer.clone());
            }
            let answer = self.route(method, path, who, body);
            if answer.0 < 300 {
                self.keys
                    .insert(key, (fingerprint, (answer.0, answer.2.clone())));
            }
            return answer;
        }
        self.route(method, path, who, body)
    }

    fn route(&mut self, method: &str, path: &str, who: Option<String>, body: &[u8]) -> FakeAnswer {
        let now = self.now;
        let base = self.base.clone();
        let decoded: Vec<String> = path
            .trim_start_matches('/')
            .split('/')
            .map(fake_decode)
            .collect();
        let parts: Vec<&str> = decoded.iter().map(String::as_str).collect();

        // The organisation's link for an app (IC-949 R): what it opens now.
        // Its bytes come through every check /a/<digest> makes.
        if method == "GET"
            && parts.len() == 5
            && parts[0] == "orgs"
            && parts[2] == "apps"
            && parts[4] == "current"
            && !self.accept.contains("application/json")
        {
            let current = self
                .orgs
                .get(parts[1])
                .and_then(|org| org.current_of(&parts[3].to_ascii_lowercase()));
            if let Some(digest) = current {
                return self.route("GET", &format!("/a/{digest}"), who, body);
            }
        }

        // A private release: the organisation is read on every fetch, and
        // every refusal is a 403 (a withdrawn one 410).
        if (method == "GET" || method == "HEAD") && parts.len() == 2 && parts[0] == "a" {
            let digest = parts[1];
            let look = self.accept.contains("application/json");
            let Some((slug, org)) = self
                .orgs
                .iter_mut()
                .find(|(_, org)| org.candidates.contains_key(digest))
            else {
                return (404, vec![], b"not found".to_vec());
            };
            let slug = slug.clone();
            let target = format!("release:{digest}");
            let Some(who) = who else {
                // Refused before the store is asked: not audited.
                return fake_refuse(403, "not-signed-in");
            };
            let reason = if org.was_removed(&who) {
                Some("removed")
            } else if org.role_of(&who).is_none() {
                Some("not-a-member")
            } else if org.candidates[digest].state == "withdrawn" {
                Some("release-withdrawn")
            } else if org.candidates[digest].state != "released" {
                Some("digest-mismatch")
            } else if !org.in_audience(
                &who,
                org.role_of(&who).unwrap_or(""),
                &org.candidates[digest].audience,
            ) {
                Some("not-in-audience")
            } else {
                None
            };
            if let Some(reason) = reason {
                org.record(
                    now,
                    &who,
                    "release.download",
                    &target,
                    "denied",
                    Some(reason),
                );
                // A removed member already knows which organisation sent it.
                let message =
                    match reason {
                        "release-withdrawn" => format!(
                        "{} withdrew {} {}: \"{}\" -- ask an admin of {} which version to use now",
                        org.name,
                        org.candidates[digest].name,
                        org.candidates[digest].version,
                        org.candidates[digest].withdraw_reason.clone().unwrap_or_default(),
                        org.name
                    ),
                        "not-in-audience" => format!(
                        "this app is only for the {} group in {} -- ask an admin of {} to add you",
                        org.candidates[digest]
                            .audience
                            .clone()
                            .unwrap_or_default()
                            .join(", "),
                        org.name,
                        org.name
                    ),
                        _ => "refused".to_string(),
                    };
                let mut refusal = serde_json::json!({ "error": reason, "message": message });
                if reason == "removed" {
                    refusal["org"] = serde_json::json!(slug);
                }
                let status = if reason == "release-withdrawn" {
                    410
                } else {
                    403
                };
                return (
                    status,
                    vec![("content-type".into(), "application/json".into())],
                    refusal.to_string().into_bytes(),
                );
            }
            let candidate = &org.candidates[digest];
            if look {
                return fake_json(
                    200,
                    serde_json::json!({
                        "name": candidate.name, "version": candidate.version,
                        "size": candidate.bytes.len(),
                        "org": { "slug": slug, "name": org.name },
                    }),
                );
            }
            let bytes = candidate.bytes.clone();
            org.record(now, &who, "release.download", &target, "allowed", None);
            self.served += 1;
            return (200, vec![], bytes);
        }

        // The big-file path, carrying the organisation (IC-945 + IC-946).
        if parts.first() == Some(&"publish") && parts.get(1) == Some(&"big") {
            return self.big_path(method, &parts, who, body);
        }

        if parts.first() != Some(&"orgs") {
            return (404, vec![], b"not found".to_vec());
        }
        let json: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();

        // GET /orgs: the signed-in person's organisations (Addendum 1).
        if method == "GET" && parts.len() == 1 {
            let Some(who) = who else {
                return fake_refuse(401, "not-signed-in");
            };
            let mine: Vec<serde_json::Value> = self
                .orgs
                .iter()
                .filter_map(|(slug, org)| {
                    org.role_of(&who).map(
                        |role| serde_json::json!({ "slug": slug, "name": org.name, "role": role }),
                    )
                })
                .collect();
            return fake_json(200, serde_json::json!(mine));
        }
        if method == "POST" && parts.len() == 1 {
            let slug = json["slug"].as_str().unwrap_or("").to_string();
            let valid = slug.len() >= 2
                && slug.len() <= 39
                && slug.as_bytes()[0].is_ascii_lowercase()
                && slug
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
            if !valid {
                return fake_refuse(400, "slug-invalid");
            }
            let Some(who) = who else {
                return fake_refuse(401, "not-signed-in");
            };
            if self.orgs.contains_key(&slug) {
                return fake_refuse(409, "slug-taken");
            }
            let mut org = FakeOrg {
                name: json["name"].as_str().unwrap_or(&slug).to_string(),
                ..Default::default()
            };
            org.members
                .insert(who.clone(), ("owner".into(), "active".into()));
            org.record(
                now,
                &who,
                "org.create",
                &format!("org:{slug}"),
                "allowed",
                None,
            );
            org.record(
                now,
                &who,
                "member.add",
                &format!("member:{who}"),
                "allowed",
                Some("owner"),
            );
            let name = org.name.clone();
            self.orgs.insert(slug.clone(), org);
            return fake_json(
                201,
                serde_json::json!({ "org": { "slug": slug, "name": name, "root_key": null } }),
            );
        }

        let slug = parts.get(1).copied().unwrap_or("").to_string();
        let Some(who) = who else {
            return fake_refuse(401, "not-signed-in");
        };
        let Some(org) = self.orgs.get_mut(&slug) else {
            return fake_refuse(404, "no-such-org");
        };
        let action = match (method, parts.get(2).copied(), parts.len()) {
            ("GET", None, 2) => "org.view",
            ("POST", Some("invites"), 3) => "invite.create",
            ("DELETE", Some("invites"), 4) => "invite.revoke",
            ("POST", Some("invites"), 5) => "invite.accept",
            ("PUT", Some("members"), 4) => "member.role",
            ("DELETE", Some("members"), 4) => "member.remove",
            ("PUT", Some("root-key"), 3) => "root-key.set",
            ("POST", Some("candidates"), 3) => "candidate.publish",
            ("GET", Some("candidates"), 3) => "candidate.list",
            ("POST", Some("candidates"), 5) => "candidate.approve",
            ("GET", Some("releases"), 3) => "release.list",
            ("GET", Some("audit"), 3) => "audit.read",
            ("GET", Some("leases"), 4) => "lease.issue",
            ("POST", Some("releases"), 5) if parts[4] == "withdraw" => "release.withdraw",
            ("PUT", Some("releases"), 5) if parts[4] == "audience" => "release.audience",
            ("PUT", Some("apps"), 5) if parts[4] == "current" => "release.set-current",
            ("GET", Some("apps"), 5) if parts[4] == "current" => "release.current-of",
            ("GET", Some("apps"), 3) => "release.apps",
            ("GET", Some("groups"), 3) => "group.list",
            ("POST", Some("groups"), 3) => "group.manage",
            ("PUT", Some("groups"), 6) => "group.add",
            ("DELETE", Some("groups"), 6) => "group.remove",
            ("GET", Some("connections"), 3) => "connection.view",
            ("POST", Some("connections"), 3) => "connection.manage",
            ("PUT", Some("connections"), 5) if parts[4] == "credential" => "connection.manage",
            ("DELETE", Some("connections"), 4) => "connection.manage",
            ("GET", Some("connections"), 6) if parts[4] == "credential" => "connection.use",
            _ => return (404, vec![], b"not found".to_vec()),
        };
        let target = match action {
            "member.role" | "member.remove" => format!("member:{}", parts[3]),
            "candidate.approve" => format!("candidate:{}", parts[3]),
            "release.withdraw" | "release.audience" => format!("release:{}", parts[3]),
            "release.set-current" | "release.current-of" => format!("app:{}", parts[3]),
            "group.manage" => format!("group:{}", json["name"].as_str().unwrap_or("")),
            "group.add" | "group.remove" => format!("group:{}/{}", parts[3], parts[5]),
            "connection.manage" | "connection.use" => format!(
                "connection:{}",
                parts
                    .get(3)
                    .copied()
                    .or(json["name"].as_str())
                    .unwrap_or("")
            ),
            "invite.create" => format!(
                "invite:{}/{}",
                json["github"]
                    .as_str()
                    .map(|g| format!("github:{g}"))
                    .or_else(|| json["email"].as_str().map(|e| format!("email:{e}")))
                    .unwrap_or_default(),
                json["role"].as_str().unwrap_or("")
            ),
            _ => format!("org:{slug}"),
        };
        let deny_with = |org: &mut FakeOrg, reason: &str, message: &str| {
            org.record(now, &who, action, &target, "denied", Some(reason));
            fake_refuse_with(fake_status(reason), reason, message)
        };
        let deny =
            |org: &mut FakeOrg, reason: &str| deny_with(org, reason, &format!("refused: {reason}"));

        // Accepting is the one route a non-member may take.
        if action == "invite.accept" {
            let code = parts[3];
            let Some(invite) = org.invites.get(code) else {
                return deny(org, "invite-revoked");
            };
            let refusal = if invite.principal != who {
                Some("invite-for-someone-else")
            } else if invite.state == "revoked" {
                Some("invite-revoked")
            } else if invite.state == "used" {
                Some("invite-used")
            } else if now >= invite.expires {
                Some("invite-expired")
            } else {
                None
            };
            if let Some(reason) = refusal {
                return deny(org, reason);
            }
            let invite = org.invites.get_mut(code).unwrap();
            invite.state = "used";
            let role = invite.role.clone();
            let before = org.two_person();
            org.members
                .insert(who.clone(), (role.clone(), "active".into()));
            org.record(now, &who, "invite.accept", code, "allowed", None);
            if !before && org.two_person() {
                let display = who
                    .split_once(':')
                    .map_or(who.as_str(), |(_, v)| v)
                    .to_string();
                org.record(
                    now,
                    &who,
                    "review.two-person.on",
                    &format!("org:{slug}"),
                    "allowed",
                    Some(&format!("{display} can approve now")),
                );
            }
            let mut membership = fake_principal(&who);
            membership["org"] = serde_json::json!(slug);
            membership["role"] = serde_json::json!(role);
            membership["since"] = serde_json::json!(now);
            return fake_json(200, serde_json::json!({ "membership": membership }));
        }

        let Some(role) = org.role_of(&who).map(str::to_string) else {
            if org.was_removed(&who) {
                return deny(org, "removed");
            }
            return deny(org, "not-a-member");
        };
        let is = |names: &[&str]| names.contains(&role.as_str());
        let high = |r: &str| r == "owner" || r == "admin";
        match action {
            "org.view" => {
                let members: Vec<serde_json::Value> = org
                    .members
                    .iter()
                    .filter(|(login, _)| role != "recipient" || **login == who)
                    .map(|(principal, (role, state))| {
                        let mut member = fake_principal(principal);
                        member["role"] = serde_json::json!(role);
                        member["state"] = serde_json::json!(state);
                        member["since"] = serde_json::json!(now);
                        member
                    })
                    .collect();
                let mut me = fake_principal(&who);
                me["role"] = serde_json::json!(role);
                fake_json(
                    200,
                    serde_json::json!({
                        "org": {
                            "slug": slug, "name": org.name, "root_key": org.root,
                            "publishing": org.publishing_view(&who),
                            "release_rule": if org.two_person() { "two-person" } else { "solo" },
                        },
                        "me": me,
                        "members": members,
                    }),
                )
            }
            "invite.create" => {
                // Exactly one of {github} and {email}.
                let principal = match (json["github"].as_str(), json["email"].as_str()) {
                    (Some(login), None) => format!("github:{}", login.to_ascii_lowercase()),
                    (None, Some(email)) => format!("email:{}", email.to_ascii_lowercase()),
                    _ => return fake_refuse(400, "bad-request"),
                };
                let invited = json["role"].as_str().unwrap_or("").to_string();
                let allowed = role == "owner" || (role == "admin" && !high(&invited));
                if !allowed {
                    return deny(org, "role-cannot");
                }
                self.next_invite += 1;
                let code = format!("inv{:04}", self.next_invite);
                let expires = now + 7 * 86_400;
                org.invites.insert(
                    code.clone(),
                    FakeInvite {
                        principal: principal.clone(),
                        role: invited.clone(),
                        expires,
                        state: "open",
                    },
                );
                org.record(now, &who, "invite.create", &target, "allowed", None);
                fake_json(
                    201,
                    serde_json::json!({
                        "code": code, "expires_at": expires, "principal": principal,
                        "role": invited,
                        "join_url": format!(
                            "{}/join/?o={slug}&c={code}",
                            self.site.as_deref().unwrap_or("https://krate.tech")
                        ),
                    }),
                )
            }
            "invite.revoke" => {
                let code = parts[3];
                let Some(invite) = org.invites.get(code) else {
                    return deny(org, "invite-revoked");
                };
                if !(role == "owner" || (role == "admin" && !high(&invite.role))) {
                    return deny(org, "role-cannot");
                }
                if invite.state == "used" {
                    return deny(org, "invite-used");
                }
                org.invites.get_mut(code).unwrap().state = "revoked";
                org.record(now, &who, "invite.revoke", code, "allowed", None);
                (204, vec![], vec![])
            }
            "member.role" => {
                let login = parts[3].to_string();
                let new = json["role"].as_str().unwrap_or("").to_string();
                let Some(old) = org.role_of(&login).map(str::to_string) else {
                    return deny(org, "not-a-member");
                };
                // Anything touching an owner or an admin is an owner's.
                let allowed = role == "owner" || (role == "admin" && !high(&old) && !high(&new));
                if !allowed {
                    return deny(org, "role-cannot");
                }
                if old == "owner" && new != "owner" && org.owners() <= 1 {
                    return deny(org, "last-owner");
                }
                org.members
                    .insert(login.clone(), (new.clone(), "active".into()));
                org.record(
                    now,
                    &who,
                    "member.role",
                    &target,
                    "allowed",
                    Some(&format!("{old}->{new}")),
                );
                let mut membership = fake_principal(&login);
                membership["org"] = serde_json::json!(slug);
                membership["role"] = serde_json::json!(new);
                fake_json(200, serde_json::json!({ "membership": membership }))
            }
            "member.remove" => {
                let login = parts[3].to_string();
                let Some(theirs) = org.role_of(&login).map(str::to_string) else {
                    return deny(org, "not-a-member");
                };
                let allowed =
                    role == "owner" || login == who || (role == "admin" && theirs != "owner");
                if !allowed {
                    return deny(org, "role-cannot");
                }
                if theirs == "owner" && org.owners() <= 1 {
                    return deny(org, "last-owner");
                }
                org.members
                    .insert(login.clone(), (theirs.clone(), "removed".into()));
                org.record(
                    now,
                    &who,
                    "member.remove",
                    &target,
                    "allowed",
                    Some(&theirs),
                );
                (204, vec![], vec![])
            }
            "root-key.set" => {
                if role != "owner" {
                    return deny(org, "role-cannot");
                }
                let key = json["public_key"]
                    .as_str()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let mut withdrawn = Vec::new();
                if !json["revocations"].is_null() {
                    let list = &json["revocations"]["list"];
                    if list["root"].as_str() != Some(key.as_str()) {
                        return deny_with(
                            org,
                            "not-signed-for-this-org",
                            &format!(
                                "this app is not signed for {slug}: the revocation list is \
                                 signed by a different root than the key being registered"
                            ),
                        );
                    }
                    for r in list["revocations"].as_array().cloned().unwrap_or_default() {
                        withdrawn.push(r["key"].as_str().unwrap_or("").to_string());
                    }
                }
                let previous = org.root.replace(key.clone());
                if previous.as_deref() != Some(key.as_str()) {
                    let what = if previous.is_some() {
                        "root-key.rotate"
                    } else {
                        "root-key.register"
                    };
                    org.record(
                        now,
                        &who,
                        what,
                        &format!("root-key:{}", &key[..16.min(key.len())]),
                        "allowed",
                        None,
                    );
                }
                let mut added = 0;
                for k in withdrawn {
                    if org.revoked.insert(k.clone()) {
                        org.record(
                            now,
                            &who,
                            "release-key.revoke",
                            &format!("release-key:{}", &k[..16.min(k.len())]),
                            "allowed",
                            None,
                        );
                        added += 1;
                    }
                }
                fake_json(
                    200,
                    serde_json::json!({ "key": {
                        "public_key": key,
                        "set_at": now,
                        "set_by": who,
                        "previous": previous.filter(|p| *p != key),
                        "revoked_keys": org.revoked.len(),
                        "revoked_now": added,
                    } }),
                )
            }
            "candidate.publish" => {
                if !is(&["owner", "developer"]) {
                    return deny(org, "role-cannot");
                }
                // A release key whose owner can no longer publish is refused
                // from the moment it is due for withdrawal (IC-958).
                org.sweep_signing(now);
                let signer = krate_bundle::open_bytes(body)
                    .ok()
                    .and_then(|o| o.signature_envelope().ok().flatten())
                    .map(|e| e.public_key);
                if let Some(p) = signer
                    .as_deref()
                    .and_then(|k| org.signing.publishers.get(k))
                    .filter(|p| p.state == "revoke-due" || p.state == "revoked")
                {
                    let whose = p
                        .principal
                        .split_once(':')
                        .map_or(p.principal.clone(), |(_, v)| v.to_string());
                    return deny_with(
                        org,
                        "not-signed-for-this-org",
                        &format!("this app is not signed for {slug}: its release key belonged to {whose}, who can no longer publish to {slug}"),
                    );
                }
                let digest = format!("{:x}", Sha256::digest(body));
                let (name, version, signer) =
                    match fake_signed_for(body, org.root.as_deref(), &org.revoked) {
                        Ok(app) => app,
                        Err(why) => {
                            return deny_with(
                                org,
                                "not-signed-for-this-org",
                                &format!("this app is not signed for {slug}: {why}"),
                            )
                        }
                    };
                if !fake_marked_for(body, &slug) {
                    return deny_with(
                        org,
                        "not-marked-for-this-org",
                        &format!("this app is not marked as {slug}'s private app"),
                    );
                }
                if let Some(existing) = org.candidates.get(&digest) {
                    return fake_json(
                        200,
                        serde_json::json!({
                            "digest": digest, "state": existing.state,
                            "name": existing.name, "version": existing.version,
                        }),
                    );
                }
                let app_id = fake_app_id(body);
                org.candidates.insert(
                    digest.clone(),
                    FakeCandidate {
                        bytes: body.to_vec(),
                        uploader: who.clone(),
                        uploaded_at: now,
                        name: name.clone(),
                        version: version.clone(),
                        signer,
                        state: "waiting-for-approval",
                        approver: None,
                        approved_at: None,
                        app_id: app_id.clone(),
                        withdraw_reason: None,
                        audience: None,
                    },
                );
                org.record(
                    now,
                    &who,
                    "candidate.publish",
                    &format!("candidate:{digest}"),
                    "allowed",
                    None,
                );
                let approvers: Vec<String> = org
                    .approvers(&who)
                    .iter()
                    .map(|p| p.split_once(':').map_or(p.clone(), |(_, v)| v.to_string()))
                    .collect();
                fake_json(
                    201,
                    serde_json::json!({
                        "digest": digest, "state": "waiting-for-approval",
                        "name": name, "version": version,
                        "solo_release": org.solo_open_for(&who),
                        "approvers": approvers,
                    }),
                )
            }
            "candidate.approve" => {
                let digest = parts[3].to_string();
                // Separation first, whatever the role: the uploader hears
                // that someone else must approve it -- unless nobody else
                // may approve yet and the uploader is its owner (IC-958).
                let own = org
                    .candidates
                    .get(&digest)
                    .is_some_and(|c| c.uploader == who);
                if own
                    && org.solo_open_for(&who)
                    && org
                        .candidates
                        .get(&digest)
                        .is_some_and(|c| c.state == "waiting-for-approval")
                {
                    let candidate = org.candidates.get_mut(&digest).unwrap();
                    candidate.state = "released";
                    candidate.approver = Some(who.clone());
                    candidate.approved_at = Some(now);
                    let app = candidate.app_id.clone();
                    org.current.insert(app, digest.clone());
                    org.signing.solo.insert(digest.clone());
                    org.record(
                        now,
                        &who,
                        "candidate.approve",
                        &target,
                        "allowed",
                        Some("solo"),
                    );
                    return fake_json(
                        200,
                        serde_json::json!({ "release": {
                            "digest": digest, "url": format!("{base}/a/{digest}"), "solo": true,
                        } }),
                    );
                }
                if own {
                    return deny(org, "self-approval");
                }
                if !is(&["owner", "reviewer"]) {
                    return deny(org, "role-cannot");
                }
                let Some(candidate) = org.candidates.get(&digest) else {
                    return deny(org, "digest-mismatch");
                };
                if candidate.state == "withdrawn" {
                    return deny(org, "release-withdrawn");
                }
                if candidate.state != "released" && org.revoked.contains(&candidate.signer) {
                    return deny_with(
                        org,
                        "not-signed-for-this-org",
                        &format!(
                            "this app is not signed for {slug}: its release key was withdrawn \
                             by the organisation's root after it was uploaded"
                        ),
                    );
                }
                if candidate.state != "released" {
                    let candidate = org.candidates.get_mut(&digest).unwrap();
                    candidate.state = "released";
                    candidate.approver = Some(who.clone());
                    candidate.approved_at = Some(now);
                    let app = candidate.app_id.clone();
                    org.current.insert(app, digest.clone());
                    org.record(now, &who, "candidate.approve", &target, "allowed", None);
                }
                fake_json(
                    200,
                    serde_json::json!({ "release": {
                        "digest": digest, "url": format!("{base}/a/{digest}")
                    } }),
                )
            }
            "release.list" => {
                let list: Vec<serde_json::Value> = org
                    .candidates
                    .iter()
                    .filter(|(_, c)| c.state == "released")
                    .filter(|(_, c)| org.in_audience(&who, &role, &c.audience))
                    .map(|(digest, c)| {
                        serde_json::json!({
                            "name": c.name, "version": c.version, "digest": digest,
                            "url": format!("{base}/a/{digest}"),
                            "uploaded_by": fake_principal(&c.uploader),
                            "approved_by": c.approver.as_deref().map(fake_principal),
                            "at": c.approved_at,
                            "app_id": c.app_id,
                            "current": org.current_of(&c.app_id).as_deref() == Some(digest.as_str()),
                            "audience": c.audience,
                            "solo": org.signing.solo.contains(digest),
                        })
                    })
                    .collect();
                fake_json(200, serde_json::json!(list))
            }
            "candidate.list" => {
                if role == "recipient" {
                    return deny(org, "role-cannot");
                }
                let list: Vec<serde_json::Value> = org
                    .candidates
                    .iter()
                    .map(|(digest, c)| {
                        serde_json::json!({
                            "digest": digest, "name": c.name, "version": c.version,
                            "state": c.state, "uploaded_by": fake_principal(&c.uploader),
                            "uploaded_at": c.uploaded_at,
                            "approved_by": c.approver.as_deref().map(fake_principal),
                            "approved_at": c.approved_at,
                        })
                    })
                    .collect();
                fake_json(200, serde_json::json!(list))
            }
            "lease.issue" => {
                let digest = parts[3].to_string();
                if org.candidates.get(&digest).map(|c| c.state) == Some("withdrawn") {
                    return deny(org, "release-withdrawn");
                }
                if org.candidates.get(&digest).map(|c| c.state) != Some("released") {
                    return deny(org, "digest-mismatch");
                }
                if !org.in_audience(&who, &role, &org.candidates[&digest].audience) {
                    return deny(org, "not-in-audience");
                }
                if self.lease_class == "online" {
                    return fake_json(200, serde_json::json!({ "class": "online" }));
                }
                let issued = (std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64
                    + self.lease_shift) as u64;
                let days = if self.lease_class == "days-30" { 30 } else { 7 };
                self.leases_issued += 1;
                let body = serde_json::json!({
                    "v": 1, "org": slug, "digest": digest, "principal": who,
                    "class": self.lease_class, "issued_at": issued,
                    "expires_at": issued + days * 86_400,
                    "lease_id": format!("lease{}", self.leases_issued),
                });
                org.record(now, &who, "lease.issue", &digest, "allowed", None);
                fake_json(
                    200,
                    krate_lease::sign(&body, &self.lease_pkcs8).expect("signed"),
                )
            }
            "release.withdraw" => {
                if !is(&["owner", "admin", "reviewer"]) {
                    return deny(org, "role-cannot");
                }
                let reason = json["reason"].as_str().unwrap_or("").trim().to_string();
                if reason.is_empty() || reason.len() > 300 {
                    return deny(org, "reason-required");
                }
                let digest = parts[3].to_string();
                match org.candidates.get(&digest).map(|c| c.state) {
                    Some("released") => {}
                    Some("withdrawn") => return deny(org, "release-withdrawn"),
                    _ => return deny(org, "digest-mismatch"),
                }
                let app = org.candidates[&digest].app_id.clone();
                let was_current = org.current_of(&app).as_deref() == Some(digest.as_str());
                let candidate = org.candidates.get_mut(&digest).unwrap();
                candidate.state = "withdrawn";
                candidate.withdraw_reason = Some(reason.clone());
                let (name, version) = (candidate.name.clone(), candidate.version.clone());
                org.record(
                    now,
                    &who,
                    "release.withdraw",
                    &target,
                    "allowed",
                    Some(&reason),
                );
                org.current.remove(&app);
                let now_current = org.current_of(&app);
                if let Some(d) = &now_current {
                    org.current.insert(app.clone(), d.clone());
                }
                if was_current {
                    let said = format!(
                        "{} after withdrawing {digest}",
                        now_current.as_deref().unwrap_or("none")
                    );
                    org.record(
                        now,
                        &who,
                        "release.current",
                        &format!("app:{app}"),
                        "allowed",
                        Some(&said),
                    );
                }
                let lease_until = self.leases_issued.min(1) as u64 * (now + 7 * 86_400);
                fake_json(
                    200,
                    serde_json::json!({
                        "release": { "digest": digest, "name": name, "version": version, "state": "withdrawn" },
                        "current": { "app_id": app, "digest": now_current },
                        "leases": { "active": self.leases_issued.min(1), "until": if lease_until > 0 { serde_json::json!(lease_until) } else { serde_json::Value::Null } },
                    }),
                )
            }
            "release.set-current" => {
                if !is(&["owner", "admin"]) {
                    return deny(org, "role-cannot");
                }
                let app = parts[3].to_ascii_lowercase();
                let digest = json["digest"].as_str().unwrap_or("").to_string();
                let Some(candidate) = org.candidates.get(&digest).filter(|c| c.app_id == app)
                else {
                    return deny(org, "digest-mismatch");
                };
                if candidate.state == "withdrawn" {
                    return deny(org, "release-withdrawn");
                }
                if candidate.state != "released" {
                    return deny(org, "digest-mismatch");
                }
                let current = serde_json::json!({
                    "digest": digest, "name": candidate.name, "version": candidate.version,
                });
                if org.current_of(&app).as_deref() != Some(digest.as_str()) {
                    org.current.insert(app.clone(), digest.clone());
                    org.record(
                        now,
                        &who,
                        "release.current",
                        &target,
                        "allowed",
                        Some(&digest),
                    );
                }
                fake_json(
                    200,
                    serde_json::json!({ "app_id": app, "current": current }),
                )
            }
            // IC-953 (org-releases.js op_app_releases): owners, admins and
            // reviewers see every app; anyone else only apps whose current
            // release is theirs to open, and no withdrawn version.
            "release.apps" => {
                let everything = is(&["owner", "admin", "reviewer"]);
                let mut by_app: std::collections::BTreeMap<String, Vec<serde_json::Value>> =
                    Default::default();
                let mut order: Vec<(&String, &FakeCandidate)> = org
                    .candidates
                    .iter()
                    .filter(|(_, c)| matches!(c.state, "released" | "withdrawn"))
                    .filter(|(_, c)| everything || c.state == "released")
                    .filter(|(_, c)| org.in_audience(&who, &role, &c.audience))
                    .collect();
                order.sort_by_key(|(_, c)| std::cmp::Reverse(c.approved_at));
                for (digest, c) in order {
                    let caps: Vec<serde_json::Value> = krate_bundle::open_bytes(&c.bytes)
                        .ok()
                        .and_then(|o| o.manifest().declared_capabilities().ok())
                        .unwrap_or_default()
                        .iter()
                        .map(|cap| serde_json::json!({ "cap": cap.to_string(), "words": cap.to_string() }))
                        .collect();
                    by_app
                        .entry(c.app_id.clone())
                        .or_default()
                        .push(serde_json::json!({
                            "app_id": c.app_id, "name": c.name, "version": c.version,
                            "digest": digest, "url": format!("{base}/a/{digest}"),
                            "state": c.state, "size": c.bytes.len(), "capabilities": caps,
                            "offline": self.lease_class, "audience": c.audience,
                        }));
                }
                let apps: Vec<serde_json::Value> = by_app
                    .into_iter()
                    .filter_map(|(app, releases)| {
                        let current = org.current_of(&app);
                        let now = releases
                            .iter()
                            .find(|r| current.as_deref() == r["digest"].as_str())
                            .cloned();
                        if now.is_none() && !everything {
                            return None;
                        }
                        let now = now.unwrap_or(serde_json::Value::Null);
                        Some(serde_json::json!({
                            "app_id": app, "name": releases[0]["name"],
                            "current": now["digest"], "digest": now["digest"],
                            "version": now["version"], "size": now["size"],
                            "capabilities": now["capabilities"], "offline": now["offline"],
                            "link": format!("{base}/orgs/{slug}/apps/{app}/current"),
                            "releases": releases,
                        }))
                    })
                    .collect();
                fake_json(200, serde_json::json!({ "apps": apps }))
            }
            "release.current-of" => {
                let app = parts[3].to_ascii_lowercase();
                let Some(digest) = org.current_of(&app) else {
                    return deny(org, "digest-mismatch");
                };
                let c = &org.candidates[&digest];
                if !org.in_audience(&who, &role, &c.audience) {
                    return deny(org, "not-in-audience");
                }
                fake_json(
                    200,
                    serde_json::json!({
                        "app_id": app, "digest": digest, "name": c.name, "version": c.version,
                        "url": format!("{base}/a/{digest}"),
                    }),
                )
            }
            "release.audience" => {
                if !is(&["owner", "admin"]) {
                    return deny(org, "role-cannot");
                }
                let digest = parts[3].to_string();
                if !org.candidates.contains_key(&digest) {
                    return deny(org, "digest-mismatch");
                }
                let audience = if json["everyone"].as_bool() == Some(true) {
                    None
                } else {
                    let mut groups: Vec<String> = json["groups"]
                        .as_array()
                        .map(|g| {
                            g.iter()
                                .filter_map(|x| x.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default();
                    groups.sort();
                    if groups.is_empty() {
                        return deny(org, "group-invalid");
                    }
                    if let Some(missing) = groups.iter().find(|g| !org.groups.contains_key(*g)) {
                        let missing = missing.clone();
                        return deny_with(
                            org,
                            "no-such-group",
                            &format!("{slug} has no group called {missing}"),
                        );
                    }
                    Some(groups)
                };
                org.candidates.get_mut(&digest).unwrap().audience = audience.clone();
                let said = audience
                    .as_ref()
                    .map(|g| format!("groups:{}", g.join(",")))
                    .unwrap_or_else(|| "everyone".into());
                org.record(
                    now,
                    &who,
                    "release.audience",
                    &target,
                    "allowed",
                    Some(&said),
                );
                fake_json(
                    200,
                    serde_json::json!({ "digest": digest, "audience": audience }),
                )
            }
            "group.list" => {
                let groups: Vec<serde_json::Value> = org
                    .groups
                    .iter()
                    .filter(|(_, m)| role != "recipient" || m.contains(&who))
                    .map(|(name, members)| serde_json::json!({ "name": name, "members": members }))
                    .collect();
                fake_json(200, serde_json::json!({ "groups": groups }))
            }
            "group.manage" => {
                if !is(&["owner", "admin"]) {
                    return deny(org, "role-cannot");
                }
                let name = json["name"].as_str().unwrap_or("").to_string();
                let valid = !name.is_empty()
                    && name.len() <= 39
                    && name.as_bytes()[0].is_ascii_lowercase()
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
                if !valid {
                    return deny(org, "group-invalid");
                }
                if org.groups.contains_key(&name) {
                    return deny(org, "group-taken");
                }
                org.groups.insert(name.clone(), Default::default());
                org.record(now, &who, "group.create", &target, "allowed", None);
                fake_json(
                    201,
                    serde_json::json!({ "group": { "name": name, "members": [] } }),
                )
            }
            "group.add" | "group.remove" => {
                if !is(&["owner", "admin"]) {
                    return deny(org, "role-cannot");
                }
                let (name, principal) = (parts[3].to_string(), parts[5].to_string());
                if !org.groups.contains_key(&name) {
                    return deny(org, "no-such-group");
                }
                if action == "group.add" {
                    if org.role_of(&principal).is_none() {
                        return deny(org, "not-a-member");
                    }
                    if org.groups.get_mut(&name).unwrap().insert(principal.clone()) {
                        org.record(now, &who, "group.add", &target, "allowed", None);
                    }
                    return fake_json(
                        200,
                        serde_json::json!({ "group": name, "principal": principal, "member": true }),
                    );
                }
                if org.groups.get_mut(&name).unwrap().remove(&principal) {
                    org.record(now, &who, "group.remove", &target, "allowed", None);
                }
                (204, vec![], vec![])
            }
            "connection.view" => {
                if !is(&["owner", "admin", "developer", "reviewer"]) {
                    return deny(org, "role-cannot");
                }
                let all: Vec<serde_json::Value> = org
                    .connections
                    .iter()
                    .map(|(name, c)| {
                        serde_json::json!({
                            "name": name, "base": c.base,
                            "allow": { "methods": c.methods, "paths": c.paths },
                            "audience": c.audience, "apps": c.apps,
                            "credential": {
                                "kind": c.kind, "header": c.header_name, "set_by": c.set_by, "set_at": c.set_at,
                                "state": if c.credential_v >= 1 { "sealed" } else { "needs-entry" },
                                "version": (c.credential_v >= 1).then_some(c.credential_v),
                            },
                            "uses": { "today": 0, "last_30_days": 0 },
                        })
                    })
                    .collect();
                fake_json(200, serde_json::json!({ "connections": all }))
            }
            "connection.manage" => {
                if !is(&["owner", "admin"]) {
                    return deny(org, "role-cannot");
                }
                // Sealed where it was typed (F-316): kind, header, key
                // version and blob. A value in the clear is never kept.
                let credential = || -> Option<(String, String, u64, String)> {
                    if !json["header"].is_null() || !json["basic"].is_null() {
                        return None;
                    }
                    let c = &json["credential"];
                    Some((
                        c["kind"].as_str()?.to_string(),
                        c["header"].as_str()?.to_string(),
                        c["v"].as_u64()?,
                        c["sealed"].as_str().filter(|s| s.len() >= 68)?.to_string(),
                    ))
                };
                let key_now = org.keys.versions.last().map(|v| v.0);
                if method != "DELETE" {
                    match (key_now, credential()) {
                        (None, _) => return deny(org, "no-org-key"),
                        (Some(now), Some((_, _, v, _))) if v != now => {
                            return deny(org, "key-rotated")
                        }
                        _ => {}
                    }
                }
                match method {
                    "POST" => {
                        let name = json["name"].as_str().unwrap_or("").to_string();
                        if org.connections.contains_key(&name) {
                            return deny(org, "connection-taken");
                        }
                        let Some((kind, header_name, credential_v, sealed)) = credential() else {
                            return deny(org, "connection-invalid");
                        };
                        let strings = |v: &serde_json::Value| -> Vec<String> {
                            v.as_array()
                                .map(|a| {
                                    a.iter()
                                        .filter_map(|x| x.as_str().map(str::to_string))
                                        .collect()
                                })
                                .unwrap_or_default()
                        };
                        let audience = json["audience"]["groups"]
                            .as_array()
                            .map(|_| strings(&json["audience"]["groups"]));
                        let apps = json["apps"]["ids"]
                            .as_array()
                            .map(|_| strings(&json["apps"]["ids"]));
                        let connection = FakeConnection {
                            base: json["base"]
                                .as_str()
                                .unwrap_or("")
                                .trim_end_matches('/')
                                .to_string(),
                            methods: strings(&json["allow"]["methods"])
                                .iter()
                                .map(|m| m.to_ascii_uppercase())
                                .collect(),
                            paths: strings(&json["allow"]["paths"]),
                            audience,
                            apps,
                            kind,
                            header_name,
                            sealed,
                            credential_v,
                            set_by: who.clone(),
                            set_at: now,
                        };
                        let view = serde_json::json!({
                            "name": name, "base": connection.base,
                            "allow": { "methods": connection.methods, "paths": connection.paths },
                            "audience": connection.audience, "apps": connection.apps,
                            "credential": { "kind": connection.kind, "header": connection.header_name, "set_by": who, "set_at": now, "state": "sealed", "version": credential_v },
                            "uses": { "today": 0, "last_30_days": 0 },
                        });
                        let base = connection.base.clone();
                        org.connections.insert(name.clone(), connection);
                        org.record(
                            now,
                            &who,
                            "connection.create",
                            &target,
                            "allowed",
                            Some(&base),
                        );
                        fake_json(201, serde_json::json!({ "connection": view }))
                    }
                    "PUT" => {
                        let name = parts[3].to_string();
                        let Some((kind, header_name, credential_v, sealed)) = credential() else {
                            return deny(org, "connection-invalid");
                        };
                        let Some(c) = org.connections.get_mut(&name) else {
                            return deny(org, "no-such-connection");
                        };
                        c.kind = kind;
                        c.header_name = header_name;
                        c.sealed = sealed;
                        c.credential_v = credential_v;
                        c.set_by = who.clone();
                        c.set_at = now;
                        org.record(now, &who, "connection.credential", &target, "allowed", None);
                        fake_json(200, serde_json::json!({ "connection": { "name": name } }))
                    }
                    _ => {
                        if org.connections.remove(parts[3]).is_none() {
                            return deny(org, "no-such-connection");
                        }
                        org.record(now, &who, "connection.remove", &target, "allowed", None);
                        (204, vec![], vec![])
                    }
                }
            }
            "connection.use" => {
                if self.connections_down {
                    return fake_refuse(503, "busy");
                }
                let (name, digest) = (parts[3].to_string(), parts[5].to_string());
                let Some(c) = org.candidates.get(&digest) else {
                    return deny(org, "digest-mismatch");
                };
                if c.state == "withdrawn" {
                    return deny(org, "release-withdrawn");
                }
                if c.state != "released" {
                    return deny(org, "digest-mismatch");
                }
                if !org.in_audience(&who, &role, &c.audience) {
                    return deny(org, "not-in-audience");
                }
                let (app_id, asked, app_name) = (
                    c.app_id.clone(),
                    fake_connections_asked(&c.bytes),
                    c.name.clone(),
                );
                let Some(conn) = org.connections.get(&name) else {
                    return deny_with(
                        org,
                        "no-such-connection",
                        &format!(
                            "{} has no connection called {name} any more -- ask an admin of {}",
                            org.name, org.name
                        ),
                    );
                };
                let base = conn.base.clone();
                let refusal = if let Some(groups) = conn.audience.clone().filter(|g| {
                    !g.iter()
                        .any(|g| org.groups.get(g).is_some_and(|m| m.contains(&who)))
                }) {
                    Some((
                        "connection-not-in-audience",
                        format!("the {name} connection is only for the {} group in {} -- ask an admin of {} to add you", groups.join(", "), org.name, org.name),
                    ))
                } else if conn
                    .apps
                    .as_ref()
                    .is_some_and(|apps| !apps.contains(&app_id))
                {
                    Some((
                        "app-not-allowed",
                        format!("{} has not let {app_name} use the {name} connection -- an owner or admin of {} can add {app_id} to it", org.name, org.name),
                    ))
                } else if !asked.contains(&name) {
                    Some((
                        "connection-not-requested",
                        format!("this version of {app_name} did not ask for the {name} connection (net.connection:{name} in its manifest), so it does not get it"),
                    ))
                } else {
                    None
                };
                if let Some((reason, message)) = refusal {
                    org.record(now, &who, action, &target, "denied", Some(reason));
                    return fake_json(
                        403,
                        serde_json::json!({ "error": reason, "message": message, "base": base }),
                    );
                }
                let conn = &org.connections[&name];
                if conn.credential_v == 0 {
                    return fake_json(
                        409,
                        serde_json::json!({ "error": "credential-needs-entry", "message": format!("the {name} credential must be entered again by an owner or admin of {}", org.name), "base": base }),
                    );
                }
                self.connection_fetches += 1;
                // The blob as it came, unopened: the hub cannot open it.
                fake_json(
                    200,
                    serde_json::json!({
                        "name": name, "base": conn.base,
                        "allow": { "methods": conn.methods, "paths": conn.paths },
                        "credential": { "kind": conn.kind, "header": conn.header_name, "v": conn.credential_v, "sealed": conn.sealed },
                    }),
                )
            }
            "audit.read" => {
                if !is(&["owner", "admin"]) {
                    return deny(org, "role-cannot");
                }
                let chain_ok = fake_chain_ok(&org.audit);
                fake_json(
                    200,
                    serde_json::json!({ "events": org.audit, "chain_ok": chain_ok }),
                )
            }
            _ => (404, vec![], b"not found".to_vec()),
        }
    }

    fn big_path(
        &mut self,
        method: &str,
        parts: &[&str],
        who: Option<String>,
        body: &[u8],
    ) -> FakeAnswer {
        let now = self.now;
        match (method, parts.len()) {
            ("POST", 3) if parts[2] == "start" => {
                let Some(who) = who else {
                    return fake_refuse(401, "not-signed-in");
                };
                let ask: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
                let slug = ask["org"].as_str().unwrap_or("").to_string();
                let Some(org) = self.orgs.get_mut(&slug) else {
                    return fake_refuse(404, "no-such-org");
                };
                if !matches!(org.role_of(&who), Some("owner" | "developer")) {
                    org.record(
                        now,
                        &who,
                        "candidate.publish",
                        &format!("org:{slug}"),
                        "denied",
                        Some("role-cannot"),
                    );
                    return fake_refuse(403, "role-cannot");
                }
                let size = ask["size"].as_u64().unwrap_or(0) as usize;
                let part_size = 5 * 1024 * 1024usize;
                self.big = Some(FakeBigUpload {
                    org: slug,
                    who,
                    parts: Default::default(),
                });
                fake_json(
                    200,
                    serde_json::json!({
                        "id": "orgup1", "parts": size.div_ceil(part_size),
                        "part_size": part_size, "token": "uptok",
                    }),
                )
            }
            ("PUT", 5) if parts[3] == "part" => {
                let n: usize = parts[4].parse().unwrap_or(0);
                if let Some(big) = self.big.as_mut() {
                    big.parts.insert(n, body.to_vec());
                }
                fake_json(
                    200,
                    serde_json::json!({ "part": n, "etag": format!("e{n}") }),
                )
            }
            ("POST", 4) if parts[3] == "finish" => fake_json(
                202,
                serde_json::json!({ "id": "orgup1", "kind": "bundle", "status": "judging" }),
            ),
            ("GET", 3) => {
                let Some(big) = self.big.take() else {
                    return (404, vec![], vec![]);
                };
                let bytes: Vec<u8> = big.parts.values().flatten().copied().collect();
                let digest = format!("{:x}", Sha256::digest(&bytes));
                let slug = big.org.clone();
                let org = self.orgs.get_mut(&slug).unwrap();
                let (name, version, signer) = match fake_signed_for(
                    &bytes,
                    org.root.as_deref(),
                    &org.revoked,
                ) {
                    Ok(app) if fake_marked_for(&bytes, &slug) => app,
                    Ok(_) => {
                        org.record(
                            now,
                            &big.who,
                            "candidate.publish",
                            &format!("candidate:{digest}"),
                            "denied",
                            Some("not-marked-for-this-org"),
                        );
                        return fake_json(
                            200,
                            serde_json::json!({
                                "id": "orgup1", "kind": "bundle", "status": "refused", "org": slug,
                                "problem": format!(
                                    "not-marked-for-this-org: this app is not marked as {slug}'s private app"
                                ),
                            }),
                        );
                    }
                    Err(why) => {
                        org.record(
                            now,
                            &big.who,
                            "candidate.publish",
                            &format!("candidate:{digest}"),
                            "denied",
                            Some("not-signed-for-this-org"),
                        );
                        return fake_json(
                            200,
                            serde_json::json!({
                                "id": "orgup1", "kind": "bundle", "status": "refused", "org": slug,
                                "problem": format!(
                                    "not-signed-for-this-org: this app is not signed for {slug}: {why}"
                                ),
                            }),
                        );
                    }
                };
                let app_id = fake_app_id(&bytes);
                org.candidates.insert(
                    digest.clone(),
                    FakeCandidate {
                        bytes,
                        uploader: big.who.clone(),
                        uploaded_at: now,
                        name,
                        version,
                        signer,
                        state: "waiting-for-approval",
                        approver: None,
                        approved_at: None,
                        app_id: app_id.clone(),
                        withdraw_reason: None,
                        audience: None,
                    },
                );
                org.record(
                    now,
                    &big.who,
                    "candidate.publish",
                    &format!("candidate:{digest}"),
                    "allowed",
                    None,
                );
                fake_json(
                    200,
                    serde_json::json!({
                        "id": "orgup1", "kind": "bundle", "status": "candidate", "org": slug,
                        "digest": digest, "state": "waiting-for-approval",
                    }),
                )
            }
            _ => (404, vec![], vec![]),
        }
    }
}

/// A fake organisation hub on a local port, and one signed-in home per
/// person (`tok-<login>` is each one's GitHub token as the hub sees it).
struct OrgWorld {
    hub: std::sync::Arc<std::sync::Mutex<FakeOrgHub>>,
    addr: SocketAddr,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    homes: std::collections::BTreeMap<String, tempfile::TempDir>,
    dir: tempfile::TempDir,
    /// What `krate org create` printed: it carries the recovery code.
    created: std::sync::Mutex<String>,
}

impl Drop for OrgWorld {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

impl OrgWorld {
    /// `None` when this machine will not bind a local port (announced by
    /// `bind_local_fixture_listener`).
    fn new(people: &[&str]) -> Option<OrgWorld> {
        let hub = std::sync::Arc::new(std::sync::Mutex::new(FakeOrgHub {
            now: 1_760_400_000,
            lease_pkcs8: krate_lease::generate_pkcs8().expect("lease key"),
            lease_class: "days-7".to_string(),
            ..Default::default()
        }));
        let state = hub.clone();
        let (addr, stop) =
            spawn_fake_hub(std::sync::Arc::new(move |method, path, headers, body| {
                state.lock().unwrap().handle(method, path, headers, body)
            }))?;
        hub.lock().unwrap().base = format!("http://{addr}");
        let mut homes = std::collections::BTreeMap::new();
        for name in people.iter().chain(["nobody"].iter()) {
            let login = OrgWorld::login_of(name);
            let home = tempfile::tempdir().expect("home");
            std::fs::create_dir_all(home.path().join(".krate")).expect("krate home");
            if *name != "nobody" {
                std::fs::write(
                    home.path().join(".krate/github.json"),
                    serde_json::json!({
                        "login": login, "name": null, "avatar_url": null,
                        "token": format!("tok-{login}"),
                    })
                    .to_string(),
                )
                .expect("sign-in");
            }
            homes.insert(name.to_string(), home);
        }
        Some(OrgWorld {
            hub,
            addr,
            stop,
            homes,
            dir: tempfile::tempdir().expect("dir"),
            created: Default::default(),
        })
    }

    /// The hub's lease public key, as hex: the fake's, or the real hub's own
    /// (`/lease-key`) when KRATE_ORG_E2E_HUB names one, so a lease the real
    /// hub signs verifies here.
    fn lease_key_hex(&self) -> String {
        if let Some(hub) = self.real_hub() {
            let text = ureq::get(&format!("{hub}/lease-key"))
                .call()
                .expect("the real hub answers /lease-key (is ORG_LEASE_KEY set?)")
                .into_string()
                .expect("text");
            let answer: serde_json::Value = serde_json::from_str(&text).expect("JSON");
            return answer["public_key"]
                .as_str()
                .expect("public_key")
                .to_string();
        }
        krate_lease::public_key_hex(&self.hub.lock().unwrap().lease_pkcs8).expect("public key")
    }

    /// The lease this person's machine holds for `digest`, if any.
    fn lease_record(&self, who: &str, digest: &str) -> Option<serde_json::Value> {
        let path = self.homes[who]
            .path()
            .join(".krate/leases")
            .join(format!("{digest}.json"));
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
    }

    /// Carol's app, signed with her delegated key, published to acme and
    /// approved by Bob: its link and its digest.
    fn released_app(&self, root: &std::path::Path, version: &str) -> (PathBuf, String, String) {
        let bundle = self.bundle(&format!("app-{version}.krate"), version, 64);
        self.sign_delegated(root, "carol", &bundle);
        let digest = OrgWorld::digest_in(&self.says(
            "carol",
            &["publish", bundle.to_str().unwrap(), "--org", "acme"],
            true,
        ));
        self.says("bob", &["org", "approve", "acme", &digest], true);
        let link = format!("{}/a/{digest}", self.url());
        (bundle, digest, link)
    }

    fn url(&self) -> String {
        self.real_hub()
            .unwrap_or_else(|| format!("http://{}", self.addr))
    }

    /// KRATE_ORG_E2E_HUB names a real hub (a local `wrangler dev` started
    /// with KRATE_TEST_IDENTITIES mapping tok-<login> to <login>): the same
    /// proof then runs against the hub's own code instead of this fake.
    fn real_hub(&self) -> Option<String> {
        std::env::var("KRATE_ORG_E2E_HUB")
            .ok()
            .filter(|url| !url.is_empty())
    }

    /// `krate <args>` as `who` ("nobody" is signed out), with the fake as
    /// the hub.
    fn krate_as(&self, who: &str, args: &[&str]) -> std::process::Output {
        let home = self.homes[who].path();
        krate()
            .args(args)
            .env("HOME", home)
            .env("USERPROFILE", home)
            .env("KRATE_HUB_URL", self.url())
            .env("KRATE_CACHE_DIR", home.join("cache"))
            .env("KRATE_NO_OPEN_CACHE", "1")
            // The fake hub's lease key stands in for the pinned one; this
            // debug build honours the override, a release build never does
            // (krate-lease's a_release_build_ignores_the_override).
            .env(krate_lease::LEASE_KEY_OVERRIDE_ENV, self.lease_key_hex())
            .output()
            .expect("krate")
    }

    /// Run it, and give back everything it printed; `ok` says whether it
    /// must succeed or must fail.
    fn says(&self, who: &str, args: &[&str], ok: bool) -> String {
        let output = self.krate_as(who, args);
        let said = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            output.status.success(),
            ok,
            "`krate {}` as {who}: {said}",
            args.join(" ")
        );
        // Never a raw JSON error, whatever happened.
        assert!(
            !said.contains("{\"error\""),
            "raw JSON reached a person: {said}"
        );
        said
    }

    /// Who each person signs in as. Dan signs in by email (tok-<address>
    /// is an inbox the hub saw proven); `alice-mail` is an email account
    /// whose address starts with Alice's GitHub login, and is not Alice.
    /// Everyone else is a GitHub login.
    fn login_of(name: &str) -> String {
        match name {
            "dan" => "dan@acme.example".to_string(),
            "alice-mail" => "alice@corp.example".to_string(),
            // Alice on a second, new computer.
            "alice-laptop" => "alice".to_string(),
            other => other.to_string(),
        }
    }

    /// The code in a `krate org join <slug> <code>` line. A person who
    /// signs in by email is invited by email.
    fn invite(&self, by: &str, person: &str, role: &str) -> String {
        let login = OrgWorld::login_of(person);
        let said = if login.contains('@') {
            self.says(
                by,
                &["org", "invite", "acme", "--email", &login, "--role", role],
                true,
            )
        } else {
            self.says(by, &["org", "invite", "acme", &login, "--role", role], true)
        };
        let code = said
            .lines()
            .find_map(|line| line.trim().strip_prefix("krate org join acme "))
            .unwrap_or_else(|| panic!("no join line: {said}"))
            .trim()
            .to_string();
        // This hub is on this machine and names no site of its own, so no
        // link is offered: krate.tech's join page talks to another hub
        // (K-1105).
        assert!(
            !said.contains("krate.tech/join"),
            "a local invitation links to krate.tech: {said}"
        );
        code
    }

    /// Alice's acme: its root key registered, Bob reviewer, Carol
    /// developer and Dan recipient, each joined.
    fn acme(&self) -> PathBuf {
        let said = self.says("alice", &["org", "create", "acme", "--name", "Acme"], true);
        *self.created.lock().unwrap() = said;
        let root = self.dir.path().join("acme-root.pkcs8");
        std::fs::write(
            &root,
            krate_bundle::signing::SigningKey::generate_pkcs8().expect("root"),
        )
        .expect("root key");
        self.says(
            "alice",
            &["org", "root-key", "acme", root.to_str().unwrap()],
            true,
        );
        for (login, role) in [
            ("bob", "reviewer"),
            ("carol", "developer"),
            ("dan", "recipient"),
        ] {
            if !self.homes.contains_key(login) {
                continue;
            }
            let code = self.invite("alice", login, role);
            let said = self.says(login, &["org", "join", "acme", &code], true);
            assert!(
                said.contains(&format!("you joined acme as {role}")),
                "{said}"
            );
        }
        root
    }

    /// A bundle named `name` at `version`, padded with `noise` incompressible
    /// bytes, packed by the CLI.
    fn bundle(&self, file: &str, version: &str, noise: usize) -> PathBuf {
        self.bundle_asking(file, version, noise, "")
    }

    /// [`OrgWorld::bundle`] with capability tables appended to its manifest.
    fn bundle_asking(&self, file: &str, version: &str, noise: usize, caps: &str) -> PathBuf {
        let dir = self.dir.path().join(format!("src-{file}"));
        std::fs::create_dir_all(dir.join("assets")).expect("dir");
        let wasm = dir.join("code.wasm");
        std::fs::write(
            &wasm,
            include_bytes!("../../bundle/tests/fixtures/minimal-run.wasm"),
        )
        .expect("component");
        let manifest = dir.join("manifest.toml");
        std::fs::write(
            &manifest,
            format!(
                "[app]\nid = \"dev.acme.demo\"\nname = \"Acme Demo\"\nversion = \"{version}\"\n\
                 entry = \"code.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n{caps}"
            ),
        )
        .expect("manifest");
        let mut x = 0x9e37_79b9_7f4a_7c15u64 ^ noise as u64;
        let noise: Vec<u8> = (0..noise.max(1))
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect();
        std::fs::write(dir.join("assets/noise.bin"), noise).expect("noise");
        let bundle = self.dir.path().join(file);
        let packed = krate()
            .arg("pack")
            .arg(&wasm)
            .arg("--manifest")
            .arg(&manifest)
            .arg("-o")
            .arg(&bundle)
            .output()
            .expect("pack");
        assert!(
            packed.status.success(),
            "{}",
            String::from_utf8_lossy(&packed.stderr)
        );
        bundle
    }

    /// `who` signs `bundle` with a release key that acme's root (held by
    /// Alice) delegated.
    fn sign_delegated(&self, root: &std::path::Path, who: &str, bundle: &std::path::Path) {
        let release = self.dir.path().join(format!("{who}-release.pkcs8"));
        if !release.exists() {
            std::fs::write(
                &release,
                krate_bundle::signing::SigningKey::generate_pkcs8().expect("release"),
            )
            .expect("release key");
        }
        let delegation = self.dir.path().join(format!("{who}-acme.json"));
        self.says(
            "alice",
            &[
                "delegate",
                "--root",
                root.to_str().unwrap(),
                "--key",
                release.to_str().unwrap(),
                "--namespace",
                "acme/*",
                "-o",
                delegation.to_str().unwrap(),
            ],
            true,
        );
        self.says(
            who,
            &[
                "sign",
                bundle.to_str().unwrap(),
                "--org",
                "acme",
                "--key",
                release.to_str().unwrap(),
                "--delegation",
                delegation.to_str().unwrap(),
            ],
            true,
        );
    }

    /// The 64-hex digest a `krate publish --org` printed.
    fn digest_in(said: &str) -> String {
        said.lines()
            .find_map(|line| line.trim().strip_prefix("digest "))
            .map(|d| d.trim().to_string())
            .unwrap_or_else(|| panic!("no digest line: {said}"))
    }

    fn run_link(&self, who: &str, url: &str) -> std::process::Output {
        self.krate_as(
            who,
            &["run", url, "--insecure-http", "--auto-grant", "--headless"],
        )
    }
}

/// `krate org list`: your organisations with your role in each, the email
/// member's included; nobody's but your own; and a plain line when there
/// are none.
#[test]
fn org_list_shows_your_organisations_and_your_role() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "dan", "erin"]) else {
        eprintln!("skipping org_list_shows_your_organisations_and_your_role: no local port");
        return;
    };
    let said = world.says("alice", &["org", "list"], true);
    assert!(
        said.contains("you're in no organisations yet: krate org create <name>"),
        "{said}"
    );
    world.acme();
    world.says(
        "bob",
        &["org", "create", "globex", "--name", "Globex"],
        true,
    );
    let said = world.says("bob", &["org", "list"], true);
    assert!(said.contains("you are in 2 organisations:"), "{said}");
    assert!(
        said.lines()
            .any(|l| l.contains("acme") && l.contains("Acme") && l.contains("reviewer")),
        "{said}"
    );
    assert!(
        said.lines()
            .any(|l| l.contains("globex") && l.contains("Globex") && l.contains("owner")),
        "{said}"
    );
    let said = world.says("dan", &["org", "list"], true);
    assert!(
        said.contains("you are in 1 organisation:")
            && said
                .lines()
                .any(|l| l.contains("acme") && l.contains("recipient"))
            && !said.contains("globex"),
        "{said}"
    );
    let said = world.says("erin", &["org", "list"], true);
    assert!(said.contains("you're in no organisations yet"), "{said}");
    let said = world.says("nobody", &["org", "list"], false);
    assert!(said.contains("you are not signed in"), "{said}");
}

/// `krate org create`: the creator is its owner and is told what to do
/// next; a taken slug and an invalid one are refused in words.
#[test]
fn org_create_makes_you_the_owner_and_refuses_taken_or_invalid_names() {
    let Some(world) = OrgWorld::new(&["alice", "bob"]) else {
        eprintln!("skipping org_create_makes_you_the_owner_and_refuses_taken_or_invalid_names: no local port");
        return;
    };
    let said = world.says("alice", &["org", "create", "acme", "--name", "Acme"], true);
    assert!(
        said.contains("created acme (\"Acme\"). You are its owner."),
        "{said}"
    );
    // Its apps are signed by it from now on, and the next step is an app,
    // not a key (IC-958).
    assert!(
        said.contains("Every app Acme releases is signed by Acme"),
        "{said}"
    );
    assert!(
        said.contains("krate publish app.krate --org acme"),
        "{said}"
    );
    assert!(!said.contains("root-key"), "{said}");

    let said = world.says("bob", &["org", "create", "acme"], false);
    assert!(
        said.contains("an organisation called acme already exists")
            && said.contains("pick another name"),
        "{said}"
    );
    let said = world.says("bob", &["org", "create", "Acme!"], false);
    assert!(
        said.contains("cannot name an organisation") && said.contains("lower-case letters"),
        "{said}"
    );
    let hub = world.hub.lock().unwrap();
    assert!(
        hub.keyless_mutations.is_empty(),
        "{:?}",
        hub.keyless_mutations
    );
    assert_eq!(hub.orgs["acme"].role_of("github:alice"), Some("owner"));
}

/// `krate org show`: members and roles; a recipient sees only their own
/// entry; an organisation that is not there, and a stranger, in words.
#[test]
fn org_show_lists_members_and_refuses_strangers_in_words() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "carol", "dan", "erin"]) else {
        eprintln!("skipping org_show_lists_members_and_refuses_strangers_in_words: no local port");
        return;
    };
    world.acme();
    let said = world.says("alice", &["org", "show", "acme"], true);
    assert!(said.contains("acme -- Acme"), "{said}");
    assert!(said.contains("you are alice (GitHub), owner"), "{said}");
    for (login, role) in [
        ("bob", "reviewer"),
        ("carol", "developer"),
        ("dan", "recipient"),
    ] {
        assert!(
            said.lines().any(|l| l.contains(login) && l.contains(role)),
            "{login} {role}: {said}"
        );
    }
    assert!(
        said.lines()
            .any(|l| l.contains("alice (GitHub)") && l.contains("owner")),
        "{said}"
    );
    assert!(
        said.lines()
            .any(|l| l.contains("dan@acme.example (email)") && l.contains("recipient")),
        "{said}"
    );
    let said = world.says("dan", &["org", "show", "acme"], true);
    assert!(
        said.contains("1 person:") && !said.contains("carol"),
        "{said}"
    );

    let said = world.says("erin", &["org", "show", "acme"], false);
    assert!(
        said.contains("erin is not a member of acme: ask an owner or admin of acme to invite erin"),
        "{said}"
    );
    let said = world.says("alice", &["org", "show", "globex"], false);
    assert!(
        said.contains("there is no organisation called globex")
            && said.contains("krate org create globex"),
        "{said}"
    );
}

/// `krate org invite`: prints the one command the invited person runs;
/// an admin cannot invite an owner (role-cannot, in words).
#[test]
fn org_invite_prints_the_join_command_and_an_admin_cannot_invite_an_owner() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "erin"]) else {
        eprintln!("skipping org_invite_prints_the_join_command_and_an_admin_cannot_invite_an_owner: no local port");
        return;
    };
    world.says("alice", &["org", "create", "acme"], true);
    let said = world.says(
        "alice",
        &["org", "invite", "acme", "Bob", "--role", "admin"],
        true,
    );
    assert!(
        said.contains("invited bob (GitHub) to acme as admin."),
        "{said}"
    );
    assert!(
        said.contains("only for bob (GitHub), until 2025-10-21"),
        "{said}"
    );
    let code = said
        .lines()
        .find_map(|l| l.trim().strip_prefix("krate org join acme "))
        .expect("join line")
        .to_string();
    world.says("bob", &["org", "join", "acme", &code], true);

    let said = world.says(
        "bob",
        &["org", "invite", "acme", "erin", "--role", "owner"],
        false,
    );
    assert!(
        said.contains("your role in acme cannot invite an owner or admin: ask an owner of acme"),
        "{said}"
    );
    // An admin may invite a developer.
    let said = world.says(
        "bob",
        &["org", "invite", "acme", "erin", "--role", "developer"],
        true,
    );
    assert!(
        said.contains("invited erin (GitHub) to acme as developer."),
        "{said}"
    );
    // This hub is on this machine and names no site: no krate.tech link,
    // whose join page talks to another hub (K-1105). Once it names the site
    // on this machine (SITE_BASE), that is the link.
    assert!(!said.contains("krate.tech/join"), "{said}");
    world.hub.lock().unwrap().site = Some("http://127.0.0.1:8000".into());
    let said = world.says(
        "alice",
        &["org", "invite", "acme", "Dan", "--role", "developer"],
        true,
    );
    assert!(
        said.contains("or open http://127.0.0.1:8000/join/?o=acme&c="),
        "{said}"
    );
}

/// `krate org join` and `krate org cancel-invite`: an invitation works once,
/// only for its login, before it expires, and not after it is cancelled --
/// each refusal in words.
#[test]
fn org_join_takes_an_invitation_once_and_only_for_its_login() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "erin"]) else {
        eprintln!(
            "skipping org_join_takes_an_invitation_once_and_only_for_its_login: no local port"
        );
        return;
    };
    world.says("alice", &["org", "create", "acme"], true);

    let code = world.invite("alice", "bob", "reviewer");
    let said = world.says("erin", &["org", "join", "acme", &code], false);
    assert!(
        said.contains("that invitation is for a different GitHub account, not erin")
            && said.contains("krate account login"),
        "{said}"
    );
    let said = world.says("bob", &["org", "join", "acme", &code], true);
    assert!(
        said.contains("you joined acme as reviewer, signed in as bob (GitHub)"),
        "{said}"
    );
    let said = world.says("bob", &["org", "join", "acme", &code], false);
    assert!(said.contains("that invitation was already used"), "{said}");

    let code = world.invite("alice", "erin", "developer");
    world.hub.lock().unwrap().now += 8 * 86_400;
    let said = world.says("erin", &["org", "join", "acme", &code], false);
    assert!(
        said.contains("that invitation has run out (they last 7 days)"),
        "{said}"
    );

    let code = world.invite("alice", "erin", "developer");
    let said = world.says("alice", &["org", "cancel-invite", "acme", &code], true);
    assert!(said.contains("cancelled that invitation to acme"), "{said}");
    let said = world.says("erin", &["org", "join", "acme", &code], false);
    assert!(said.contains("that invitation was cancelled"), "{said}");
}

/// `krate org role`: a role changes; the last owner cannot be demoted.
#[test]
fn org_role_changes_a_role_and_never_leaves_no_owner() {
    let Some(world) = OrgWorld::new(&["alice", "bob"]) else {
        eprintln!("skipping org_role_changes_a_role_and_never_leaves_no_owner: no local port");
        return;
    };
    world.acme();
    let said = world.says("alice", &["org", "role", "acme", "bob", "admin"], true);
    assert!(
        said.contains("bob (GitHub) is now admin in acme."),
        "{said}"
    );
    let said = world.says("bob", &["org", "role", "acme", "bob", "owner"], false);
    assert!(
        said.contains("your role in acme cannot make bob (GitHub) an owner: ask an owner of acme"),
        "{said}"
    );
    let said = world.says("alice", &["org", "role", "acme", "alice", "admin"], false);
    assert!(
        said.contains("that would leave acme with no owner, so nothing changed")
            && said.contains("krate org role acme <login> owner"),
        "{said}"
    );
}

/// `krate org remove`: a removed member is told so on their next command;
/// a reviewer cannot remove others; the last owner cannot leave.
#[test]
fn org_remove_ends_membership_and_keeps_an_owner() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "carol"]) else {
        eprintln!("skipping org_remove_ends_membership_and_keeps_an_owner: no local port");
        return;
    };
    world.acme();
    let said = world.says("bob", &["org", "remove", "acme", "carol"], false);
    assert!(
        said.contains(
            "your role in acme cannot remove carol (GitHub): ask an owner or admin of acme"
        ),
        "{said}"
    );
    let said = world.says("alice", &["org", "remove", "acme", "carol"], true);
    assert!(
        said.contains("removed carol (GitHub) from acme.") && said.contains("untouched"),
        "{said}"
    );
    let said = world.says("carol", &["org", "show", "acme"], false);
    assert!(said.contains("you were removed from acme"), "{said}");

    let said = world.says("bob", &["org", "remove", "acme", "bob"], true);
    assert!(said.contains("you left acme."), "{said}");
    let said = world.says("alice", &["org", "remove", "acme", "alice"], false);
    assert!(
        said.contains("that would leave acme with no owner"),
        "{said}"
    );
}

/// `krate org root-key`: only the public half of a root key file leaves the
/// machine; a non-owner is refused; a second key is a recorded rotation.
#[test]
fn org_root_key_sends_only_the_public_half() {
    let Some(world) = OrgWorld::new(&["alice", "bob"]) else {
        eprintln!("skipping org_root_key_sends_only_the_public_half: no local port");
        return;
    };
    let root = world.acme();
    let private = std::fs::read(&root).expect("root");
    let public: String = krate_bundle::signing::SigningKey::from_pkcs8(&private)
        .expect("key")
        .public_key()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    {
        let hub = world.hub.lock().unwrap();
        assert_eq!(hub.orgs["acme"].root.as_deref(), Some(public.as_str()));
        // What was sent is the public key and nothing else.
        let sent = hub
            .seen
            .iter()
            .zip(&hub.bodies)
            .find(|((method, path, _), _)| method == "PUT" && path == "/orgs/acme/root-key")
            .map(|(_, body)| serde_json::from_slice::<serde_json::Value>(body).unwrap())
            .expect("the root key was sent");
        assert_eq!(sent, serde_json::json!({ "public_key": public }));
    }
    let said = world.says(
        "bob",
        &["org", "root-key", "acme", root.to_str().unwrap()],
        false,
    );
    assert!(
        said.contains("your role in acme cannot set acme's root key: ask an owner of acme"),
        "{said}"
    );
    // A public key file: 64 hex characters.
    let next = world.dir.path().join("next.pub");
    std::fs::write(&next, format!("{}\n", "ab".repeat(32))).expect("pub");
    let said = world.says(
        "alice",
        &["org", "root-key", "acme", next.to_str().unwrap()],
        true,
    );
    assert!(
        said.contains("acme's root key is now abababababab"),
        "{said}"
    );
    assert!(said.contains("replaced the previous key"), "{said}");
    // In full, here and in `krate org show`: a machine policy's `[trust]
    // roots` takes the whole 64-hex key (IC-949 P2).
    let full = "ab".repeat(32);
    assert!(
        said.contains(&format!("acme's root key is now {full}\n")),
        "{said}"
    );
    let shown = world.says("alice", &["org", "show", "acme"], true);
    assert!(
        shown
            .lines()
            .any(|l| l.trim() == format!("root key {full}")),
        "{shown}"
    );
}

/// `krate publish --org`: an unsigned bundle is turned back before anything
/// is uploaded, with the exact commands that sign it.
#[test]
fn publish_to_an_org_refuses_an_unsigned_bundle_before_uploading() {
    let Some(world) = OrgWorld::new(&["alice", "carol"]) else {
        eprintln!(
            "skipping publish_to_an_org_refuses_an_unsigned_bundle_before_uploading: no local port"
        );
        return;
    };
    world.acme();
    let bundle = world.bundle("plain.krate", "1.0.0", 64);
    let before = world.hub.lock().unwrap().seen.len();
    let said = world.says(
        "carol",
        &["publish", bundle.to_str().unwrap(), "--org", "acme"],
        false,
    );
    assert!(
        said.contains("is not signed or marked for acme, so nothing was uploaded"),
        "{said}"
    );
    assert!(
        said.contains(
            "krate delegate --root <acme-root.pkcs8> --key <release.pkcs8> --namespace 'acme/*'"
        ),
        "{said}"
    );
    assert!(
        said.contains(&format!(
            "krate sign {} --org acme --key <release.pkcs8> --delegation acme-release.json",
            bundle.display()
        )),
        "the exact line that fixes it: {said}"
    );
    // Krate may ask whether the organisation signs by itself (a GET);
    // nothing is sent.
    assert!(
        world.hub.lock().unwrap().seen[before..]
            .iter()
            .all(|(method, _, _)| method == "GET"),
        "nothing was sent"
    );
}

/// `krate publish --org`: a signed bundle becomes a candidate, its digest
/// printed with the approve command; a bundle signed by a key acme's root
/// never delegated is refused in words; a reviewer cannot publish.
#[test]
fn publish_to_an_org_uploads_a_signed_candidate_and_prints_its_digest() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "carol"]) else {
        eprintln!("skipping publish_to_an_org_uploads_a_signed_candidate_and_prints_its_digest: no local port");
        return;
    };
    let root = world.acme();
    let bundle = world.bundle("app.krate", "1.0.0", 64);
    world.sign_delegated(&root, "carol", &bundle);
    let said = world.says(
        "carol",
        &["publish", bundle.to_str().unwrap(), "--org", "acme"],
        true,
    );
    let digest = OrgWorld::digest_in(&said);
    let sha = format!("{:x}", Sha256::digest(std::fs::read(&bundle).unwrap()));
    assert_eq!(digest, sha, "the digest is the bytes' sha256");
    assert!(said.contains("waits for approval"), "{said}");
    assert!(
        said.contains(&format!("krate org approve acme {sha}")),
        "{said}"
    );
    assert!(said.contains("app     Acme Demo 1.0.0"), "{said}");
    assert_eq!(
        world.hub.lock().unwrap().orgs["acme"].candidates[&sha].state,
        "waiting-for-approval",
        "nothing is released by uploading"
    );

    // Signed, but by a key of its own: not acme's.
    let stray = world.bundle("stray.krate", "1.0.1", 64);
    let key = world.dir.path().join("stray.pkcs8");
    world.says(
        "carol",
        &[
            "sign",
            stray.to_str().unwrap(),
            "--key",
            key.to_str().unwrap(),
            "--generate-key",
            "--org",
            "acme",
        ],
        true,
    );
    let said = world.says(
        "carol",
        &["publish", stray.to_str().unwrap(), "--org", "acme"],
        false,
    );
    assert!(
        said.contains("acme did not take it: this app is not signed for acme"),
        "{said}"
    );
    let said = world.says(
        "bob",
        &["publish", bundle.to_str().unwrap(), "--org", "acme"],
        false,
    );
    assert!(
        said.contains("your role in acme cannot publish to acme: ask an owner or admin of acme to make you a developer"),
        "{said}"
    );
}

/// `krate org root-key --revocations`: the list `krate revoke` wrote goes
/// to the hub with the root key; a candidate whose key was withdrawn after
/// upload cannot be approved, and the withdrawn key cannot upload again --
/// each refusal in words that say why.
#[test]
fn a_withdrawn_release_key_can_neither_upload_nor_be_approved() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "carol"]) else {
        eprintln!(
            "skipping a_withdrawn_release_key_can_neither_upload_nor_be_approved: no local port"
        );
        return;
    };
    let root = world.acme();
    let bundle = world.bundle("app.krate", "1.0.0", 64);
    world.sign_delegated(&root, "carol", &bundle);
    let digest = OrgWorld::digest_in(&world.says(
        "carol",
        &["publish", bundle.to_str().unwrap(), "--org", "acme"],
        true,
    ));
    let release = world.dir.path().join("carol-release.pkcs8");
    let list = world.dir.path().join("acme-revocations.json");
    world.says(
        "alice",
        &[
            "revoke",
            "--root",
            root.to_str().unwrap(),
            "--key",
            release.to_str().unwrap(),
            "--reason",
            "laptop lost",
            "-o",
            list.to_str().unwrap(),
        ],
        true,
    );
    let said = world.says(
        "alice",
        &[
            "org",
            "root-key",
            "acme",
            root.to_str().unwrap(),
            "--revocations",
            list.to_str().unwrap(),
        ],
        true,
    );
    assert!(said.contains("Withdrew 1 release key"), "{said}");
    let said = world.says("bob", &["org", "approve", "acme", &digest], false);
    assert!(
        said.contains(
            "nothing was approved: this app is not signed for acme: its release key was withdrawn"
        ) && said.contains("sign it again with a current release key"),
        "{said}"
    );
    let next = world.bundle("app-v2.krate", "1.0.1", 64);
    world.sign_delegated(&root, "carol", &next);
    let said = world.says(
        "carol",
        &["publish", next.to_str().unwrap(), "--org", "acme"],
        false,
    );
    assert!(
        said.contains(
            "acme did not take it: this app is not signed for acme: its release key was withdrawn"
        ),
        "{said}"
    );
    // A list from some other root is turned back before it is sent.
    let stranger = world.dir.path().join("other-root.pkcs8");
    std::fs::write(
        &stranger,
        krate_bundle::signing::SigningKey::generate_pkcs8().expect("root"),
    )
    .expect("key");
    let said = world.says(
        "alice",
        &[
            "org",
            "root-key",
            "acme",
            stranger.to_str().unwrap(),
            "--revocations",
            list.to_str().unwrap(),
        ],
        false,
    );
    assert!(said.contains("was signed by a different root"), "{said}");
}

/// `krate publish --org` over the publish door's 5 MiB: the candidate goes
/// through the big-file path, carrying the organisation, and still prints
/// its digest and waits for approval.
#[test]
fn a_big_candidate_goes_through_the_big_file_path() {
    let Some(world) = OrgWorld::new(&["alice", "carol"]) else {
        eprintln!("skipping a_big_candidate_goes_through_the_big_file_path: no local port");
        return;
    };
    let root = world.acme();
    let bundle = world.bundle("big.krate", "2.0.0", 6 * 1024 * 1024);
    world.sign_delegated(&root, "carol", &bundle);
    let said = world.says(
        "carol",
        &["publish", bundle.to_str().unwrap(), "--org", "acme"],
        true,
    );
    let sha = format!("{:x}", Sha256::digest(std::fs::read(&bundle).unwrap()));
    assert_eq!(OrgWorld::digest_in(&said), sha);
    let hub = world.hub.lock().unwrap();
    assert!(
        !hub.seen
            .iter()
            .any(|(_, path, _)| path == "/orgs/acme/candidates"),
        "never the one-request door"
    );
    let start = hub
        .seen
        .iter()
        .zip(&hub.bodies)
        .find(|((_, path, _), _)| path == "/publish/big/start")
        .map(|(_, body)| serde_json::from_slice::<serde_json::Value>(body).unwrap())
        .expect("the big-file path started");
    assert_eq!(start["org"], "acme", "the start names the organisation");
    assert_eq!(start["sha256"], sha.as_str());
    assert_eq!(
        hub.orgs["acme"].candidates[&sha].state,
        "waiting-for-approval"
    );
}

/// `krate org approve`: the uploader cannot approve their own upload; a
/// recipient cannot approve; a digest that is not the current candidate
/// approves nothing; a reviewer's approval releases exactly that digest.
#[test]
fn org_approve_releases_the_exact_digest_and_never_your_own() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "carol", "dan"]) else {
        eprintln!(
            "skipping org_approve_releases_the_exact_digest_and_never_your_own: no local port"
        );
        return;
    };
    let root = world.acme();
    let bundle = world.bundle("app.krate", "1.0.0", 64);
    world.sign_delegated(&root, "carol", &bundle);
    let digest = OrgWorld::digest_in(&world.says(
        "carol",
        &["publish", bundle.to_str().unwrap(), "--org", "acme"],
        true,
    ));
    let said = world.says("carol", &["org", "approve", "acme", &digest], false);
    assert!(
        said.contains("you can't approve your own upload: ask a reviewer in acme"),
        "{said}"
    );
    let said = world.says("dan", &["org", "approve", "acme", &digest], false);
    assert!(
        said.contains("your role in acme cannot approve uploads: ask a reviewer in acme"),
        "a recipient has no approve authority: {said}"
    );
    let other = "ab".repeat(32);
    let said = world.says("bob", &["org", "approve", "acme", &other], false);
    assert!(
        said.contains("nothing was approved: acme has no candidate with the digest abababababab"),
        "{said}"
    );
    assert_eq!(
        world.hub.lock().unwrap().orgs["acme"].candidates[&digest].state,
        "waiting-for-approval"
    );
    let said = world.says("bob", &["org", "approve", "acme", &digest], true);
    assert!(
        said.contains(&format!(
            "approved. {} is now a release of acme.",
            &digest[..12]
        )),
        "{said}"
    );
    assert!(
        said.contains(&format!("krate run {}/a/{digest}", world.url())),
        "{said}"
    );
    let hub = world.hub.lock().unwrap();
    assert_eq!(hub.orgs["acme"].candidates[&digest].state, "released");
    assert_eq!(
        hub.orgs["acme"].candidates[&digest].approver.as_deref(),
        Some("github:bob")
    );
}

/// `krate org releases`: each release with who uploaded and who approved.
#[test]
fn org_releases_lists_who_uploaded_and_who_approved() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "carol", "dan"]) else {
        eprintln!("skipping org_releases_lists_who_uploaded_and_who_approved: no local port");
        return;
    };
    let root = world.acme();
    let said = world.says("dan", &["org", "releases", "acme"], true);
    assert!(said.contains("acme has no releases yet"), "{said}");
    let bundle = world.bundle("app.krate", "1.0.0", 64);
    world.sign_delegated(&root, "carol", &bundle);
    let digest = OrgWorld::digest_in(&world.says(
        "carol",
        &["publish", bundle.to_str().unwrap(), "--org", "acme"],
        true,
    ));
    world.says("bob", &["org", "approve", "acme", &digest], true);
    let said = world.says("dan", &["org", "releases", "acme"], true);
    assert!(said.contains("acme: 1 release"), "{said}");
    assert!(
        said.contains(&format!("Acme Demo 1.0.0  {}", &digest[..12])),
        "{said}"
    );
    assert!(
        said.contains("uploaded by carol (GitHub), approved by bob (GitHub), 2025-10-14"),
        "{said}"
    );

    // `krate org candidates`: the history, for those who publish or approve.
    let said = world.says("carol", &["org", "candidates", "acme"], true);
    assert!(said.contains("acme: 1 candidate"), "{said}");
    assert!(
        said.contains(&format!("Acme Demo 1.0.0  {digest}  released")),
        "{said}"
    );
    assert!(
        said.contains("uploaded by carol (GitHub) 2025-10-14 00:00 UTC, approved by bob (GitHub)"),
        "{said}"
    );
    let said = world.says("dan", &["org", "candidates", "acme"], false);
    assert!(
        said.contains("your role in acme cannot list acme's candidates"),
        "{said}"
    );
}

/// `krate org audit`: every event and every denial, the chain's verdict;
/// a tampered event fails the command; a developer cannot read it.
#[test]
fn org_audit_shows_every_event_and_a_broken_chain_fails() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "carol"]) else {
        eprintln!("skipping org_audit_shows_every_event_and_a_broken_chain_fails: no local port");
        return;
    };
    world.acme();
    world.says("bob", &["org", "remove", "acme", "carol"], false);
    let said = world.says("alice", &["org", "audit", "acme"], true);
    for action in ["create", "root-key", "invite", "accept"] {
        assert!(said.contains(action), "{action}: {said}");
    }
    assert!(said.contains("denied: role-cannot"), "{said}");
    assert!(said.contains("chain intact"), "{said}");
    let said = world.says("carol", &["org", "audit", "acme"], false);
    assert!(
        said.contains("your role in acme cannot read acme's audit: ask an owner or admin of acme"),
        "{said}"
    );
    world
        .hub
        .lock()
        .unwrap()
        .orgs
        .get_mut("acme")
        .unwrap()
        .audit[1]["actor"] = serde_json::json!("mallory");
    let said = world.says("alice", &["org", "audit", "acme"], false);
    assert!(said.contains("THE CHAIN IS BROKEN"), "{said}");
}

/// Every mutation carries an Idempotency-Key; a key the hub saw with a
/// different request, and a token the hub does not know, come back in words.
#[test]
fn org_commands_carry_an_idempotency_key_and_hub_refusals_are_worded() {
    let Some(world) = OrgWorld::new(&["alice", "bob"]) else {
        eprintln!("skipping org_commands_carry_an_idempotency_key_and_hub_refusals_are_worded: no local port");
        return;
    };
    world.acme();
    {
        let hub = world.hub.lock().unwrap();
        assert!(
            hub.keyless_mutations.is_empty(),
            "{:?}",
            hub.keyless_mutations
        );
        assert!(
            hub.keys.len() >= 4,
            "one key per mutation: {}",
            hub.keys.len()
        );
    }
    world.hub.lock().unwrap().force_next = Some((409, "idempotency-key-reused".into()));
    let said = world.says(
        "alice",
        &["org", "invite", "acme", "erin", "--role", "developer"],
        false,
    );
    assert!(
        said.contains("under this command's key, so nothing changed: run the command again"),
        "{said}"
    );
    for (status, reason, words) in [
        (422, "invalid-bundle", "could not read this as a Krate app"),
        (
            409,
            "bytes-held-elsewhere",
            "already on the hub outside acme",
        ),
        (413, "too-large", "would not take it in one request"),
        (500, "store-failed", "could not store it"),
        (400, "bad-request", "could not read the request"),
        (
            503,
            "orgs-off",
            "organisations are switched off on this hub",
        ),
    ] {
        world.hub.lock().unwrap().force_next = Some((status, reason.into()));
        let said = world.says("bob", &["org", "show", "acme"], false);
        assert!(said.contains(words), "{reason}: {said}");
    }
    world.hub.lock().unwrap().force_next = Some((401, "not-signed-in".into()));
    let said = world.says("bob", &["org", "show", "acme"], false);
    assert!(
        said.contains("you are not signed in: run `krate account login`, then run this again"),
        "{said}"
    );
    // Signed out on this machine: refused before anything is sent.
    let before = world.hub.lock().unwrap().seen.len();
    let said = world.says("nobody", &["org", "show", "acme"], false);
    assert!(said.contains("you are not signed in"), "{said}");
    assert_eq!(world.hub.lock().unwrap().seen.len(), before);
}

/// `krate run <private link>`: a member's session opens it; with no
/// session, as a stranger, and once removed, nothing runs and the person is
/// told why. The session goes only to the hub, never to another host.
#[test]
fn a_private_link_runs_only_with_a_current_members_session() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "carol", "dan", "erin"]) else {
        eprintln!(
            "skipping a_private_link_runs_only_with_a_current_members_session: no local port"
        );
        return;
    };
    let root = world.acme();
    let bundle = world.bundle("app.krate", "1.0.0", 64);
    world.sign_delegated(&root, "carol", &bundle);
    let digest = OrgWorld::digest_in(&world.says(
        "carol",
        &["publish", bundle.to_str().unwrap(), "--org", "acme"],
        true,
    ));
    world.says("bob", &["org", "approve", "acme", &digest], true);
    let link = format!("{}/a/{digest}", world.url());

    let opened = world.run_link("dan", &link);
    assert!(
        opened.status.success(),
        "{}",
        String::from_utf8_lossy(&opened.stderr)
    );
    assert_eq!(world.hub.lock().unwrap().served, 1);

    let copied = world.run_link("nobody", &link);
    let said = String::from_utf8_lossy(&copied.stderr);
    assert!(!copied.status.success());
    assert!(
        said.contains("you are not signed in, so nothing was downloaded or run")
            && said.contains("krate account login"),
        "{said}"
    );
    let stranger = world.run_link("erin", &link);
    let said = String::from_utf8_lossy(&stranger.stderr);
    assert!(!stranger.status.success());
    assert!(
        said.contains("this app is private to the organisation that sent it, and erin is not one of its members"),
        "{said}"
    );

    world.says(
        "alice",
        &["org", "remove", "acme", "dan@acme.example"],
        true,
    );
    let removed = world.run_link("dan", &link);
    let said = String::from_utf8_lossy(&removed.stderr);
    assert!(!removed.status.success());
    assert!(
        said.contains("you were removed from acme, so this app no longer opens for you"),
        "{said}"
    );
    assert_eq!(
        world.hub.lock().unwrap().served,
        1,
        "only Dan's first run was served"
    );

    // A hub that is not this machine's: the session is not sent to it.
    let home = world.homes["bob"].path();
    let elsewhere = krate()
        .args([
            "run",
            &link,
            "--insecure-http",
            "--auto-grant",
            "--headless",
        ])
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("KRATE_HUB_URL", "https://hub.example")
        .output()
        .expect("run");
    let said = String::from_utf8_lossy(&elsewhere.stderr);
    assert!(!elsewhere.status.success());
    assert!(said.contains("Krate did not send your sign-in"), "{said}");
    let hub = world.hub.lock().unwrap();
    let (_, _, auth) = hub.seen.last().expect("a request");
    assert!(
        auth.is_none(),
        "no session went to a host that is not the hub"
    );
}

/// Addendum 1: an email account whose address starts with a member's
/// GitHub login is not that member. `alice@corp.example` signed in by email
/// cannot see acme, act for Alice, accept an invitation made out to
/// `github:` someone, or open acme's apps.
#[test]
fn an_email_named_after_a_github_member_is_not_that_member() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "carol", "alice-mail"]) else {
        eprintln!(
            "skipping an_email_named_after_a_github_member_is_not_that_member: no local port"
        );
        return;
    };
    let root = world.acme();
    let said = world.says("alice-mail", &["org", "show", "acme"], false);
    assert!(
        said.contains("alice@corp.example is not a member of acme")
            && said.contains("--email alice@corp.example"),
        "{said}"
    );
    let said = world.says("alice-mail", &["org", "remove", "acme", "bob"], false);
    assert!(said.contains("is not a member of acme"), "{said}");
    // An invitation for github:erin is not accepted by erin@... either.
    let code = world.invite("alice", "erin", "developer");
    let said = world.says("alice-mail", &["org", "join", "acme", &code], false);
    assert!(
        said.contains("that invitation is for a different GitHub account"),
        "{said}"
    );

    let bundle = world.bundle("app.krate", "1.0.0", 64);
    world.sign_delegated(&root, "carol", &bundle);
    let digest = OrgWorld::digest_in(&world.says(
        "carol",
        &["publish", bundle.to_str().unwrap(), "--org", "acme"],
        true,
    ));
    world.says("bob", &["org", "approve", "acme", &digest], true);
    let link = format!("{}/a/{digest}", world.url());
    let opened = world.run_link("alice-mail", &link);
    assert!(!opened.status.success());
    assert!(
        String::from_utf8_lossy(&opened.stderr)
            .contains("alice@corp.example is not one of its members"),
        "{}",
        String::from_utf8_lossy(&opened.stderr)
    );
    // Alice herself, by GitHub, still opens it.
    assert!(world.run_link("alice", &link).status.success());
    // Members are addressed by principal: the email one is not Alice's.
    if world.real_hub().is_none() {
        let hub = world.hub.lock().unwrap();
        let acme = &hub.orgs["acme"];
        assert_eq!(acme.role_of("github:alice"), Some("owner"));
        assert_eq!(acme.role_of("email:alice@corp.example"), None);
        assert!(
            hub.seen
                .iter()
                .any(|(m, p, _)| m == "DELETE" && p == "/orgs/acme/members/github%3Abob"),
            "a member is addressed by principal, URL-encoded"
        );
    }
}

/// IC-946's proof, end to end through the CLI against the fake hub: Alice
/// creates acme and registers its root key; Bob (reviewer), Carol
/// (developer) and Dan (recipient) join; Carol publishes a delegated-key
/// candidate and cannot approve it; Bob approves its exact digest; Dan runs
/// the private link, a stranger and a copied link cannot; new bytes are not
/// released and approving the old digest does not release them; Alice
/// removes Dan and his next run is refused while the release and Carol's
/// history stay; two racing removals of the last two owners leave one; the
/// audit lists every step and denial with its chain intact.
#[test]
fn the_org_pilot_proof_runs_end_to_end_through_the_cli() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "carol", "dan", "erin"]) else {
        eprintln!("skipping the_org_pilot_proof_runs_end_to_end_through_the_cli: no local port");
        return;
    };
    // 1. Create, register the root key, invite, join. Dan is invited by his
    //    work email and joins with an email sign-in.
    let root = world.acme();
    let said = world.says("alice", &["org", "show", "acme"], true);
    assert!(said.contains("dan@acme.example (email)"), "{said}");
    assert!(said.contains("bob (GitHub)"), "{said}");

    // 2. Carol's candidate, signed by a release key acme's root delegated.
    let bundle = world.bundle("app.krate", "1.0.0", 64);
    world.sign_delegated(&root, "carol", &bundle);
    let first = OrgWorld::digest_in(&world.says(
        "carol",
        &["publish", bundle.to_str().unwrap(), "--org", "acme"],
        true,
    ));
    let said = world.says("carol", &["org", "approve", "acme", &first], false);
    assert!(
        said.contains("you can't approve your own upload: ask a reviewer in acme"),
        "{said}"
    );
    let said = world.says("bob", &["org", "approve", "acme", &first], true);
    assert!(said.contains("is now a release of acme"), "{said}");
    let link = format!("{}/a/{first}", world.url());

    // 3. Dan opens it; a stranger and a copied link do not.
    assert!(world.run_link("dan", &link).status.success());
    assert!(!world.run_link("erin", &link).status.success());
    assert!(!world.run_link("nobody", &link).status.success());

    // 4. New bytes are a new candidate, not a release; approving the old
    //    digest does nothing to it.
    let changed = world.bundle("app-v2.krate", "1.0.1", 64);
    world.sign_delegated(&root, "carol", &changed);
    let second = OrgWorld::digest_in(&world.says(
        "carol",
        &["publish", changed.to_str().unwrap(), "--org", "acme"],
        true,
    ));
    assert_ne!(first, second);
    // Approving the old digest again answers for the old digest only.
    world.says("bob", &["org", "approve", "acme", &first], true);
    let said = world.says("dan", &["org", "releases", "acme"], true);
    assert!(
        said.contains("acme: 1 release") && !said.contains(&second[..12]),
        "{said}"
    );
    let said = world.says("carol", &["org", "candidates", "acme"], true);
    assert!(
        said.lines()
            .any(|l| l.contains(&second) && l.contains("waiting for approval")),
        "{said}"
    );
    let unreleased = world.run_link("dan", &format!("{}/a/{second}", world.url()));
    assert!(!unreleased.status.success(), "a candidate is never served");
    assert!(
        String::from_utf8_lossy(&unreleased.stderr).contains("has not approved"),
        "{}",
        String::from_utf8_lossy(&unreleased.stderr)
    );

    // 5. Alice removes Dan: his next run is refused; the release and
    //    Carol's history remain.
    world.says(
        "alice",
        &["org", "remove", "acme", "dan@acme.example"],
        true,
    );
    let refused = world.run_link("dan", &link);
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("you were removed from"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    let said = world.says("alice", &["org", "releases", "acme"], true);
    assert!(
        said.contains("uploaded by carol (GitHub), approved by bob (GitHub)"),
        "{said}"
    );

    // 6. Bob becomes a second owner; Alice and Bob each leave at once.
    world.says("alice", &["org", "role", "acme", "bob", "owner"], true);
    let (a, b) = std::thread::scope(|scope| {
        let a = scope.spawn(|| world.krate_as("alice", &["org", "remove", "acme", "alice"]));
        let b = scope.spawn(|| world.krate_as("bob", &["org", "remove", "acme", "bob"]));
        (a.join().unwrap(), b.join().unwrap())
    });
    let outcomes = [&a, &b].map(|o| {
        (
            o.status.success(),
            format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            ),
        )
    });
    assert_eq!(
        outcomes.iter().filter(|(ok, _)| *ok).count(),
        1,
        "exactly one leaves: {outcomes:?}"
    );
    let (_, lost) = outcomes.iter().find(|(ok, _)| !ok).unwrap();
    assert!(
        lost.contains("that would leave acme with no owner"),
        "{lost}"
    );
    let owner = if a.status.success() { "bob" } else { "alice" };
    if world.real_hub().is_none() {
        assert_eq!(world.hub.lock().unwrap().orgs["acme"].owners(), 1);
    } else {
        // The real hub's store is out of reach here; the one who stayed is
        // still an owner, and the one who left is no longer a member.
        let left = if owner == "bob" { "alice" } else { "bob" };
        let said = world.says(owner, &["org", "show", "acme"], true);
        assert!(said.contains(owner), "{said}");
        world.says(left, &["org", "show", "acme"], false);
    }

    // 7. The audit lists every step and denial, and the chain holds.
    let said = world.says(owner, &["org", "audit", "acme"], true);
    for step in [
        "create",
        "root-key",
        "invite",
        "accept",
        "publish",
        "approve",
        "download",
        "remove",
        "role",
        "denied: self-approval",
        "denied: digest-mismatch",
        "allowed",
        "denied: not-a-member",
        "denied: removed",
        "denied: last-owner",
    ] {
        assert!(said.contains(step), "the audit lacks {step}: {said}");
    }
    assert!(said.contains("chain intact"), "{said}");
    if world.real_hub().is_some() {
        // Tampering with the real hub's stored events is the hub's own
        // proof (cloud/worker/test, and the wrangler run's tamper step).
        eprintln!("skipping the tamper step: KRATE_ORG_E2E_HUB is a real hub");
        return;
    }
    world
        .hub
        .lock()
        .unwrap()
        .orgs
        .get_mut("acme")
        .unwrap()
        .audit[3]["target"] = serde_json::json!("someone-else");
    let said = world.says(owner, &["org", "audit", "acme"], false);
    assert!(said.contains("THE CHAIN IS BROKEN"), "{said}");
}

// ---------------------------------------------------------------------------
// Offline access and the desktop doors (IC-948, Addendum 3 G and H).
// ---------------------------------------------------------------------------

/// The same command as `OrgWorld::krate_as`, before it runs, so a test can
/// change one thing about it (the lease key override, the hub).
fn org_command(world: &OrgWorld, who: &str, args: &[&str]) -> Command {
    let home = world.homes[who].path();
    let mut command = krate();
    command
        .args(args)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("KRATE_HUB_URL", world.url())
        .env("KRATE_CACHE_DIR", home.join("cache"))
        .env("KRATE_NO_OPEN_CACHE", "1")
        .env(krate_lease::LEASE_KEY_OVERRIDE_ENV, world.lease_key_hex());
    command
}

fn stderr_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

/// Dan's own copy of the app, as the dashboard's Open in Krate hands it to
/// him: a file on his machine, never fetched by Krate.
fn dans_copy(world: &OrgWorld, bundle: &std::path::Path) -> PathBuf {
    let copy = world.homes["dan"].path().join("Acme Demo.krate");
    std::fs::copy(bundle, &copy).expect("copy");
    copy
}

/// G: a private app opens offline only on a stored lease for its digest and
/// its person that verifies under the pinned key and has not run out. The
/// first open shows the terms with the exact date; a lease past half its
/// life is refreshed on the next online open.
#[test]
fn a_private_app_opens_offline_only_on_a_lease_that_holds() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "carol", "dan"]) else {
        eprintln!("skipping a_private_app_opens_offline_only_on_a_lease_that_holds: no local port");
        return;
    };
    let root = world.acme();
    let (bundle, digest, link) = world.released_app(&root, "1.0.0");
    let copy = dans_copy(&world, &bundle);
    let run_copy = |who: &str| {
        org_command(
            &world,
            who,
            &["run", copy.to_str().unwrap(), "--headless", "--auto-grant"],
        )
        .output()
        .expect("run")
    };

    // First open, online, through the link. The lease clock is four days
    // back, so this first 7-day lease is already past half its life.
    world.hub.lock().unwrap().lease_shift = -4 * 86_400;
    let first = world.run_link("dan", &link);
    let said = stderr_of(&first);
    assert!(first.status.success(), "{said}");
    let record = world.lease_record("dan", &digest).expect("a lease is kept");
    let expires = record["answer"]["lease"]["expires_at"]
        .as_u64()
        .expect("expiry");
    let day = |secs: u64| {
        let days = (secs / 86_400) as i64;
        // The same civil date the CLI prints.
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = yoe + era * 400 + i64::from(m <= 2);
        format!("{y:04}-{m:02}-{d:02}")
    };
    assert!(
        said.contains(&format!("Works offline until {}", day(expires)))
            && said.contains("If you are removed, it stops when it next connects."),
        "the terms, with the exact date, before the first open: {said}"
    );
    assert_eq!(record["class"], "days-7");
    assert_eq!(record["principal"], "email:dan@acme.example");
    assert_eq!(world.hub.lock().unwrap().leases_issued, 1);

    // Online again, from his own copy: past half its life, so refreshed;
    // the terms are not shown twice.
    world.hub.lock().unwrap().lease_shift = 0;
    let second = run_copy("dan");
    assert!(second.status.success(), "{}", stderr_of(&second));
    assert!(!stderr_of(&second).contains("Works offline until"));
    assert_eq!(world.hub.lock().unwrap().leases_issued, 2, "refreshed");
    let third = run_copy("dan");
    assert!(third.status.success());
    assert_eq!(world.hub.lock().unwrap().leases_issued, 2, "fresh: kept");

    // Offline: the hub is gone. The lease holds, so it opens.
    world.stop.store(true, std::sync::atomic::Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(100));
    let offline = run_copy("dan");
    let said = stderr_of(&offline);
    assert!(offline.status.success(), "{said}");
    assert!(
        said.contains("offline: opening Acme's app on its offline access"),
        "{said}"
    );

    // Without the override, this Krate checks against the pinned production
    // key, which never signed this test hub's lease: nothing opens offline.
    let unkeyed = org_command(
        &world,
        "dan",
        &["run", copy.to_str().unwrap(), "--headless"],
    )
    .env_remove(krate_lease::LEASE_KEY_OVERRIDE_ENV)
    .output()
    .expect("run");
    assert!(!unkeyed.status.success());
    assert!(
        stderr_of(&unkeyed).contains("could not be checked")
            && stderr_of(&unkeyed).contains("its signature does not hold"),
        "{}",
        stderr_of(&unkeyed)
    );

    // A lease that ran out, signed by the real key: refused, with its date.
    let lease_file = world.homes["dan"]
        .path()
        .join(".krate/leases")
        .join(format!("{digest}.json"));
    let mut record: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&lease_file).unwrap()).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let mut body = record["answer"]["lease"].clone();
    body["issued_at"] = serde_json::json!(now - 8 * 86_400);
    body["expires_at"] = serde_json::json!(now - 86_400);
    let pkcs8 = world.hub.lock().unwrap().lease_pkcs8.clone();
    record["answer"] = krate_lease::sign(&body, &pkcs8).unwrap();
    std::fs::write(&lease_file, record.to_string()).unwrap();
    let expired = run_copy("dan");
    assert!(!expired.status.success());
    assert!(
        stderr_of(&expired).contains(&format!(
            "its offline access ran out on {}",
            day(now - 86_400)
        )),
        "{}",
        stderr_of(&expired)
    );

    // A lease with a later date that the hub never signed.
    body["expires_at"] = serde_json::json!(now + 86_400);
    let forged_key = krate_lease::generate_pkcs8().unwrap();
    record["answer"] = krate_lease::sign(&body, &forged_key).unwrap();
    std::fs::write(&lease_file, record.to_string()).unwrap();
    let forged = run_copy("dan");
    assert!(!forged.status.success());
    assert!(
        stderr_of(&forged).contains("its signature does not hold"),
        "{}",
        stderr_of(&forged)
    );

    // Signed out, offline: it does not open either.
    record["answer"] = krate_lease::sign(&body, &pkcs8).unwrap();
    std::fs::write(&lease_file, record.to_string()).unwrap();
    std::fs::remove_file(world.homes["dan"].path().join(".krate/github.json")).unwrap();
    let signed_out = run_copy("dan");
    assert!(!signed_out.status.success());
    assert!(
        stderr_of(&signed_out).contains("you are not signed in"),
        "{}",
        stderr_of(&signed_out)
    );
}

/// G: on reconnect, a refused check-in deletes the lease and nothing runs,
/// while the person's app data stays. The bytes are still known to be
/// private afterwards, so going offline does not open them again.
#[test]
fn a_refused_check_in_deletes_the_lease_and_keeps_the_data() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "carol", "dan"]) else {
        eprintln!(
            "skipping a_refused_check_in_deletes_the_lease_and_keeps_the_data: no local port"
        );
        return;
    };
    let root = world.acme();
    let (bundle, digest, link) = world.released_app(&root, "1.0.0");
    let copy = dans_copy(&world, &bundle);
    assert!(world.run_link("dan", &link).status.success());
    assert!(world.lease_record("dan", &digest).unwrap()["answer"].is_object());
    let data = world.homes["dan"].path().join(".krate/store/dev.acme.demo");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("notes.txt"), "Dan's own notes").unwrap();

    world.says(
        "alice",
        &["org", "remove", "acme", "dan@acme.example"],
        true,
    );
    let run_copy = || {
        org_command(
            &world,
            "dan",
            &["run", copy.to_str().unwrap(), "--headless"],
        )
        .output()
        .expect("run")
    };
    let refused = run_copy();
    assert!(!refused.status.success());
    assert!(
        stderr_of(&refused)
            .contains("you were removed from acme, so this app no longer opens for you"),
        "{}",
        stderr_of(&refused)
    );
    let record = world
        .lease_record("dan", &digest)
        .expect("still known as private");
    assert!(record["answer"].is_null(), "the lease is gone: {record}");
    assert_eq!(
        std::fs::read_to_string(data.join("notes.txt")).unwrap(),
        "Dan's own notes",
        "the app's data is the person's and stays (R10)"
    );

    world.stop.store(true, std::sync::atomic::Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(100));
    let offline = run_copy();
    assert!(!offline.status.success());
    assert!(
        stderr_of(&offline).contains("it opens only with a connection"),
        "{}",
        stderr_of(&offline)
    );
}

/// G: an `online` class has no lease: the terms say so, and offline it
/// does not open. And a member who never opened the app through Krate --
/// the file came from the dashboard -- is still checked in on first open.
#[test]
fn an_online_only_app_never_opens_offline() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "carol", "dan"]) else {
        eprintln!("skipping an_online_only_app_never_opens_offline: no local port");
        return;
    };
    let root = world.acme();
    let (bundle, digest, _link) = world.released_app(&root, "1.0.0");
    let copy = dans_copy(&world, &bundle);
    world.hub.lock().unwrap().lease_class = "online".into();
    let run_copy = || {
        org_command(
            &world,
            "dan",
            &["run", copy.to_str().unwrap(), "--headless"],
        )
        .output()
        .expect("run")
    };
    let first = run_copy();
    assert!(first.status.success(), "{}", stderr_of(&first));
    assert!(
        stderr_of(&first).contains("It needs a connection every time it opens."),
        "{}",
        stderr_of(&first)
    );
    assert_eq!(
        world.lease_record("dan", &digest).unwrap()["class"],
        "online"
    );

    world.stop.store(true, std::sync::atomic::Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(100));
    let offline = run_copy();
    assert!(!offline.status.success());
    assert!(
        stderr_of(&offline).contains("it opens only with a connection"),
        "{}",
        stderr_of(&offline)
    );
}

/// G: a stranger who is signed in and holds a copy of the file is refused,
/// and an ordinary signed app is asked about once and then never again.
#[test]
fn a_copied_private_file_does_not_open_for_a_stranger() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "carol", "erin"]) else {
        eprintln!("skipping a_copied_private_file_does_not_open_for_a_stranger: no local port");
        return;
    };
    let root = world.acme();
    let (bundle, digest, _link) = world.released_app(&root, "1.0.0");
    let copy = world.homes["erin"].path().join("found.krate");
    std::fs::copy(&bundle, &copy).unwrap();
    let refused = org_command(
        &world,
        "erin",
        &["run", copy.to_str().unwrap(), "--headless"],
    )
    .output()
    .expect("run");
    assert!(!refused.status.success());
    assert!(
        stderr_of(&refused).contains("erin is not one of its members"),
        "{}",
        stderr_of(&refused)
    );

    let _ = digest;
}

/// Addendum 5: an unmarked app, signed or not, opens without a single
/// request to any hub -- counted by the fake, which sees every one.
#[test]
fn an_unmarked_app_opens_with_no_hub_request() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "carol", "dan"]) else {
        eprintln!("skipping an_unmarked_app_opens_with_no_hub_request: no local port");
        return;
    };
    let root = world.acme();
    let unsigned = world.bundle("plain.krate", "1.0.0", 64);
    let signed = world.bundle("signed.krate", "1.0.1", 64);
    // Signed with the same delegated key an acme app uses -- but not marked.
    let release = world.dir.path().join("carol-release.pkcs8");
    std::fs::write(&release, krate_lease::generate_pkcs8().unwrap()).unwrap();
    let slip = world.dir.path().join("carol-acme.json");
    world.says(
        "alice",
        &[
            "delegate",
            "--root",
            root.to_str().unwrap(),
            "--key",
            release.to_str().unwrap(),
            "--namespace",
            "acme/*",
            "-o",
            slip.to_str().unwrap(),
        ],
        true,
    );
    world.says(
        "carol",
        &[
            "sign",
            signed.to_str().unwrap(),
            "--key",
            release.to_str().unwrap(),
            "--namespace",
            "acme/demo",
            "--delegation",
            slip.to_str().unwrap(),
        ],
        true,
    );
    let before = world.hub.lock().unwrap().seen.len();
    for (who, app) in [("dan", &unsigned), ("dan", &signed), ("nobody", &signed)] {
        let ran = org_command(&world, who, &["run", app.to_str().unwrap(), "--headless"])
            .output()
            .expect("run");
        assert!(ran.status.success(), "{}", stderr_of(&ran));
    }
    let hub = world.hub.lock().unwrap();
    assert_eq!(
        hub.seen.len(),
        before,
        "not one request for an unmarked app: {:?}",
        &hub.seen[before..]
    );
}

/// Addendum 5: a marked app checks in, every time it opens; one the hub has
/// no release of is refused in words.
#[test]
fn a_marked_app_checks_in_and_an_unreleased_one_is_refused() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "carol", "dan"]) else {
        eprintln!(
            "skipping a_marked_app_checks_in_and_an_unreleased_one_is_refused: no local port"
        );
        return;
    };
    let root = world.acme();
    let (bundle, digest, _link) = world.released_app(&root, "1.0.0");
    let copy = dans_copy(&world, &bundle);
    let check_ins = |world: &OrgWorld| {
        world
            .hub
            .lock()
            .unwrap()
            .seen
            .iter()
            .filter(|(m, p, _)| m == "GET" && *p == format!("/a/{digest}"))
            .count()
    };
    for round in 1..=2 {
        let ran = org_command(
            &world,
            "dan",
            &["run", copy.to_str().unwrap(), "--headless"],
        )
        .output()
        .expect("run");
        assert!(ran.status.success(), "{}", stderr_of(&ran));
        // Counted on the fake; a real hub (KRATE_ORG_E2E_HUB) keeps its own
        // log, where run-one.sh counts the requests.
        if world.real_hub().is_none() {
            assert_eq!(check_ins(&world), round, "every open checks in");
        }
    }
    // Marked and signed, but never released: refused, and says so.
    let own = world.bundle("own.krate", "9.9.9", 64);
    world.sign_delegated(&root, "carol", &own);
    let ran = org_command(
        &world,
        "carol",
        &["run", own.to_str().unwrap(), "--headless"],
    )
    .output()
    .expect("run");
    assert!(!ran.status.success());
    assert!(
        stderr_of(&ran).contains("this is acme's private app")
            && stderr_of(&ran).contains("has no release of it"),
        "{}",
        stderr_of(&ran)
    );
}

/// The zip a bundle is, rewritten without its organisation mark: the
/// mark's file gone and its declaration with it.
fn strip_mark(bundle: &std::path::Path, out: &std::path::Path) {
    use std::io::Read as _;
    let mut archive = zip::ZipArchive::new(std::fs::File::open(bundle).unwrap()).unwrap();
    let mut writer = zip::ZipWriter::new(std::fs::File::create(out).unwrap());
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let name = entry.name().to_string();
        let mut body = Vec::new();
        entry.read_to_end(&mut body).unwrap();
        if name == krate_bundle::ORG_MARK_ENTRY {
            continue;
        }
        if name == krate_bundle::EXTENSIONS_ENTRY {
            let mut record: serde_json::Value = serde_json::from_slice(&body).unwrap();
            record["extensions"]
                .as_array_mut()
                .unwrap()
                .retain(|e| e["owner"] != "krate");
            if record["extensions"].as_array().unwrap().is_empty() {
                continue;
            }
            body = serde_json::to_vec(&record).unwrap();
        }
        writer
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut writer, &body).unwrap();
    }
    writer.finish().unwrap();
}

/// Addendum 5: the signature covers the mark. Stripping it leaves a bundle
/// whose signature no longer holds -- refused as changed after signing,
/// with no hub asked -- and a mark whose signature does not hold is
/// refused in words before anything is asked either.
#[test]
fn a_stripped_or_unsigned_mark_does_not_open() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "carol", "dan"]) else {
        eprintln!("skipping a_stripped_or_unsigned_mark_does_not_open: no local port");
        return;
    };
    let root = world.acme();
    let (bundle, _digest, _link) = world.released_app(&root, "1.0.0");
    let stripped = world.homes["dan"].path().join("stripped.krate");
    strip_mark(&bundle, &stripped);
    assert!(krate_bundle::read_org_mark(&stripped).unwrap().is_none());
    let before = world.hub.lock().unwrap().seen.len();
    let ran = org_command(
        &world,
        "dan",
        &["run", stripped.to_str().unwrap(), "--headless"],
    )
    .output()
    .expect("run");
    assert!(!ran.status.success());
    assert!(
        stderr_of(&ran).contains("this app was changed after it was signed"),
        "{}",
        stderr_of(&ran)
    );
    // A mark added after signing: the signature no longer holds.
    let remarked = world.homes["dan"].path().join("remarked.krate");
    std::fs::copy(&stripped, &remarked).unwrap();
    krate_bundle::add_org_mark(&remarked, "globex").unwrap();
    let ran = org_command(
        &world,
        "dan",
        &["run", remarked.to_str().unwrap(), "--headless"],
    )
    .output()
    .expect("run");
    assert!(!ran.status.success());
    assert!(
        stderr_of(&ran).contains("marked as globex's private app, but its signature does not hold"),
        "{}",
        stderr_of(&ran)
    );
    // And a mark on an app nobody signed.
    let bare = world.bundle("bare.krate", "3.0.0", 64);
    krate_bundle::add_org_mark(&bare, "acme").unwrap();
    let ran = org_command(
        &world,
        "dan",
        &["run", bare.to_str().unwrap(), "--headless"],
    )
    .output()
    .expect("run");
    assert!(!ran.status.success());
    assert!(
        stderr_of(&ran).contains("its signature does not hold"),
        "{}",
        stderr_of(&ran)
    );
    assert_eq!(
        world.hub.lock().unwrap().seen.len(),
        before,
        "no hub was asked"
    );
}

/// Addendum 5: `krate publish --org` takes only a bundle marked for that
/// organisation, and says the exact line to fix it; the hub refuses one
/// that slips past (`not-marked-for-this-org`); and the public door is
/// closed to a marked app before anything is sent.
#[test]
fn only_a_marked_app_goes_to_an_organisation_and_never_to_the_public_door() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "carol"]) else {
        eprintln!("skipping only_a_marked_app_goes_to_an_organisation_and_never_to_the_public_door: no local port");
        return;
    };
    let root = world.acme();
    // Signed with an acme release key, but unmarked: turned back locally.
    let app = world.bundle("unmarked.krate", "1.0.0", 64);
    let release = world.dir.path().join("carol-release.pkcs8");
    std::fs::write(&release, krate_lease::generate_pkcs8().unwrap()).unwrap();
    let slip = world.dir.path().join("carol-acme.json");
    world.says(
        "alice",
        &[
            "delegate",
            "--root",
            root.to_str().unwrap(),
            "--key",
            release.to_str().unwrap(),
            "--namespace",
            "acme/*",
            "-o",
            slip.to_str().unwrap(),
        ],
        true,
    );
    world.says(
        "carol",
        &[
            "sign",
            app.to_str().unwrap(),
            "--key",
            release.to_str().unwrap(),
            "--namespace",
            "acme/demo",
            "--delegation",
            slip.to_str().unwrap(),
        ],
        true,
    );
    let before = world.hub.lock().unwrap().seen.len();
    let said = world.says(
        "carol",
        &["publish", app.to_str().unwrap(), "--org", "acme"],
        false,
    );
    assert!(
        said.contains("is signed but not marked as acme's private app"),
        "{said}"
    );
    assert!(
        said.contains(&format!(
            "krate sign {} --org acme --key <release.pkcs8> --delegation acme-release.json",
            app.display()
        )),
        "{said}"
    );
    // Krate may ask whether the organisation signs by itself (a GET);
    // nothing is sent.
    assert!(
        world.hub.lock().unwrap().seen[before..]
            .iter()
            .all(|(method, _, _)| method == "GET"),
        "nothing was sent"
    );

    // Marked for another organisation: also turned back.
    let other = world.bundle("globex.krate", "1.0.1", 64);
    krate_bundle::add_org_mark(&other, "globex").unwrap();
    let said = world.says(
        "carol",
        &["publish", other.to_str().unwrap(), "--org", "acme"],
        false,
    );
    assert!(
        said.contains("is marked as globex's private app, not acme's"),
        "{said}"
    );

    // The hub's own refusal, for a client that does not check first: the
    // unmarked signed bytes sent straight to the candidates route.
    let bytes = std::fs::read(&app).unwrap();
    let answer = ureq::post(&format!("{}/orgs/acme/candidates", world.url()))
        .set("Authorization", "Bearer tok-carol")
        .set("Idempotency-Key", "k1")
        .send_bytes(&bytes);
    match answer {
        Err(ureq::Error::Status(422, response)) => {
            let body: serde_json::Value =
                serde_json::from_str(&response.into_string().unwrap()).unwrap();
            assert_eq!(body["error"], "not-marked-for-this-org");
        }
        Err(ureq::Error::Status(code, response)) => panic!(
            "the hub must refuse an unmarked candidate: {code} {}",
            response.into_string().unwrap_or_default()
        ),
        other => panic!("the hub must refuse an unmarked candidate: {other:?}"),
    }

    // The public door: a marked app never leaves the machine.
    world.sign_delegated(&root, "carol", &app);
    let before = world.hub.lock().unwrap().seen.len();
    let said = world.says(
        "carol",
        &["publish", app.to_str().unwrap(), "--hub", &world.url()],
        false,
    );
    assert!(
        said.contains(&format!(
            "this app is private to acme: publish it with krate publish {} --org acme",
            app.display()
        )),
        "{said}"
    );
    // Krate may ask whether the organisation signs by itself (a GET);
    // nothing is sent.
    assert!(
        world.hub.lock().unwrap().seen[before..]
            .iter()
            .all(|(method, _, _)| method == "GET"),
        "nothing was sent"
    );

    // `krate sign --org` marked it, and the mark is in the bundle.
    assert_eq!(
        krate_bundle::read_org_mark(&app).unwrap().as_deref(),
        Some("acme")
    );
}

/// H: `krate launch`, the door behind double-click and Studio's Open,
/// fetches a private link with the person's session and refuses in words
/// with the sign-in to try.
#[test]
fn krate_launch_fetches_a_private_link_with_the_session_and_refuses_in_words() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "carol", "dan", "erin"]) else {
        eprintln!("skipping krate_launch_fetches_a_private_link_with_the_session_and_refuses_in_words: no local port");
        return;
    };
    let root = world.acme();
    let (_bundle, _digest, link) = world.released_app(&root, "1.0.0");
    let launch = |who: &str| world.krate_as(who, &["launch", &link]);

    let signed_out = launch("nobody");
    assert!(!signed_out.status.success());
    assert!(
        stderr_of(&signed_out).contains("you are not signed in")
            && stderr_of(&signed_out).contains("krate account login"),
        "{}",
        stderr_of(&signed_out)
    );
    let stranger = launch("erin");
    assert!(!stranger.status.success());
    assert!(
        stderr_of(&stranger).contains("erin is not one of its members"),
        "{}",
        stderr_of(&stranger)
    );
    world.says(
        "alice",
        &["org", "remove", "acme", "dan@acme.example"],
        true,
    );
    let removed = launch("dan");
    assert!(!removed.status.success());
    assert!(
        stderr_of(&removed).contains("you were removed from acme"),
        "the session went with the fetch, so the hub knew who asked: {}",
        stderr_of(&removed)
    );
    let hub = world.hub.lock().unwrap();
    let sessions: Vec<_> = hub
        .seen
        .iter()
        .filter(|(m, p, _)| m == "GET" && p.starts_with("/a/"))
        .map(|(_, _, auth)| auth.clone())
        .collect();
    assert!(
        sessions.contains(&None),
        "the signed-out fetch carried nothing"
    );
    assert!(
        sessions.contains(&Some("Bearer tok-dan@acme.example".to_string())),
        "{sessions:?}"
    );
}

/// H: the Player's door on macOS -- `krate open-app <file>`, which a
/// double-click reaches -- checks a private app in before it runs.
#[cfg(target_os = "macos")]
#[test]
fn the_players_door_checks_a_private_app_in_before_it_runs() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "carol", "dan"]) else {
        eprintln!(
            "skipping the_players_door_checks_a_private_app_in_before_it_runs: no local port"
        );
        return;
    };
    let root = world.acme();
    let (bundle, _digest, link) = world.released_app(&root, "1.0.0");
    let copy = dans_copy(&world, &bundle);
    assert!(world.run_link("dan", &link).status.success());
    world.says(
        "alice",
        &["org", "remove", "acme", "dan@acme.example"],
        true,
    );
    // stderr is a pipe here, so no alert is shown: someone is reading.
    let opened = world.krate_as("dan", &["open-app", copy.to_str().unwrap()]);
    assert!(!opened.status.success());
    assert!(
        stderr_of(&opened).contains("you were removed from acme"),
        "{}",
        stderr_of(&opened)
    );
}

/// H: the MCP door. `run_component` with a private link runs it for a
/// member, and for someone signed out says why in words, not "403".
#[test]
fn the_mcp_door_fetches_a_private_link_with_the_session() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "carol", "dan"]) else {
        eprintln!("skipping the_mcp_door_fetches_a_private_link_with_the_session: no local port");
        return;
    };
    let root = world.acme();
    let (_bundle, _digest, link) = world.released_app(&root, "1.0.0");
    let mcp = |who: &str| -> serde_json::Value {
        let root_dir = world.homes[who].path().join("mcp");
        std::fs::create_dir_all(&root_dir).unwrap();
        let mut child = org_command(&world, who, &["mcp"])
            .env("KRATE_MCP_ROOT", &root_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("krate mcp");
        {
            let mut stdin = child.stdin.take().unwrap();
            for message in [
                serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
                    "params": { "protocolVersion": "2024-11-05", "capabilities": {},
                                "clientInfo": { "name": "test", "version": "0" } } }),
                serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
                serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                    "params": { "name": "run_component",
                                "arguments": { "bundle": link, "insecure_http": true } } }),
            ] {
                writeln!(stdin, "{message}").unwrap();
            }
        }
        let output = child.wait_with_output().expect("mcp output");
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .find(|answer| answer["id"] == 2)
            .expect("an answer to the tool call")
    };
    let member = mcp("dan");
    assert_eq!(member["result"]["isError"], false, "{member}");
    let signed_out = mcp("nobody");
    assert_eq!(signed_out["result"]["isError"], true, "{signed_out}");
    let text = signed_out["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert!(
        text.contains("you are not signed in") && text.contains("krate account login"),
        "{text}"
    );
}

/// K-1102: an organisation's app opened through the MCP door gets its team
/// data as `krate run` gives it -- the same gate, the same grant -- rather
/// than answering not-an-org-app; without the store.org grant it is denied.
#[test]
fn the_mcp_door_gives_an_org_app_its_team_data() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "carol"]) else {
        eprintln!("skipping the_mcp_door_gives_an_org_app_its_team_data: no local port");
        return;
    };
    let root = world.acme();
    let (bundle, _) = released_probe(&world, &root, "1.0.0");
    let copy = world.homes["carol"].path().join("Acme Tasks.krate");
    std::fs::copy(&bundle, &copy).expect("copy");
    let run = |grants: &[&str], steps: &[&str]| -> serde_json::Value {
        let root_dir = world.homes["carol"].path().join("mcp");
        std::fs::create_dir_all(&root_dir).unwrap();
        let mut child = org_command(&world, "carol", &["mcp"])
            .env("KRATE_MCP_ROOT", &root_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("krate mcp");
        {
            let mut stdin = child.stdin.take().unwrap();
            for message in [
                serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
                    "params": { "protocolVersion": "2024-11-05", "capabilities": {},
                                "clientInfo": { "name": "test", "version": "0" } } }),
                serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
                serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                    "params": { "name": "run_component",
                                "arguments": { "bundle": copy.to_str().unwrap(),
                                               "grants": grants, "app_args": steps } } }),
            ] {
                writeln!(stdin, "{message}").unwrap();
            }
        }
        let output = child.wait_with_output().expect("mcp output");
        let answer = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .find(|answer| answer["id"] == 2)
            .expect("an answer to the tool call");
        let text = answer["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("");
        serde_json::from_str(text).unwrap_or_else(|_| serde_json::json!({ "said": text }))
    };
    let all = ["io.stdout", "io.args", "store.org"];
    let report = run(
        &all,
        &[
            "put",
            "tasks",
            "m1",
            "title=from mcp",
            "then",
            "sync",
            "then",
            "who",
        ],
    );
    let out = report["stdout"].as_str().unwrap_or_default();
    assert!(out.contains("put m1 ok"), "{out}");
    assert!(
        out.contains("sync online=true changed=false pending=0"),
        "{out}"
    );
    assert!(out.contains("who carol developer Acme"), "{out}");
    // The write reached the team: the same app through `krate run` reads it.
    let (lines, err) = probe(
        &world,
        "carol",
        &bundle,
        &["sync", "then", "get", "tasks", "m1"],
        &[],
    );
    assert!(
        lines[1].starts_with("get m1 title=from mcp"),
        "{lines:?} {err}"
    );
    // Without the grant it is denied, as everywhere.
    let denied = run(&["io.stdout", "io.args"], &["get", "tasks", "m1"]);
    assert_eq!(denied["exit"]["class"], "permission-denied", "{denied}");
    assert_eq!(denied["capabilities"]["denied"][0], "store.org", "{denied}");
}

/// I (the CLI half Studio drives): `krate org release-key` makes a release
/// key and prints the exact `krate delegate` line for an owner, with the
/// key's public half -- never the private file. Once the delegation is
/// installed, `krate publish --org` signs the app itself; without one,
/// nothing unsigned is uploaded.
#[test]
fn a_release_key_from_studio_signs_what_is_sent_for_approval() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "carol"]) else {
        eprintln!(
            "skipping a_release_key_from_studio_signs_what_is_sent_for_approval: no local port"
        );
        return;
    };
    let root = world.acme();
    let app = world.bundle("studio.krate", "1.0.0", 64);
    let before = world.hub.lock().unwrap().seen.len();
    let said = world.says(
        "carol",
        &["publish", app.to_str().unwrap(), "--org", "acme"],
        false,
    );
    assert!(said.contains("krate org release-key acme"), "{said}");
    assert!(
        world.hub.lock().unwrap().seen[before..]
            .iter()
            .all(|(method, _, _)| method == "GET"),
        "nothing unsigned is uploaded"
    );

    let said = world.says("carol", &["org", "release-key", "acme"], true);
    let public = said
        .lines()
        .find_map(|l| l.strip_prefix("your release key for acme: "))
        .expect("the public key")
        .trim()
        .to_string();
    assert_eq!(public.len(), 64);
    let delegate_line = format!(
        "krate delegate --root <acme-root.pkcs8> --key {public} --namespace 'acme/*' -o acme-release.json"
    );
    assert!(said.contains(&delegate_line), "{said}");
    assert!(world.homes["carol"]
        .path()
        .join(".krate/orgs/acme/release.pkcs8")
        .is_file());

    // The owner runs exactly that line, with the public key as given.
    let slip = world.dir.path().join("acme-release.json");
    world.says(
        "alice",
        &[
            "delegate",
            "--root",
            root.to_str().unwrap(),
            "--key",
            &public,
            "--namespace",
            "acme/*",
            "-o",
            slip.to_str().unwrap(),
        ],
        true,
    );
    let said = world.says(
        "carol",
        &[
            "org",
            "release-key",
            "acme",
            "--delegation",
            slip.to_str().unwrap(),
        ],
        true,
    );
    assert!(said.contains("installed."), "{said}");
    let said = world.says(
        "carol",
        &["publish", app.to_str().unwrap(), "--org", "acme"],
        true,
    );
    assert!(said.contains("signed "), "it signed the app first: {said}");
    let digest = OrgWorld::digest_in(&said);
    assert_eq!(
        world.hub.lock().unwrap().orgs["acme"].candidates[&digest].state,
        "waiting-for-approval"
    );
    // A delegation for somebody else's key is refused, plainly.
    let other = world.dir.path().join("other.pkcs8");
    std::fs::write(&other, krate_lease::generate_pkcs8().unwrap()).unwrap();
    let wrong = world.dir.path().join("wrong.json");
    world.says(
        "alice",
        &[
            "delegate",
            "--root",
            root.to_str().unwrap(),
            "--key",
            other.to_str().unwrap(),
            "--namespace",
            "acme/*",
            "-o",
            wrong.to_str().unwrap(),
        ],
        true,
    );
    let said = world.says(
        "carol",
        &[
            "org",
            "release-key",
            "acme",
            "--delegation",
            wrong.to_str().unwrap(),
        ],
        false,
    );
    assert!(said.contains("delegates a different key"), "{said}");
    // `krate org list --json` is what Studio reads to offer the menu.
    let said = world.says("carol", &["org", "list", "--json"], true);
    let list: serde_json::Value = serde_json::from_str(said.trim()).expect("JSON");
    assert_eq!(list[0]["slug"], "acme");
    assert_eq!(list[0]["role"], "developer");
}

/// IC-958, end to end, with no key command anywhere: Alice makes acme and
/// releases alone; Carol joins as a developer and publishes, approved as a
/// publisher in the background by Alice's computer while she waits; Bob
/// joins as a reviewer and from then on every release needs a second
/// person; Carol is removed and her next upload is refused, and Alice's
/// computer withdraws her release key; the audit names every automatic
/// signing step. With KRATE_ORG_E2E_HUB, against the hub's own code.
#[test]
fn an_organisation_signs_and_releases_with_nobody_handling_a_key() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "carol"]) else {
        eprintln!(
            "skipping an_organisation_signs_and_releases_with_nobody_handling_a_key: no local port"
        );
        return;
    };
    let no_key_words = |said: &str| {
        for word in [
            "pkcs8",
            "delegat",
            "root key",
            "release key",
            "namespace",
            "krate sign",
            "krate delegate",
        ] {
            assert!(
                !said.to_ascii_lowercase().contains(word),
                "{word:?} on screen: {said}"
            );
        }
    };
    let said = world.says("alice", &["org", "create", "acme", "--name", "Acme"], true);
    assert!(
        said.contains("Every app Acme releases is signed by Acme"),
        "{said}"
    );
    assert!(
        said.contains("krate publish app.krate --org acme"),
        "{said}"
    );

    // Alice alone: her upload is released at once, marked as released alone.
    let first = world.bundle("notes-1.krate", "1.0.0", 64);
    let said = world.says(
        "alice",
        &["publish", first.to_str().unwrap(), "--org", "acme"],
        true,
    );
    no_key_words(&said);
    assert!(said.contains("Signed for acme."), "{said}");
    assert!(
        said.contains("released Acme Demo 1.0.0 to acme. Everyone in acme can use it now."),
        "{said}"
    );
    assert!(
        said.contains("Released by you: acme has nobody else who can approve yet."),
        "{said}"
    );
    let solo = OrgWorld::digest_in(&said);

    // Carol joins as a developer and publishes. She waits; Alice's computer
    // approves her the next time it runs a `krate org` command.
    let code = world.invite("alice", "carol", "developer");
    world.says("carol", &["org", "join", "acme", &code], true);
    let second = world.bundle("notes-2.krate", "1.0.1", 65);
    let carol = {
        let mut command = org_command(
            &world,
            "carol",
            &["publish", second.to_str().unwrap(), "--org", "acme"],
        );
        command.env("KRATE_PUBLISH_WAIT_SECONDS", "90");
        command
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        command.spawn().expect("carol publishes")
    };
    // Until her request is on the hub.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let asked = match world.real_hub() {
            Some(_) => {
                std::thread::sleep(std::time::Duration::from_secs(4));
                true
            }
            None => world.hub.lock().unwrap().orgs["acme"]
                .signing
                .publishers
                .values()
                .any(|p| p.principal == "github:carol"),
        };
        if asked || std::time::Instant::now() > deadline {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let output = world.krate_as("alice", &["org", "releases", "acme"]);
    let alice_said = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        stderr_of(&output)
    );
    assert!(output.status.success(), "{alice_said}");
    assert!(
        alice_said.contains("This computer approved carol to publish to acme."),
        "{alice_said}"
    );
    let carol = carol.wait_with_output().expect("carol");
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&carol.stdout),
        stderr_of(&carol)
    );
    assert!(carol.status.success(), "{said}");
    no_key_words(&said);
    assert!(
        said.contains("Getting you ready to publish to Acme..."),
        "{said}"
    );
    assert!(
        said.contains(
            "Waiting for an owner's computer to approve you as a publisher -- alice can."
        ),
        "{said}"
    );
    assert!(
        said.contains("Approved: you can publish to Acme."),
        "{said}"
    );
    assert!(said.contains("uploaded to acme as a candidate"), "{said}");
    assert!(said.contains("Waiting for alice to approve it"), "{said}");
    let carols = OrgWorld::digest_in(&said);
    world.says("alice", &["org", "approve", "acme", &carols], true);

    // Bob joins as a reviewer: from now on, two people every time.
    let code = world.invite("alice", "bob", "reviewer");
    world.says("bob", &["org", "join", "acme", &code], true);
    let third = world.bundle("notes-3.krate", "1.0.2", 66);
    let said = world.says(
        "alice",
        &["publish", third.to_str().unwrap(), "--org", "acme"],
        true,
    );
    assert!(said.contains("waits for approval"), "{said}");
    assert!(said.contains("Waiting for bob to approve it"), "{said}");
    let alices = OrgWorld::digest_in(&said);
    let said = world.says("alice", &["org", "approve", "acme", &alices], false);
    assert!(said.contains("you can't approve your own upload"), "{said}");
    let fourth = world.bundle("notes-4.krate", "1.0.3", 67);
    let said = world.says(
        "carol",
        &["publish", fourth.to_str().unwrap(), "--org", "acme"],
        true,
    );
    assert!(
        said.contains("Waiting for alice or another reviewer to approve it"),
        "{said}"
    );
    world.says("bob", &["org", "approve", "acme", &alices], true);

    // Carol is removed: her next upload is refused, and Alice's computer
    // withdraws her release key in the background.
    let said = world.says("alice", &["org", "remove", "acme", "carol"], true);
    let said_after = world.says("alice", &["org", "show", "acme"], true);
    assert!(
        format!("{said}{said_after}")
            .contains("withdrew the publishing rights of someone who left acme"),
        "{said}{said_after}"
    );
    let fifth = world.bundle("notes-5.krate", "1.0.4", 68);
    let said = world.says(
        "carol",
        &["publish", fifth.to_str().unwrap(), "--org", "acme"],
        false,
    );
    assert!(said.contains("you were removed from acme"), "{said}");

    // The audit names every automatic step, in order, with the chain intact.
    let said = world.says("alice", &["org", "audit", "acme"], true);
    assert!(said.contains("chain intact"), "{said}");
    let mut at = 0;
    for step in [
        "signing.init",
        "candidate.approve",
        "publisher.request",
        "publisher.delegate",
        "review.two-person.on",
        "publisher.revoke-due",
        "release-key.revoke",
    ] {
        let found = said[at..]
            .find(step)
            .unwrap_or_else(|| panic!("{step} after byte {at}: {said}"));
        at += found + step.len();
    }
    assert!(
        said.lines().any(|l| l.contains("candidate.approve")
            && l.contains(&solo[..12])
            && l.contains("allowed: solo")),
        "{said}"
    );
    assert!(said.lines().any(|l| l.contains("publisher.delegate") && l.contains("automatic: carol may publish")), "{said}");
    assert!(
        said.lines()
            .any(|l| l.contains("release-key.revoke") && l.contains("automatic")),
        "{said}"
    );
}

// ---------------------------------------------------------------------------
// The machine policy and the open record (IC-949, FLEET-CONTRACT P and A).
// ---------------------------------------------------------------------------

/// One machine, as an organisation sets it up: a policy file, a home of its
/// own, and the open record in a folder the test can read.
struct Fleet {
    dir: tempfile::TempDir,
    home: tempfile::TempDir,
}

impl Fleet {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("fleet dir"),
            home: tempfile::tempdir().expect("fleet home"),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn record_path(&self) -> PathBuf {
        self.path("opens.jsonl")
    }

    /// Write the policy, with the open record pointed into this folder.
    fn policy(&self, body: &str) -> PathBuf {
        let path = self.path("policy.toml");
        std::fs::write(
            &path,
            format!(
                "version = 1\nname = \"Acme Labs\"\n{body}\n[audit]\npath = \"{}\"\n",
                self.record_path()
                    .display()
                    .to_string()
                    .replace('\\', "\\\\")
            ),
        )
        .expect("policy");
        path
    }

    /// `krate`, on this machine.
    fn krate(&self) -> Command {
        let mut command = krate();
        command
            .env("KRATE_TEST_POLICY", self.path("policy.toml"))
            .env("HOME", self.home.path())
            .env("USERPROFILE", self.home.path());
        command
    }

    fn key(&self, name: &str) -> (PathBuf, String) {
        let path = self.path(&format!("{name}.pkcs8"));
        if !path.exists() {
            std::fs::write(
                &path,
                krate_bundle::signing::SigningKey::generate_pkcs8().expect("key"),
            )
            .expect("write key");
        }
        let bytes = std::fs::read(&path).expect("read key");
        let public = krate_bundle::signing::SigningKey::from_pkcs8(&bytes)
            .expect("key")
            .public_key();
        (path, public.iter().map(|b| format!("{b:02x}")).collect())
    }

    /// A minimal CLI app that requires `caps`, packed.
    fn app(&self, name: &str, caps: &[&str]) -> PathBuf {
        let wasm = self.path("code.wasm");
        std::fs::write(
            &wasm,
            include_bytes!("../../bundle/tests/fixtures/minimal-run.wasm"),
        )
        .expect("component");
        let declared: String = caps
            .iter()
            .map(|cap| {
                format!(
                    "\n[[capabilities]]\ncap = \"{cap}\"\nrationale = \"its work\"\nrequired = true\n"
                )
            })
            .collect();
        let manifest = self.path(&format!("{name}.toml"));
        std::fs::write(
            &manifest,
            format!(
                "[app]\nid = \"com.acme.{name}\"\nname = \"{name}\"\nversion = \"1.0.0\"\n\
                 entry = \"code.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n{declared}"
            ),
        )
        .expect("manifest");
        let bundle = self.path(&format!("{name}.krate"));
        let out = krate()
            .arg("pack")
            .arg(&wasm)
            .arg("--manifest")
            .arg(&manifest)
            .arg("-o")
            .arg(&bundle)
            .output()
            .expect("pack");
        assert!(
            out.status.success(),
            "pack: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        bundle
    }

    /// Sign directly with `key`.
    fn sign(&self, bundle: &std::path::Path, key: &std::path::Path) {
        let out = krate()
            .arg("sign")
            .arg(bundle)
            .arg("--key")
            .arg(key)
            .args(["--namespace", "acme/app"])
            .output()
            .expect("sign");
        assert!(
            out.status.success(),
            "sign: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Sign with a release key the root delegated.
    fn sign_delegated(
        &self,
        bundle: &std::path::Path,
        root: &std::path::Path,
        release: &std::path::Path,
    ) {
        let delegation = self.path("delegation.json");
        let out = krate()
            .args(["delegate", "--namespace", "acme/*", "--days", "30"])
            .arg("--root")
            .arg(root)
            .arg("--key")
            .arg(release)
            .arg("-o")
            .arg(&delegation)
            .output()
            .expect("delegate");
        assert!(
            out.status.success(),
            "delegate: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let out = krate()
            .arg("sign")
            .arg(bundle)
            .arg("--key")
            .arg(release)
            .args(["--namespace", "acme/app"])
            .arg("--delegation")
            .arg(&delegation)
            .output()
            .expect("sign");
        assert!(
            out.status.success(),
            "sign delegated: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Every record written, parsed: each line must be one JSON object.
    fn records(&self) -> Vec<serde_json::Value> {
        records_in(&self.record_path())
    }
}

fn records_in(path: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {line}")))
        .collect()
}

/// Run one app through the MCP door and return what the server answered.
fn mcp_run(fleet: &Fleet, bundle: &std::path::Path) -> String {
    mcp_run_granting(fleet, bundle, &[])
}

fn mcp_run_granting(fleet: &Fleet, bundle: &std::path::Path, grants: &[&str]) -> String {
    let mut child = fleet
        .krate()
        .arg("mcp")
        .env("KRATE_MCP_ROOT", fleet.path("mcp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("mcp");
    let call = serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {
            "name": "run_component",
            "arguments": { "bundle": bundle.display().to_string(), "grants": grants }
        }
    });
    let init = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "fleet-test", "version": "0" }
        }
    });
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        writeln!(stdin, "{init}").expect("init");
        writeln!(stdin, "{call}").expect("call");
    }
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("mcp output");
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// Proof item 1: the policy is read from each source this OS has, the first
/// present wins, the others are named as ignored, and `krate policy` and
/// `--json` say so.
#[test]
fn the_policy_is_read_from_each_source_and_shown() {
    let fleet = Fleet::new();
    let root = fleet.path("machine");
    let file_at = if cfg!(target_os = "macos") {
        root.join("Library/Application Support/Krate/policy.toml")
    } else if cfg!(windows) {
        root.join("ProgramData/Krate/policy.toml")
    } else {
        root.join("etc/krate/policy.toml")
    };
    let show = |json: bool| {
        let mut command = krate();
        command
            .env("KRATE_TEST_POLICY_ROOT", &root)
            .env("HOME", fleet.home.path())
            .arg("policy");
        if json {
            command.arg("--json");
        }
        command.output().expect("policy")
    };

    // Nothing there: no policy, and it says so plainly.
    let none = show(false);
    assert!(none.status.success());
    assert!(
        String::from_utf8_lossy(&none.stdout).contains("No organisation policy on this computer.")
    );

    // The file.
    std::fs::create_dir_all(file_at.parent().unwrap()).unwrap();
    std::fs::write(
        &file_at,
        "version = 1\nname = \"From The File\"\n[usage]\ncounting = false\n",
    )
    .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&show(true).stdout).expect("json");
    assert_eq!(json["state"], "in-force");
    assert_eq!(json["source"]["kind"], "file");
    assert_eq!(json["source"]["location"], file_at.display().to_string());
    assert_eq!(json["policy"]["name"], "From The File");
    assert_eq!(json["policy"]["usage"]["counting"], false);
    let words = String::from_utf8_lossy(&show(false).stdout).to_string();
    assert!(
        words.contains("Your organisation (From The File) sets how Krate works"),
        "{words}"
    );
    assert!(words.contains("Usage counting: off"), "{words}");

    // On a Mac the managed preferences come first, as the binary plist a
    // configuration profile writes, and the file is then named as ignored.
    if cfg!(target_os = "macos") {
        let prefs = root.join("Library/Managed Preferences");
        std::fs::create_dir_all(&prefs).unwrap();
        let plist = prefs.join("tech.krate.policy.plist");
        std::fs::write(
            &plist,
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
             \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\"><dict><key>version</key><integer>1</integer>\
             <key>name</key><string>From MDM</string>\
             <key>studio</key><dict><key>ai</key><false/></dict>\
             <key>network</key><dict><key>allow</key>\
             <array><string>*.acme.com:443</string></array></dict>\
             </dict></plist>\n",
        )
        .unwrap();
        assert!(Command::new("/usr/bin/plutil")
            .args(["-convert", "binary1"])
            .arg(&plist)
            .status()
            .expect("plutil")
            .success());
        let json: serde_json::Value = serde_json::from_slice(&show(true).stdout).expect("json");
        assert_eq!(json["source"]["kind"], "managed-preferences", "{json}");
        assert_eq!(json["policy"]["name"], "From MDM");
        assert_eq!(json["policy"]["studio"]["ai"], false);
        assert_eq!(json["policy"]["network"]["allow"][0], "*.acme.com:443");
        assert_eq!(json["ignored"][0]["kind"], "file");
        let words = String::from_utf8_lossy(&show(false).stdout).to_string();
        assert!(
            words.contains("Ignored:") && words.contains("policy.toml"),
            "{words}"
        );
    }
}

/// Proof item 2: at every door that can be driven without a screen -- the
/// terminal, MCP, and `krate launch` -- an unsigned app, an app signed by
/// another root, a changed signed app, an app needing a blocked capability
/// and one needing a host outside the ceiling are each refused in the
/// organisation's words, with a `refused` record; a trusted app opens and
/// leaves an `opened` record.
#[test]
fn each_policy_refusal_is_made_at_every_door_and_recorded() {
    let fleet = Fleet::new();
    let (root, root_hex) = fleet.key("root");
    let (release, release_hex) = fleet.key("release");
    let (stranger, _) = fleet.key("stranger");
    fleet.policy(&format!(
        "[trust]\nroots = [{{ name = \"Acme Labs\", key = \"{root_hex}\" }}]\n\
         [capabilities]\ndeny = [\"random.bytes\"]\n\
         [network]\nallow = [\"*.acme.com:443\"]\n"
    ));

    let unsigned = fleet.app("unsigned", &[]);
    let other = fleet.app("other", &[]);
    fleet.sign(&other, &stranger);
    let signed = fleet.app("tampered-source", &[]);
    fleet.sign(&signed, &root);
    let tampered = fleet.path("tampered.krate");
    rewrite_bundle_entry(&signed, &tampered, "manifest.toml", |bytes| {
        let mut text = String::from_utf8_lossy(bytes).into_owned();
        text.push_str("\n# changed after signing\n");
        text.into_bytes()
    });
    let blocked = fleet.app("blocked", &["random.bytes"]);
    fleet.sign(&blocked, &root);
    let outside = fleet.app("outside", &["net.connect:evil.example:443"]);
    fleet.sign(&outside, &root);
    let trusted = fleet.app("trusted", &["net.connect:api.acme.com:443"]);
    fleet.sign_delegated(&trusted, &root, &release);

    let cases: [(&PathBuf, &str, &str); 5] = [
        (
            &unsigned,
            "policy-unsigned",
            "allows only signed apps on this computer",
        ),
        (
            &other,
            "policy-other-signer",
            "allows only apps signed by Acme Labs",
        ),
        (
            &tampered,
            "policy-tampered",
            "this app's signature does not",
        ),
        (
            &blocked,
            "policy-capability",
            "which your organisation (Acme Labs) blocks",
        ),
        (&outside, "policy-network", "connect to evil.example:443"),
    ];
    for (bundle, reason, words) in cases {
        // The terminal.
        let run = fleet
            .krate()
            .arg("run")
            .arg(bundle)
            .args(["--headless", "--auto-grant"])
            .output()
            .expect("run");
        let stderr = String::from_utf8_lossy(&run.stderr);
        assert!(!run.status.success(), "{reason} must not open: {stderr}");
        assert!(
            stderr.contains(words),
            "{reason} in the organisation's words: {stderr}"
        );
        assert!(stderr.contains("your organisation (Acme Labs)"), "{stderr}");
        assert!(
            stderr.contains("Ask Acme Labs's IT team"),
            "who to ask: {stderr}"
        );

        // `krate launch` refuses in its own process, before anything opens.
        let launch = fleet
            .krate()
            .arg("launch")
            .arg(bundle)
            .output()
            .expect("launch");
        let launch_err = String::from_utf8_lossy(&launch.stderr);
        assert!(!launch.status.success(), "launch {reason}: {launch_err}");
        assert!(launch_err.contains(words), "launch {reason}: {launch_err}");

        // MCP.
        let answer = mcp_run(&fleet, bundle);
        assert!(answer.contains(words), "mcp {reason}: {answer}");
        assert!(
            answer.contains("\"isError\":true"),
            "mcp {reason}: {answer}"
        );
    }

    let records = fleet.records();
    for (bundle, reason, _) in cases {
        let name = bundle.file_stem().unwrap().to_string_lossy().to_string();
        for door in ["terminal", "window", "mcp"] {
            assert!(
                records.iter().any(|r| r["event"] == "refused"
                    && r["reason"] == reason
                    && r["door"] == door
                    && r["words"].as_str().is_some_and(|w| w.contains("Acme Labs"))),
                "a {reason} record from the {door} door for {name}: {records:#?}"
            );
        }
    }

    // The trusted app opens, at the terminal and through MCP.
    let run = fleet
        .krate()
        .arg("run")
        .arg(&trusted)
        .args(["--headless", "--auto-grant"])
        .output()
        .expect("run trusted");
    assert_eq!(
        run.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let answer = mcp_run_granting(&fleet, &trusted, &["net.connect:api.acme.com:443"]);
    assert!(
        answer.contains("\"isError\":false"),
        "mcp trusted: {answer}"
    );
    assert!(answer.contains("success"), "mcp trusted ran: {answer}");
    let records = fleet.records();
    let opened: Vec<_> = records.iter().filter(|r| r["event"] == "opened").collect();
    assert_eq!(opened.len(), 2, "one opened record per open: {records:#?}");
    let digest = {
        let bytes = std::fs::read(&trusted).unwrap();
        format!("sha256:{}", sha256_hex(&bytes))
    };
    for record in &opened {
        assert_eq!(record["v"], 1);
        assert_eq!(record["app"], "com.acme.trusted");
        assert_eq!(record["name"], "trusted");
        assert_eq!(record["version"], "1.0.0");
        assert_eq!(record["digest"], digest.as_str());
        assert_eq!(record["root"], root_hex.as_str(), "the delegation's root");
        assert_eq!(record["signer"], release_hex.as_str(), "the release key");
        assert!(record["krate"].as_str().is_some_and(|v| !v.is_empty()));
        assert!(record["at"]
            .as_str()
            .is_some_and(|at| at.ends_with('Z') && at.len() == 20));
        assert!(record["user"].is_string());
    }
    assert!(opened.iter().any(|r| r["door"] == "terminal"
        && r["granted"] == serde_json::json!(["net.connect:api.acme.com:443"])));
    assert!(opened.iter().any(|r| r["door"] == "mcp"));
}

/// The ceiling also holds for what a person grants: an optional capability
/// the organisation blocks is shown in the prompt as blocked, is not one of
/// the choices, and stays refused when everything is allowed.
#[test]
fn a_blocked_optional_capability_is_shown_and_cannot_be_granted() {
    let fleet = Fleet::new();
    fleet.policy("[capabilities]\ndeny = [\"random.bytes\"]\n");
    let wasm = fleet.path("code.wasm");
    std::fs::write(
        &wasm,
        include_bytes!("../../bundle/tests/fixtures/minimal-run.wasm"),
    )
    .unwrap();
    let manifest = fleet.path("optional.toml");
    std::fs::write(
        &manifest,
        "[app]\nid = \"com.acme.optional\"\nname = \"optional\"\nversion = \"1.0.0\"\n\
         entry = \"code.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n\n\
         [[capabilities]]\ncap = \"random.bytes\"\nrationale = \"dice\"\nrequired = false\n\n\
         [[capabilities]]\ncap = \"store.kv\"\nrationale = \"notes\"\nrequired = true\n",
    )
    .unwrap();
    let bundle = fleet.path("optional.krate");
    assert!(krate()
        .arg("pack")
        .arg(&wasm)
        .arg("--manifest")
        .arg(&manifest)
        .arg("-o")
        .arg(&bundle)
        .status()
        .unwrap()
        .success());
    let mut child = fleet
        .krate()
        .arg("run")
        .arg(&bundle)
        .args(["--headless", "--prompt"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run");
    child.stdin.as_mut().unwrap().write_all(b"A\n").unwrap();
    let out = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(
        stderr.contains("Not available on this computer (blocked by Acme Labs)"),
        "{stderr}"
    );
    assert!(
        stderr.contains("[1]") && !stderr.contains("[2]"),
        "only store.kv is a choice: {stderr}"
    );
    let opened = fleet
        .records()
        .into_iter()
        .find(|r| r["event"] == "opened")
        .expect("opened");
    assert_eq!(opened["granted"], serde_json::json!(["store.kv"]));
    assert_eq!(opened["denied"], serde_json::json!(["random.bytes"]));
}

/// Proof item 3: a policy that cannot be read, names an unknown version, or
/// carries an unknown key refuses to open apps, in words naming the key and
/// the file, and is recorded as `policy-unreadable`.
#[test]
fn an_unusable_policy_refuses_to_open_apps() {
    let fleet = Fleet::new();
    let app = fleet.app("any", &[]);
    let policy = fleet.path("policy.toml");
    for (body, names) in [
        ("version = 2\n", "`version` is 2"),
        (
            "version = 1\n[trust]\nalow_unsigned = true\n",
            "`trust.alow_unsigned` is not a setting Krate knows",
        ),
        (
            "version = 1\nfavourite = \"blue\"\n",
            "`favourite` is not a setting Krate knows",
        ),
        (
            "version = 1\n[trust]\nroots = [{ key = \"nothex\" }]\n",
            "`trust.roots[0].key`",
        ),
        ("this is not toml at all [[[", "not valid TOML"),
    ] {
        std::fs::write(&policy, body).unwrap();
        let run = fleet
            .krate()
            .arg("run")
            .arg(&app)
            .args(["--headless"])
            .output()
            .expect("run");
        let stderr = String::from_utf8_lossy(&run.stderr);
        assert!(!run.status.success(), "{body:?} must refuse: {stderr}");
        assert!(
            stderr.contains("Krate opens no apps on this computer"),
            "{stderr}"
        );
        assert!(stderr.contains(names), "names the key: {stderr}");
        assert!(
            stderr.contains(&policy.display().to_string()),
            "names the file: {stderr}"
        );
        let shown = fleet.krate().arg("policy").output().expect("policy");
        assert_eq!(shown.status.code(), Some(1), "krate policy exits 1");
        assert!(String::from_utf8_lossy(&shown.stdout).contains("cannot be used"));
    }
    // A policy that cannot be read at all: a folder where the file should be.
    std::fs::remove_file(&policy).unwrap();
    std::fs::create_dir(&policy).unwrap();
    let run = fleet
        .krate()
        .arg("run")
        .arg(&app)
        .args(["--headless"])
        .output()
        .expect("run");
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        !run.status.success() && stderr.contains("it cannot be read"),
        "{stderr}"
    );
    // The organisation's record path is unknown, so the record goes to the
    // default, and from there to this person's own when that cannot be
    // written.
    let fallback = fleet.home.path().join(".krate/logs/opens.jsonl");
    let records = records_in(&fallback);
    assert!(
        records
            .iter()
            .filter(|r| r["reason"] == "policy-unreadable")
            .count()
            >= 6,
        "every refusal recorded: {records:#?}"
    );
}

/// Accept connections on a local port and count the requests that arrive.
fn counting_listener() -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}", listener.local_addr().unwrap());
    let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = count.clone();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut stream = stream;
            let mut buf = [0u8; 4096];
            let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\ncontent-type: application/json\r\n\
                  connection: close\r\n\r\n{}",
            );
        }
    });
    (url, count)
}

fn wait_for(count: &std::sync::atomic::AtomicUsize, at_least: usize, secs: u64) -> usize {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if count.load(std::sync::atomic::Ordering::SeqCst) >= at_least {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    count.load(std::sync::atomic::Ordering::SeqCst)
}

/// Proof item 4: under `counting = false` no usage request leaves the
/// machine. The same run without the policy does send one, so the listener
/// is proved to be listening.
#[test]
fn counting_off_sends_no_usage_at_all() {
    let fleet = Fleet::new();
    let app = fleet.app("counted", &[]);
    let (url, count) = counting_listener();
    let endpoint = format!("{url}/usage");

    // No policy: the open is counted and sent.
    let baseline = Fleet::new();
    let out = baseline
        .krate()
        .env("KRATE_TEST_POLICY", "none")
        .env("KRATE_USAGE_URL", &endpoint)
        .env_remove("KRATE_NO_USAGE")
        .env_remove("DO_NOT_TRACK")
        .arg("run")
        .arg(&app)
        .arg("--headless")
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        wait_for(&count, 1, 20) >= 1,
        "the listener hears an ordinary open"
    );
    // Let the baseline's helper finish whatever else it sends.
    thread::sleep(Duration::from_secs(2));
    let before = count.load(std::sync::atomic::Ordering::SeqCst);

    fleet.policy("[usage]\ncounting = false\n");
    let app_arg = app.display().to_string();
    for args in [
        vec!["run", app_arg.as_str(), "--headless"],
        vec!["usage-flush"],
    ] {
        let out = fleet
            .krate()
            .env("KRATE_USAGE_URL", &endpoint)
            .env_remove("KRATE_NO_USAGE")
            .env_remove("DO_NOT_TRACK")
            .args(&args)
            .output()
            .expect("run");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    assert_eq!(wait_for(&count, before + 1, 5), before, "nothing was sent");
    let krate_dir = fleet.home.path().join(".krate");
    assert!(
        !krate_dir.join("usage-spool.jsonl").exists(),
        "nothing was even queued"
    );
    assert!(!krate_dir.join("install-id").exists(), "no id was made");
    let status = fleet
        .krate()
        .args(["telemetry", "on"])
        .output()
        .expect("telemetry");
    assert!(String::from_utf8_lossy(&status.stdout).contains("your organisation turned it off"));
}

/// Proof item 5, the hub: under `[hub] url` a different hub asked for by
/// `KRATE_HUB_URL` or `--hub` is refused in the organisation's words with
/// nothing sent, and with nothing else named the pinned hub is used.
#[test]
fn a_pinned_hub_refuses_any_other_hub() {
    let fleet = Fleet::new();
    let (pinned, pinned_count) = counting_listener();
    let (other, other_count) = counting_listener();
    fleet.policy(&format!("[hub]\nurl = \"{pinned}\"\n"));

    let refused = fleet
        .krate()
        .env("KRATE_HUB_URL", &other)
        .args(["blocklist", "--update"])
        .output()
        .expect("blocklist");
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(!refused.status.success(), "{stderr}");
    assert!(
        stderr.contains(&format!("lets this computer use only the hub at {pinned}")),
        "{stderr}"
    );
    assert!(stderr.contains("KRATE_HUB_URL asks for"), "{stderr}");

    let app = fleet.app("published", &[]);
    let flag = fleet
        .krate()
        .env_remove("KRATE_HUB_URL")
        .arg("publish")
        .arg(&app)
        .args(["--hub", &other])
        .output()
        .expect("publish");
    let stderr = String::from_utf8_lossy(&flag.stderr);
    assert!(
        !flag.status.success() && stderr.contains("--hub asks for"),
        "{stderr}"
    );
    assert_eq!(
        wait_for(&other_count, 1, 2),
        0,
        "the other hub heard nothing"
    );

    // Nothing else named: the pinned hub is the one asked.
    let _ = fleet
        .krate()
        .env_remove("KRATE_HUB_URL")
        .args(["blocklist", "--update"])
        .output()
        .expect("blocklist");
    assert!(
        wait_for(&pinned_count, 1, 5) >= 1,
        "the pinned hub was asked"
    );
    assert_eq!(other_count.load(std::sync::atomic::Ordering::SeqCst), 0);
}

/// Proof item 5, Studio: `[studio] ai = false` stops an AI build before any
/// provider is contacted, and `providers` stops one that is not listed. The
/// provider is a fake `claude` that leaves a mark whenever it is started;
/// without a policy it is started, so the mark proves the test bites.
#[cfg(unix)]
#[test]
fn studio_ai_off_stops_a_build_before_any_provider_is_contacted() {
    use std::os::unix::fs::PermissionsExt;
    let fleet = Fleet::new();
    let bin = fleet.path("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let mark = fleet.path("provider-was-contacted");
    let fake = bin.join("claude");
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\necho \"$@\" >> '{}'\necho '{{}}'\n",
            mark.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let plan = |policy: Option<&str>| {
        if let Some(body) = policy {
            fleet.policy(body);
        }
        let mut command = fleet.krate();
        if policy.is_none() {
            command.env("KRATE_TEST_POLICY", "none");
        }
        let child = command
            .env("PATH", &path)
            .args([
                "plan",
                "a habit tracker that counts my morning runs",
                "--agent",
                "claude",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("plan");
        output_bounded_child(child, 60)
    };

    let (code, stderr) = plan(Some("[studio]\nai = false\n"));
    assert_ne!(code, Some(0), "{stderr}");
    assert!(
        stderr.contains("has turned off building apps with AI on this computer"),
        "{stderr}"
    );
    assert!(stderr.contains("nothing was sent to any AI"), "{stderr}");
    assert!(!mark.exists(), "no provider was started");

    let (code, stderr) = plan(Some("[studio]\nproviders = [\"codex\"]\n"));
    assert_ne!(code, Some(0), "{stderr}");
    assert!(
        stderr.contains("allows building with codex only"),
        "{stderr}"
    );
    assert!(!mark.exists(), "an unlisted provider was not started");

    let _ = plan(None);
    assert!(
        mark.exists(),
        "without the policy the fake provider is started"
    );
}

fn output_bounded_child(mut child: std::process::Child, secs: u64) -> (Option<i32>, String) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Ok(Some(_)) = child.try_wait() {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    let out = child.wait_with_output().expect("output");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// `[runtime] interpreter = true` runs the app on the interpreter, from the
/// policy and not the environment.
#[test]
fn a_policy_can_put_every_app_on_the_interpreter() {
    let fleet = Fleet::new();
    let app = fleet.app("interpreted", &[]);
    let run = |body: &str| {
        fleet.policy(body);
        let out = fleet
            .krate()
            .env("KRATE_STAGE_TIMES", "1")
            .env_remove("KRATE_PULLEY")
            .arg("run")
            .arg(&app)
            .arg("--headless")
            .output()
            .expect("run");
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(out.status.success(), "{stderr}");
        stderr
    };
    assert!(run("[runtime]\ninterpreter = true\n").contains("engine is the interpreter (pulley64)"));
    assert!(!run("[runtime]\ninterpreter = false\n").contains("engine is the interpreter"));
    let shown = fleet.krate().arg("policy").output().unwrap();
    assert!(!String::from_utf8_lossy(&shown.stdout).contains("interpreter"));
    fleet.policy("[runtime]\ninterpreter = true\n");
    let shown = fleet.krate().arg("policy").output().unwrap();
    assert!(String::from_utf8_lossy(&shown.stdout).contains("they may run slower this way"));
}

/// `krate policy check` gives the verdict and the exact words, and runs
/// and records nothing.
#[test]
fn policy_check_says_whether_an_app_would_open_here() {
    let fleet = Fleet::new();
    let (root, root_hex) = fleet.key("root");
    fleet.policy(&format!(
        "[trust]\nroots = [{{ name = \"Acme Labs\", key = \"{root_hex}\" }}]\n"
    ));
    let unsigned = fleet.app("loose", &[]);
    let signed = fleet.app("good", &[]);
    fleet.sign(&signed, &root);
    let no = fleet
        .krate()
        .args(["policy", "check"])
        .arg(&unsigned)
        .output()
        .unwrap();
    assert_eq!(no.status.code(), Some(1));
    let words = String::from_utf8_lossy(&no.stdout);
    assert!(
        words.contains("would not open here") && words.contains("allows only signed apps"),
        "{words}"
    );
    let yes = fleet
        .krate()
        .args(["policy", "check", "--json"])
        .arg(&signed)
        .output()
        .unwrap();
    assert_eq!(
        yes.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&yes.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&yes.stdout).unwrap();
    assert_eq!(json["opens"], true);
    assert_eq!(json["root"], root_hex.to_ascii_lowercase());

    // Signed by a publisher the policy does not list: the refusal carries a
    // short key for a person, and the check prints the whole root and the
    // line an IT team pastes into the policy to let that publisher in.
    let (other, other_hex) = fleet.key("other");
    let theirs = fleet.app("theirs", &[]);
    fleet.sign(&theirs, &other);
    let out = fleet
        .krate()
        .args(["policy", "check"])
        .arg(&theirs)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let words = String::from_utf8_lossy(&out.stdout);
    let other_hex = other_hex.to_ascii_lowercase();
    assert!(
        words.contains("signed by someone else")
            && words.contains(&format!("key = \"{other_hex}\" }}")),
        "{words}"
    );
    assert!(fleet.records().is_empty(), "a check records nothing");
}

/// `[network] proxy` is the route every connection takes, in place of the
/// environment's proxy, and `krate policy` shows it with any password
/// hidden. Both proxies are local listeners, so nothing leaves the machine.
#[test]
fn the_policy_proxy_replaces_the_environments() {
    let fleet = Fleet::new();
    let (policy_proxy, policy_count) = counting_listener();
    let (env_proxy, env_count) = counting_listener();
    let policy_hostport = policy_proxy.trim_start_matches("http://").to_string();
    fleet.policy(&format!(
        "[network]\nproxy = \"http://me:secret@{policy_hostport}\"\nno_proxy = \"internal.acme.com\"\n"
    ));
    let shown = fleet.krate().arg("policy").output().expect("policy");
    let words = String::from_utf8_lossy(&shown.stdout);
    assert!(
        words.contains(&format!(
            "through the proxy http://me:***@{policy_hostport}, except internal.acme.com"
        )),
        "{words}"
    );
    assert!(
        !words.contains("secret"),
        "the password is never shown: {words}"
    );
    let json: serde_json::Value = serde_json::from_slice(
        &fleet
            .krate()
            .args(["policy", "--json"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(
        json["policy"]["network"]["proxy"],
        format!("http://me:***@{policy_hostport}")
    );
    assert_eq!(json["policy"]["network"]["no_proxy"], "internal.acme.com");

    // A hub that is not on this machine, so the proxy is asked for it.
    let _ = fleet
        .krate()
        .env("KRATE_HUB_URL", "https://hub.example.invalid")
        .env("HTTPS_PROXY", &env_proxy)
        .env("https_proxy", &env_proxy)
        .args(["blocklist", "--update"])
        .output()
        .expect("blocklist");
    assert!(
        wait_for(&policy_count, 1, 10) >= 1,
        "the policy's proxy carried it"
    );
    assert_eq!(
        env_count.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the environment's proxy was not used"
    );
}

// ------------------------------------------------- release control (IC-949 R)

/// R, proof item 7: a withdrawn release is refused at the next check-in and
/// at download, the lease goes, and the words carry the reason. Withdraw
/// needs a reason and the authority (owner, admin, reviewer).
#[test]
fn a_withdrawn_release_is_refused_at_the_next_check_in() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "carol", "dan"]) else {
        eprintln!("skipping a_withdrawn_release_is_refused_at_the_next_check_in: no local port");
        return;
    };
    let root = world.acme();
    let (bundle, digest, link) = world.released_app(&root, "1.0.0");
    let copy = dans_copy(&world, &bundle);
    assert!(world.run_link("dan", &link).status.success());
    assert!(world.lease_record("dan", &digest).unwrap()["answer"].is_object());

    // No reason: clap refuses before anything is sent.
    let bare = world.krate_as("bob", &["org", "release", "withdraw", "acme", &digest]);
    assert!(!bare.status.success());
    assert!(
        stderr_of(&bare).contains("--reason"),
        "{}",
        stderr_of(&bare)
    );
    let blank = world.says(
        "bob",
        &[
            "org", "release", "withdraw", "acme", &digest, "--reason", "  ",
        ],
        false,
    );
    assert!(blank.contains("say why with --reason"), "{blank}");
    let carol = world.says(
        "carol",
        &[
            "org", "release", "withdraw", "acme", &digest, "--reason", "x",
        ],
        false,
    );
    assert!(
        carol.contains(
            "your role in acme cannot withdraw acme's releases: ask an owner, admin or reviewer of acme"
        ),
        "{carol}"
    );

    let said = world.says(
        "bob",
        &[
            "org",
            "release",
            "withdraw",
            "acme",
            &digest,
            "--reason",
            "it loses data on save",
        ],
        true,
    );
    assert!(said.contains("withdrew Acme Demo 1.0.0"), "{said}");
    assert!(
        said.contains("every computer refuses it when it next connects"),
        "{said}"
    );
    assert!(
        said.contains(
            "Offline access already given out holds without a connection until it runs out: 1 person holds it"
        ),
        "{said}"
    );
    assert!(
        said.contains("No release of dev.acme.demo opens now"),
        "{said}"
    );

    // Dan's next check-in, from his own copy: refused, the lease gone.
    let run_copy = org_command(
        &world,
        "dan",
        &["run", copy.to_str().unwrap(), "--headless"],
    )
    .output()
    .expect("run");
    assert!(!run_copy.status.success());
    let words = stderr_of(&run_copy);
    assert!(words.contains("this app no longer opens"), "{words}");
    assert!(words.contains("it loses data on save"), "{words}");
    assert!(!words.contains("{\"error\""), "{words}");
    let record = world.lease_record("dan", &digest).expect("still known");
    assert!(record["answer"].is_null(), "the lease is gone: {record}");
    // And the download is refused the same way.
    let fetched = world.run_link("dan", &link);
    assert!(!fetched.status.success());
    assert!(
        stderr_of(&fetched).contains("this app no longer opens"),
        "{}",
        stderr_of(&fetched)
    );
    // Not undone.
    let again = world.says("bob", &["org", "approve", "acme", &digest], false);
    assert!(again.contains("a withdrawal is not undone"), "{again}");
}

/// R: the newest approval is what the app's link opens; setting an older
/// approved release current is the rollback, and the older one is what
/// opens. Only owners and admins choose.
#[test]
fn setting_an_older_release_current_is_what_the_app_link_opens() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "carol", "dan"]) else {
        eprintln!(
            "skipping setting_an_older_release_current_is_what_the_app_link_opens: no local port"
        );
        return;
    };
    let root = world.acme();
    let (_b1, d1, _l1) = world.released_app(&root, "1.0.0");
    let (_b2, d2, _l2) = world.released_app(&root, "1.0.1");
    let app_link = format!("{}/orgs/acme/apps/dev.acme.demo/current", world.url());

    let now = world.says(
        "dan",
        &["org", "release", "current", "acme", "dev.acme.demo"],
        true,
    );
    assert!(
        now.contains(&format!(
            "dev.acme.demo opens Acme Demo 1.0.1 ({})",
            &d2[..12]
        )),
        "{now}"
    );
    assert!(now.contains(&format!("krate run {app_link}")), "{now}");
    assert!(world.run_link("dan", &app_link).status.success());
    assert!(world.lease_record("dan", &d2).is_some(), "1.0.1 opened");
    assert!(world.lease_record("dan", &d1).is_none(), "1.0.0 did not");

    let carol = world.says(
        "carol",
        &["org", "release", "current", "acme", "dev.acme.demo", &d1],
        false,
    );
    assert!(
        carol.contains("your role in acme cannot choose which release of an app opens"),
        "{carol}"
    );
    let set = world.says(
        "alice",
        &["org", "release", "current", "acme", "dev.acme.demo", &d1],
        true,
    );
    assert!(
        set.contains("dev.acme.demo now opens Acme Demo 1.0.0"),
        "{set}"
    );
    assert!(world.run_link("dan", &app_link).status.success());
    assert!(
        world.lease_record("dan", &d1).is_some(),
        "the older release is what opens"
    );

    let listed = world.says("dan", &["org", "releases", "acme"], true);
    let at_d1 = listed.find(&d1[..12]).expect("1.0.0 listed");
    let opens = listed
        .find("opens now for its app (dev.acme.demo)")
        .expect("current marked");
    assert!(opens > at_d1, "the current mark follows 1.0.0: {listed}");
    let audit = world.says("alice", &["org", "audit", "acme"], true);
    assert!(audit.contains("release.current"), "{audit}");
}

/// R: a release limited to a group refuses a member outside it, in words
/// that name the group; owners and reviewers pass. Groups are owners' and
/// admins'.
#[test]
fn a_member_outside_the_audience_is_refused_and_an_owner_passes() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "carol", "dan"]) else {
        eprintln!(
            "skipping a_member_outside_the_audience_is_refused_and_an_owner_passes: no local port"
        );
        return;
    };
    let root = world.acme();
    let (_bundle, digest, link) = world.released_app(&root, "1.0.0");

    let carol = world.says(
        "carol",
        &["org", "group", "create", "acme", "finance"],
        false,
    );
    assert!(
        carol.contains("your role in acme cannot manage acme's groups"),
        "{carol}"
    );
    let made = world.says(
        "alice",
        &["org", "group", "create", "acme", "Finance"],
        true,
    );
    assert!(made.contains("made the group finance in acme"), "{made}");
    let added = world.says(
        "alice",
        &["org", "group", "add", "acme", "finance", "carol"],
        true,
    );
    assert!(added.contains("carol (GitHub) is in finance"), "{added}");
    let listed = world.says("alice", &["org", "group", "list", "acme"], true);
    assert!(listed.contains("finance  carol (GitHub)"), "{listed}");
    let missing = world.says(
        "alice",
        &[
            "org", "release", "audience", "acme", &digest, "--groups", "legal",
        ],
        false,
    );
    assert!(
        missing.contains("Make it first with `krate org group create acme <name>`"),
        "{missing}"
    );

    let limited = world.says(
        "alice",
        &[
            "org", "release", "audience", "acme", &digest, "--groups", "finance",
        ],
        true,
    );
    assert!(
        limited.contains("opens only for finance in acme"),
        "{limited}"
    );

    let refused = world.run_link("dan", &link);
    assert!(!refused.status.success());
    let words = stderr_of(&refused);
    assert!(words.contains("this app does not open for you"), "{words}");
    assert!(words.contains("finance"), "{words}");
    assert!(
        world.run_link("carol", &link).status.success(),
        "in the group"
    );
    assert!(
        world.run_link("alice", &link).status.success(),
        "an owner passes"
    );
    assert!(
        world.run_link("bob", &link).status.success(),
        "a reviewer passes"
    );
    // Dan does not see it listed.
    let dans = world.says("dan", &["org", "releases", "acme"], true);
    assert!(!dans.contains(&digest[..12]), "{dans}");

    world.says(
        "alice",
        &["org", "release", "audience", "acme", &digest, "--everyone"],
        true,
    );
    assert!(
        world.run_link("dan", &link).status.success(),
        "everyone again"
    );
    let out = world.says(
        "alice",
        &["org", "group", "remove", "acme", "finance", "carol"],
        true,
    );
    assert!(out.contains("carol (GitHub) is out of finance"), "{out}");
}

/// `who` on a new laptop: the organisation device this computer made is
/// gone, so the next Krate run registers a new one that holds no key yet --
/// the first open a person makes after a teammate's computer last ran Krate.
/// Every `krate org` command passes keys on in the background (IC-958), so
/// without this the waiting path would only be reachable by timing.
fn new_laptop(world: &OrgWorld, who: &str) {
    let keys = world.homes[who].path().join(".krate/org-keys");
    if keys.exists() {
        std::fs::remove_dir_all(&keys).expect("forget this computer's device");
    }
}

/// IC-950 through the CLI, against a real hub (its validator gives each
/// capability its plain words, and its store compares a candidate with the
/// version that opens now): an update that adds a capability is listed as
/// new, refused before anything is sent until each is named with --accept,
/// and approved once it is.
#[test]
fn an_update_that_adds_a_capability_is_approved_only_once_it_is_named() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "carol"]) else {
        eprintln!("skipping an_update_that_adds_a_capability_is_approved_only_once_it_is_named: no local port");
        return;
    };
    if world.real_hub().is_none() {
        eprintln!(
            "skipping an_update_that_adds_a_capability_is_approved_only_once_it_is_named: \
             the comparison is the hub's (cloud/worker/test/org-review.test.mjs); set \
             KRATE_ORG_E2E_HUB to run it through the CLI"
        );
        return;
    }
    let root = world.acme();
    let (_bundle, _digest, _link) = world.released_app(&root, "1.0.0");
    let update = world.bundle_asking(
        "app-1.1.0.krate",
        "1.1.0",
        65,
        "\n[[capabilities]]\ncap = \"net.connect:api.acme.example:443\"\nrationale = \"send expenses\"\nrequired = true\n",
    );
    world.sign_delegated(&root, "carol", &update);
    let digest = OrgWorld::digest_in(&world.says(
        "carol",
        &["publish", update.to_str().unwrap(), "--org", "acme"],
        true,
    ));
    let listed = world.says("bob", &["org", "candidates", "acme"], true);
    assert!(
        listed.contains("new since 1.0.0, check each before approving:")
            && listed.contains("connect to api.acme.example:443 over the network (net.connect:api.acme.example:443)"),
        "{listed}"
    );
    let refused = world.says("bob", &["org", "approve", "acme", &digest], false);
    assert!(
        refused.contains("nothing was approved: Acme Demo 1.1.0 asks for more than 1.0.0")
            && refused.contains(&format!(
                "krate org approve acme {digest} --accept net.connect:api.acme.example:443"
            )),
        "{refused}"
    );
    let approved = world.says(
        "bob",
        &[
            "org",
            "approve",
            "acme",
            &digest,
            "--accept",
            "net.connect:api.acme.example:443",
        ],
        true,
    );
    assert!(approved.contains("approved."), "{approved}");
}

// ---------------------------------------------------------------------------
// Company connections (IC-952).
// ---------------------------------------------------------------------------

/// One request a local API received: method, path, and every header.
type ApiRequest = (String, String, Vec<(String, String)>);

/// A local stand-in for a company's API that records every request it
/// receives, headers included, and never echoes them back. `/moved` under
/// any path answers a redirect to `redirect_to`.
struct RecordingApi {
    addr: SocketAddr,
    seen: std::sync::Arc<std::sync::Mutex<Vec<ApiRequest>>>,
    _stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for RecordingApi {
    fn drop(&mut self) {
        self._stop.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

impl RecordingApi {
    fn start(redirect_to: Option<String>) -> Option<RecordingApi> {
        let seen: std::sync::Arc<std::sync::Mutex<Vec<ApiRequest>>> = Default::default();
        let log = seen.clone();
        let (addr, stop) = spawn_fake_hub(std::sync::Arc::new(move |method, path, headers, _| {
            log.lock()
                .unwrap()
                .push((method.to_string(), path.to_string(), headers.to_vec()));
            match &redirect_to {
                Some(to) if path.ends_with("/moved") => (
                    302,
                    vec![("Location".into(), to.clone())],
                    b"moved".to_vec(),
                ),
                _ => (200, vec![], br#"{"invoices":3}"#.to_vec()),
            }
        }))?;
        Some(RecordingApi {
            addr,
            seen,
            _stop: stop,
        })
    }

    fn count(&self) -> usize {
        self.seen.lock().unwrap().len()
    }

    /// The `authorization` header of every request so far.
    fn authorizations(&self) -> Vec<Option<String>> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|(_, _, headers)| {
                headers
                    .iter()
                    .find(|(k, _)| k == "authorization")
                    .map(|(_, v)| v.clone())
            })
            .collect()
    }
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Every file under `dir`, read whole.
fn files_under(dir: &std::path::Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(bytes) = std::fs::read(&path) {
                out.push((path, bytes));
            }
        }
    }
    out
}

impl OrgWorld {
    /// The connection probe (apps/krate-connection-probe) packed under
    /// `id` at `version`, asking for `net.connection:finance-api` or not.
    fn probe_bundle(&self, file: &str, id: &str, version: &str, asks: bool) -> PathBuf {
        let dir = self.dir.path().join(format!("src-{file}"));
        std::fs::create_dir_all(&dir).expect("dir");
        let wasm = dir.join("code.wasm");
        std::fs::write(&wasm, include_bytes!("fixtures/connection-probe.wasm")).expect("probe");
        let mut manifest = format!(
            "[app]\nid = \"{id}\"\nname = \"Ledger\"\nversion = \"{version}\"\n\
             entry = \"code.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n\n\
             [[capabilities]]\ncap = \"io.stdout\"\nrationale = \"print\"\nrequired = true\n\n\
             [[capabilities]]\ncap = \"io.args\"\nrationale = \"args\"\nrequired = true\n"
        );
        if asks {
            manifest.push_str(
                "\n[[capabilities]]\ncap = \"net.connection:finance-api\"\n\
                 rationale = \"read invoices\"\nrequired = false\n",
            );
        }
        let manifest_path = dir.join("manifest.toml");
        std::fs::write(&manifest_path, manifest).expect("manifest");
        let bundle = self.dir.path().join(file);
        let packed = krate()
            .arg("pack")
            .arg(&wasm)
            .arg("--manifest")
            .arg(&manifest_path)
            .arg("-o")
            .arg(&bundle)
            .output()
            .expect("pack");
        assert!(
            packed.status.success(),
            "{}",
            String::from_utf8_lossy(&packed.stderr)
        );
        bundle
    }

    /// Carol signs and publishes `bundle`, Bob approves it: its digest and link.
    fn release(&self, root: &std::path::Path, bundle: &std::path::Path) -> (String, String) {
        self.sign_delegated(root, "carol", bundle);
        let digest = OrgWorld::digest_in(&self.says(
            "carol",
            &["publish", bundle.to_str().unwrap(), "--org", "acme"],
            true,
        ));
        self.says("bob", &["org", "approve", "acme", &digest], true);
        (digest.clone(), format!("{}/a/{digest}", self.url()))
    }

    /// `who` runs the probe from `link`, asking it for `method url`.
    fn probe(&self, who: &str, link: &str, args: &[&str]) -> std::process::Output {
        let mut all = vec![
            "run",
            link,
            "--insecure-http",
            "--auto-grant",
            "--headless",
            "--",
        ];
        all.extend_from_slice(args);
        self.krate_as(who, &all)
    }
}

/// The first line the probe printed: its outcome.
fn probe_said(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .to_string()
}

/// IC-952, the contract's proof: an organisation's app reaches the
/// company's API with a credential it never holds. The credential arrives
/// at the API and is never in the app's memory, its output, or anything
/// written on the person's machine; a wrong method, a wrong path, an app
/// that did not ask, a person outside the connection's audience, an app the
/// connection does not allow, a removed connection and a removed member are
/// each refused, and nothing reaches the API; a redirect off the base never
/// carries it; a replaced credential is the one sent at the next run.
///
/// Against the fake hub by default; set KRATE_ORG_E2E_HUB to a local
/// `wrangler dev` hub (with ORG_SECRET_KEY) to run it against the real one.
#[test]
fn a_company_connection_reaches_the_api_and_the_app_never_holds_the_credential() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "carol", "dan"]) else {
        eprintln!("skipping the company connection proof: no local port");
        return;
    };
    let Some(elsewhere) = RecordingApi::start(None) else {
        return;
    };
    let steal = format!("http://{}/steal", elsewhere.addr);
    let Some(api) = RecordingApi::start(Some(steal.clone())) else {
        eprintln!(
            "skipping a_company_connection_reaches_the_api...: no local port for the stand-in API"
        );
        return;
    };
    let base = format!("http://{}/finance", api.addr);
    let secret = "Bearer sk-live-acme-0f1e2d3c4b5a69788796a5b4c3d2e1f0";
    let rotated = "Bearer sk-live-acme-rotated-11223344556677889900aabb";
    let secret_file = world.dir.path().join("secret.txt");
    std::fs::write(&secret_file, format!("{secret}\n")).expect("secret");

    let root = world.acme();
    world.says(
        "alice",
        &["org", "group", "create", "acme", "finance"],
        true,
    );
    world.says(
        "alice",
        &["org", "group", "add", "acme", "finance", "dan@acme.example"],
        true,
    );
    // Only an owner or admin sets one up, and the credential never comes
    // back in what is printed.
    let create = [
        "org",
        "connection",
        "create",
        "acme",
        "finance-api",
        "--base",
        &base,
        "--header",
        "Authorization",
        "--value-file",
        secret_file.to_str().unwrap(),
        "--allow",
        "GET",
        "--paths",
        "/invoices",
        "--groups",
        "finance",
        "--apps",
        "com.acme.ledger",
    ];
    let refused = world.says("carol", &create, false);
    assert!(refused.contains("your role in acme cannot"), "{refused}");
    let made = world.says("alice", &create, true);
    assert!(made.contains("set up the finance-api connection"), "{made}");
    assert!(made.contains("net.connection:finance-api"), "{made}");
    assert!(
        !made.contains("sk-live"),
        "the credential is never printed: {made}"
    );
    let listed = world.says("carol", &["org", "connection", "list", "acme"], true);
    assert!(
        listed.contains(&base) && listed.contains("Authorization"),
        "{listed}"
    );
    assert!(!listed.contains("sk-live"), "{listed}");
    assert!(
        made.contains("Krate cannot read it") && listed.contains("encrypted: Krate cannot read"),
        "{made}{listed}"
    );
    // Nothing but the sealed credential left Alice's computer.
    if world.real_hub().is_none() {
        for body in &world.hub.lock().unwrap().bodies {
            assert!(
                !contains_bytes(body, b"sk-live"),
                "the credential left the machine in the clear: {}",
                String::from_utf8_lossy(body)
            );
        }
    }

    let ledger = world.probe_bundle("ledger.krate", "com.acme.ledger", "1.0.0", true);
    let (digest, link) = world.release(&root, &ledger);
    let invoices = format!("{base}/invoices");

    // 0. The credential is encrypted to acme's key (F-316), and Dan's
    // computer does not have that key yet: the app opens, says why in plain
    // words, and nothing reaches the API.
    new_laptop(&world, "dan");
    let early = world.probe("dan", &link, &["GET", &invoices]);
    assert!(early.status.success(), "{}", stderr_of(&early));
    assert!(
        probe_said(&early).contains("does not have acme's encryption key yet"),
        "{}",
        probe_said(&early)
    );
    assert!(
        stderr_of(&early).contains("which opens without it"),
        "{}",
        stderr_of(&early)
    );
    assert_eq!(api.count(), 0, "nothing reached the API");
    // Alice's computer is online once: it passes the key to Dan's.
    let shown = world.says("alice", &["org", "keys", "show", "acme"], true);
    assert!(shown.contains("holds acme's key, version 1"), "{shown}");

    // 1. Dan, in finance, opens the app: the request arrives with the
    // credential, and the app's own Authorization header does not survive.
    let ran = world.probe(
        "dan",
        &link,
        &[
            "GET",
            &invoices,
            "Authorization",
            "Bearer app-guess",
            "dump",
        ],
    );
    assert!(ran.status.success(), "{}", stderr_of(&ran));
    assert_eq!(probe_said(&ran), r#"status=200 body={"invoices":3}"#);
    assert_eq!(api.authorizations(), vec![Some(secret.to_string())]);
    // Never in the app's memory (the probe dumped all of it to stdout), its
    // output, or anything on Dan's machine.
    assert!(
        String::from_utf8_lossy(&ran.stdout).contains("--- memory ---"),
        "the probe dumped its memory"
    );
    assert!(ran.stdout.len() > 65_536, "a whole memory, not a stub");
    assert!(
        !contains_bytes(&ran.stdout, b"sk-live"),
        "not in the app's memory or stdout"
    );
    assert!(!contains_bytes(&ran.stderr, b"sk-live"), "not in stderr");
    for (path, bytes) in files_under(world.homes["dan"].path()) {
        assert!(
            !contains_bytes(&bytes, b"sk-live"),
            "the credential was written to {}",
            path.display()
        );
    }
    let audit = world.says("alice", &["org", "audit", "acme"], true);
    assert!(!audit.contains("sk-live"), "{audit}");

    // 2. A method or a path the connection does not allow: refused in words,
    // and nothing reaches the API.
    let post = world.probe("dan", &link, &["POST", &invoices]);
    assert!(
        probe_said(&post)
            .starts_with("error=other the finance-api connection lets this app use GET only"),
        "{}",
        probe_said(&post)
    );
    let payroll = world.probe("dan", &link, &["GET", &format!("{base}/payroll")]);
    assert!(
        probe_said(&payroll).contains("does not reach /payroll"),
        "{}",
        probe_said(&payroll)
    );
    assert_eq!(api.count(), 1, "neither reached the API");

    // 3. A redirect off the base is not followed, and the place it points
    // never sees the credential, even when the app goes there itself.
    let moved = world.probe("dan", &link, &["GET", &format!("{base}/invoices/moved")]);
    assert!(
        probe_said(&moved).starts_with("status=302"),
        "{}",
        probe_said(&moved)
    );
    let follow = world.probe("dan", &link, &["GET", &steal]);
    assert_eq!(probe_said(&follow), "error=permission-denied");
    assert_eq!(elsewhere.count(), 0, "the redirect's target saw nothing");

    // 4. A version of the app that did not ask for the connection gets no
    // credential; its request to the base is an ordinary one it may not make.
    let plain = world.probe_bundle("ledger-plain.krate", "com.acme.ledger", "1.0.1", false);
    let (_, plain_link) = world.release(&root, &plain);
    let asked = api.count();
    let without = world.probe("dan", &plain_link, &["GET", &invoices]);
    assert_eq!(probe_said(&without), "error=permission-denied");
    assert_eq!(api.count(), asked, "nothing reached the API");

    // 5. Bob is a member, outside the connection's audience: the app opens,
    // its request is refused with the reason.
    let bobs = world.probe("bob", &link, &["GET", &invoices]);
    assert!(bobs.status.success(), "{}", stderr_of(&bobs));
    assert!(
        probe_said(&bobs).contains("only for the finance group"),
        "{}",
        probe_said(&bobs)
    );
    assert!(
        stderr_of(&bobs).contains("finance-api connection is not available"),
        "{}",
        stderr_of(&bobs)
    );

    // 6. An app the connection does not allow.
    let other = world.probe_bundle("other.krate", "com.acme.other", "1.0.0", true);
    let (_, other_link) = world.release(&root, &other);
    let others = world.probe("dan", &other_link, &["GET", &invoices]);
    assert!(
        probe_said(&others).contains("has not let"),
        "{}",
        probe_said(&others)
    );
    assert_eq!(api.count(), asked, "no refused request reached the API");

    // 7. The credential is replaced: the next run sends the new one.
    let rotated_file = world.dir.path().join("rotated.txt");
    std::fs::write(&rotated_file, rotated).expect("rotated");
    let replaced = world.says(
        "alice",
        &[
            "org",
            "connection",
            "replace",
            "acme",
            "finance-api",
            "--header",
            "Authorization",
            "--value-file",
            rotated_file.to_str().unwrap(),
        ],
        true,
    );
    assert!(replaced.contains("next time it opens"), "{replaced}");
    let again = world.probe("dan", &link, &["GET", &invoices]);
    assert_eq!(probe_said(&again), r#"status=200 body={"invoices":3}"#);
    assert_eq!(
        api.authorizations().last().cloned().flatten().as_deref(),
        Some(rotated)
    );

    // 8. No answer from the hub for the credential: nothing is sent, and the
    // words say why (the fake hub only; a real one cannot be made to fail).
    if world.real_hub().is_none() {
        world.hub.lock().unwrap().connections_down = true;
        let before = api.count();
        let down = world.probe("dan", &link, &["GET", &invoices]);
        assert!(
            probe_said(&down).contains("could not be reached"),
            "{}",
            probe_said(&down)
        );
        assert_eq!(api.count(), before);
        world.hub.lock().unwrap().connections_down = false;
    }

    // 9. The connection is removed: its address is refused from the next run.
    world.says(
        "alice",
        &["org", "connection", "remove", "acme", "finance-api"],
        true,
    );
    let before = api.count();
    let gone = world.probe("dan", &link, &["GET", &invoices]);
    assert!(
        probe_said(&gone).contains("any more"),
        "{}",
        probe_said(&gone)
    );
    assert_eq!(api.count(), before);

    // 10. A removed member: the app does not open at all, and the hub would
    // not hand the credential over either.
    world.says(
        "alice",
        &[
            "org",
            "connection",
            "create",
            "acme",
            "finance-api",
            "--base",
            &base,
            "--header",
            "Authorization",
            "--value-file",
            secret_file.to_str().unwrap(),
        ],
        true,
    );
    // What Dan's computer holds of acme's key before he is removed: his
    // device key (kept sealed with his computer's machine key) and the
    // keyring his envelope opens to. He keeps this copy.
    let dans_home = world.homes["dan"].path().join(".krate");
    let machine = std::fs::read(dans_home.join("machine.key")).expect("Dan's machine key");
    let stored: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dans_home.join("org-keys/device.json")).expect("Dan's device"),
    )
    .unwrap();
    let dans_device = krate_orgkeys::KeyPair::from_secret(
        &krate_orgkeys::open_local(
            &machine,
            "krate org device key v1",
            stored["sealed"].as_str().unwrap(),
        )
        .expect("Dan's device key opens"),
    )
    .unwrap();
    let dans_view: serde_json::Value = serde_json::from_str(
        &ureq::get(&format!(
            "{}/orgs/acme/keys?device={}",
            world.url(),
            dans_device.id()
        ))
        .set("Authorization", "Bearer tok-dan@acme.example")
        .call()
        .expect("Dan's keyring view")
        .into_string()
        .unwrap(),
    )
    .unwrap();
    let dans_keyring = krate_orgkeys::Keyring::open(
        dans_view["envelope"]["blob"]
            .as_str()
            .expect("Dan holds the key"),
        &dans_device,
        "acme",
    )
    .expect("Dan's envelope opens");
    assert_eq!(dans_keyring.versions(), vec![1]);
    world.says(
        "alice",
        &["org", "remove", "acme", "dan@acme.example"],
        true,
    );
    let removed = world.probe("dan", &link, &["GET", &invoices]);
    assert!(!removed.status.success());
    assert!(
        stderr_of(&removed).contains("removed"),
        "{}",
        stderr_of(&removed)
    );
    let asked_directly = ureq::get(&format!(
        "{}/orgs/acme/connections/finance-api/credential/{digest}",
        world.url()
    ))
    .set("Authorization", "Bearer tok-dan@acme.example")
    .call();
    match asked_directly {
        Err(ureq::Error::Status(403, response)) => {
            let body = response.into_string().unwrap_or_default();
            assert!(body.contains("\"removed\""), "{body}");
            assert!(!body.contains("sk-live"), "{body}");
        }
        other => panic!("a removed member got {other:?}"),
    }
    assert_eq!(api.count(), before, "nothing reached the API");
    // The credential was handed out for the two runs that used it, and to
    // nobody else (the fake counts; a real hub keeps its own daily count).
    if world.real_hub().is_none() {
        assert!(world.hub.lock().unwrap().connection_fetches >= 2);
    }

    // 11. Dan's removal replaced acme's key: the next member computer
    // online (Alice's) makes version 2 and encrypts the credential again to
    // it. What the hub hands out now does not open with the copy Dan kept,
    // and still reaches the API for Alice.
    let shown = world.says("alice", &["org", "keys", "show", "acme"], true);
    assert!(shown.contains("holds acme's key, version 2"), "{shown}");
    let handed: serde_json::Value = serde_json::from_str(
        &ureq::get(&format!(
            "{}/orgs/acme/connections/finance-api/credential/{digest}",
            world.url()
        ))
        .set("Authorization", "Bearer tok-alice")
        .call()
        .expect("Alice's hand-out")
        .into_string()
        .unwrap(),
    )
    .unwrap();
    assert_eq!(handed["credential"]["v"], 2, "{handed}");
    let context = krate_orgkeys::connection_context(
        "acme",
        "finance-api",
        handed["credential"]["header"].as_str().unwrap(),
        handed["base"].as_str().unwrap(),
    );
    let sealed = handed["credential"]["sealed"].as_str().unwrap();
    assert!(!sealed.contains("sk-live"));
    for v in [1, 2] {
        assert!(
            krate_orgkeys::open_for_org(&dans_keyring, v, &context, sealed).is_err(),
            "Dan's kept keyring opened the new credential at version {v}"
        );
    }
    let alices = world.probe("alice", &link, &["GET", &invoices]);
    assert_eq!(
        probe_said(&alices),
        r#"status=200 body={"invoices":3}"#,
        "{}",
        stderr_of(&alices)
    );
    assert_eq!(
        api.authorizations().last().cloned().flatten().as_deref(),
        Some(secret)
    );

    // Nothing the hub keeps, or wrote to its log, holds either credential,
    // though it took two creates and a replace, handed them out and saw a
    // rotation (a real hub: KRATE_ORG_E2E_STATE names its --persist-to
    // directory, KRATE_ORG_E2E_LOG the file its output went to).
    let clear = |bytes: &[u8]| {
        [secret, rotated].iter().any(|s| {
            contains(bytes, &s[7..])
                || contains(bytes, &krate_orgkeys::b64_encode(s.as_bytes())[..24])
        })
    };
    if world.real_hub().is_none() {
        let hub = world.hub.lock().unwrap();
        for body in &hub.bodies {
            assert!(!clear(body), "{}", String::from_utf8_lossy(body));
        }
    }
    for var in ["KRATE_ORG_E2E_STATE", "KRATE_ORG_E2E_LOG"] {
        if let Ok(path) = std::env::var(var) {
            let path = std::path::Path::new(&path);
            let mut kept = Vec::new();
            if path.is_dir() {
                all_bytes_under(path, &mut kept);
            } else {
                kept = std::fs::read(path).expect(var);
            }
            assert!(!kept.is_empty(), "{var} names nothing");
            assert!(!clear(&kept), "{var} holds a credential");
        }
    }
}

// ---------------------------------------------------------------------------
// store.org (IC-951), end to end encrypted (F-316): an organisation's team
// data, through the binary. The probe is apps/krate-orgstore-probe built
// (fixtures/orgstore-probe.wasm): `put`, `get`, `delete`, `list`, `sync`,
// `who` and `status` steps joined by `then`, one line each. The fake hub answers
// /orgs/<slug>/keys and /orgs/<slug>/data/<digest>/sync the way
// cloud/worker/src/org-keys.js and org-data.js do, holding only what the
// devices send; with KRATE_ORG_E2E_HUB set, the online test runs against a
// real hub, and with KRATE_ORG_E2E_STATE naming its --persist-to directory,
// that hub's stored bytes are searched for the plaintext too.
// ---------------------------------------------------------------------------

/// The probe, packed as `dev.acme.tasks` at `version`, as Carol builds it.
fn probe_bundle(world: &OrgWorld, file: &str, version: &str) -> PathBuf {
    let dir = world.dir.path().join(format!("probe-{file}"));
    std::fs::create_dir_all(&dir).expect("dir");
    let wasm = dir.join("code.wasm");
    std::fs::write(&wasm, include_bytes!("fixtures/orgstore-probe.wasm")).expect("probe");
    let manifest = dir.join("manifest.toml");
    std::fs::write(
        &manifest,
        format!(
            "[app]\nid = \"dev.acme.tasks\"\nname = \"Acme Tasks\"\nversion = \"{version}\"\n\
             entry = \"code.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n\n\
             [[capabilities]]\ncap = \"io.stdout\"\nrationale = \"print\"\nrequired = true\n\n\
             [[capabilities]]\ncap = \"io.args\"\nrationale = \"steps\"\nrequired = true\n\n\
             [[capabilities]]\ncap = \"store.org\"\nrationale = \"the team's records\"\nrequired = true\n"
        ),
    )
    .expect("manifest");
    let bundle = world.dir.path().join(file);
    let packed = krate()
        .arg("pack")
        .arg(&wasm)
        .arg("--manifest")
        .arg(&manifest)
        .arg("-o")
        .arg(&bundle)
        .output()
        .expect("pack");
    assert!(
        packed.status.success(),
        "{}",
        String::from_utf8_lossy(&packed.stderr)
    );
    bundle
}

/// Carol's probe, signed for acme, published, approved by Bob: the bundle
/// and its digest.
fn released_probe(world: &OrgWorld, root: &std::path::Path, version: &str) -> (PathBuf, String) {
    let bundle = probe_bundle(world, &format!("tasks-{version}.krate"), version);
    world.sign_delegated(root, "carol", &bundle);
    let digest = OrgWorld::digest_in(&world.says(
        "carol",
        &["publish", bundle.to_str().unwrap(), "--org", "acme"],
        true,
    ));
    world.says("bob", &["org", "approve", "acme", &digest], true);
    (bundle, digest)
}

/// Run the probe from `who`'s own copy of `bundle` with these steps;
/// stdout's lines, and stderr.
fn probe(
    world: &OrgWorld,
    who: &str,
    bundle: &std::path::Path,
    steps: &[&str],
    env: &[(&str, &str)],
) -> (Vec<String>, String) {
    let copy = world.homes[who].path().join("Acme Tasks.krate");
    std::fs::copy(bundle, &copy).expect("copy");
    let mut args = vec![
        "run",
        copy.to_str().unwrap(),
        "--headless",
        "--auto-grant",
        "--",
    ];
    args.extend_from_slice(steps);
    let mut command = org_command(world, who, &args);
    for (k, v) in env {
        command.env(k, v);
    }
    let out = command.output().expect("run");
    let stdout = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect();
    (stdout, stderr_of(&out))
}

/// `who` runs `krate org keys show acme`: their computer does its share of
/// the keyring (registers, takes the key, passes it on) and says so.
fn keys_show(world: &OrgWorld, who: &str) -> String {
    world.says(who, &["org", "keys", "show", "acme"], true)
}

/// G17: the machine policy's per-app rules are shown by `krate policy`, and
/// a required capability one of them blocks stops the app it names from
/// opening -- only that app.
#[test]
fn per_app_rules_are_shown_and_stop_only_the_app_they_name() {
    let fleet = Fleet::new();
    fleet.policy(
        "[apps.\"*\"]\nwipe_when_offline_ends = true\n\
         [apps.\"com.acme.copier\"]\nclipboard_write = false\nprinting = false\n",
    );
    let shown = fleet.krate().arg("policy").output().expect("policy");
    let words = String::from_utf8_lossy(&shown.stdout);
    assert!(
        words.contains("com.acme.copier: no copying out by the clipboard; no printing."),
        "{words}"
    );
    assert!(
        words.contains(
            "Every organisation app: team data deleted from this computer when offline access ends."
        ),
        "{words}"
    );
    let json = fleet
        .krate()
        .args(["policy", "--json"])
        .output()
        .expect("policy --json");
    let json: serde_json::Value = serde_json::from_slice(&json.stdout).expect("json");
    let rules = &json["policy"]["apps"]["com.acme.copier"];
    assert_eq!(rules["clipboard_write"], false, "{json}");
    assert_eq!(rules["printing"], false, "{json}");
    assert_eq!(rules["clipboard_read"], true, "{json}");
    // Windowed apps that need to copy out: the same bytes under two ids.
    let windowed = |name: &str| -> PathBuf {
        let wasm = fleet.path("code.wasm");
        std::fs::write(
            &wasm,
            include_bytes!("../../bundle/tests/fixtures/minimal-run.wasm"),
        )
        .expect("component");
        let manifest = fleet.path(&format!("{name}.toml"));
        std::fs::write(
            &manifest,
            format!(
                "[app]\nid = \"com.acme.{name}\"\nname = \"{name}\"\nversion = \"1.0.0\"\n\
                 entry = \"code.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n\n\
                 [[capabilities]]\ncap = \"ui.clipboard:write\"\nrationale = \"copy\"\nrequired = true\n"
            ),
        )
        .expect("manifest");
        let bundle = fleet.path(&format!("{name}.krate"));
        let out = krate()
            .arg("pack")
            .arg(&wasm)
            .arg("--manifest")
            .arg(&manifest)
            .arg("-o")
            .arg(&bundle)
            .output()
            .expect("pack");
        assert!(out.status.success(), "pack: {}", stderr_of(&out));
        bundle
    };
    let copier = windowed("copier");
    let other = windowed("other");
    let refused = fleet
        .krate()
        .args(["policy", "check"])
        .arg(&copier)
        .output()
        .expect("check");
    assert_eq!(refused.status.code(), Some(1), "{}", stderr_of(&refused));
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&refused.stdout),
        stderr_of(&refused)
    );
    assert!(
        said.contains("which your organisation (Acme Labs) blocks"),
        "{said}"
    );
    let opens = fleet
        .krate()
        .args(["policy", "check"])
        .arg(&other)
        .output()
        .expect("check");
    assert_eq!(opens.status.code(), Some(0), "{}", stderr_of(&opens));
}

/// G17: under `[apps."*"] wipe_when_offline_ends = true`, an organisation
/// app whose offline access has run out leaves no team data on the
/// computer; without the rule the data stays (the app still does not open).
#[test]
fn team_data_goes_when_offline_access_ends_where_the_policy_says() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "carol"]) else {
        eprintln!("skipping team_data_goes_when_offline_access_ends_...: no local port");
        return;
    };
    if world.real_hub().is_some() {
        eprintln!("skipping team_data_goes_when_offline_access_ends_...: it stops the hub");
        return;
    }
    let root = world.acme();
    let (bundle, digest) = released_probe(&world, &root, "1.0.0");
    let (out, err) = probe(
        &world,
        "carol",
        &bundle,
        &["put", "tasks", "t1", "title=kept here", "then", "sync"],
        &[],
    );
    assert_eq!(out[0], "put t1 ok", "{err}");
    assert!(
        !org_data_files(&world, "carol").is_empty(),
        "the copy is here"
    );
    // Offline, and the lease ran out a day ago.
    world.stop.store(true, std::sync::atomic::Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(100));
    let lease_file = world.homes["carol"]
        .path()
        .join(".krate/leases")
        .join(format!("{digest}.json"));
    let mut record: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&lease_file).expect("a lease")).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let mut body = record["answer"]["lease"].clone();
    body["issued_at"] = serde_json::json!(now - 8 * 86_400);
    body["expires_at"] = serde_json::json!(now - 86_400);
    let pkcs8 = world.hub.lock().unwrap().lease_pkcs8.clone();
    record["answer"] = krate_lease::sign(&body, &pkcs8).unwrap();
    std::fs::write(&lease_file, record.to_string()).unwrap();
    let keep = world.dir.path().join("keep.toml");
    std::fs::write(&keep, "version = 1\n").unwrap();
    let wipe = world.dir.path().join("wipe.toml");
    std::fs::write(
        &wipe,
        "version = 1\nname = \"Acme\"\n[apps.\"*\"]\nwipe_when_offline_ends = true\n",
    )
    .unwrap();
    let keep_s = keep.to_str().unwrap();
    let (_, err) = probe(
        &world,
        "carol",
        &bundle,
        &["list", "tasks"],
        &[("KRATE_TEST_POLICY", keep_s)],
    );
    assert!(err.contains("its offline access ran out on"), "{err}");
    assert!(!org_data_files(&world, "carol").is_empty(), "no rule: kept");
    let wipe_s = wipe.to_str().unwrap();
    let (_, err) = probe(
        &world,
        "carol",
        &bundle,
        &["list", "tasks"],
        &[("KRATE_TEST_POLICY", wipe_s)],
    );
    assert!(err.contains("its offline access ran out on"), "{err}");
    assert!(
        err.contains("team data for this app was deleted from this computer"),
        "{err}"
    );
    assert!(
        org_data_files(&world, "carol").is_empty(),
        "{:?}",
        org_data_files(&world, "carol")
    );
}

/// The organisation-data files on `who`'s machine.
fn org_data_files(world: &OrgWorld, who: &str) -> Vec<PathBuf> {
    let dir = world.homes[who].path().join(".krate/org-data/acme");
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "json"))
                .collect()
        })
        .unwrap_or_default()
}

/// Every file under `dir`, read whole.
fn all_bytes_under(dir: &std::path::Path, out: &mut Vec<u8>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            all_bytes_under(&path, out);
        } else if let Ok(bytes) = std::fs::read(&path) {
            out.extend_from_slice(&bytes);
        }
    }
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|w| w == needle.as_bytes())
}

/// Two members on two computers each edit one record, each a different
/// field; once both have synced, both see both, with who changed it. The
/// key reaches Dan's computer from Carol's, with no owner involved, and the
/// audit says so; no plaintext ever reaches the hub; a second release of
/// the same app reads the same data; the recovery code restores Alice's
/// key on a new computer; and after Dan is removed the key is replaced, so
/// what is written next is under a version he never had. Online only, so
/// it also runs against a real hub (KRATE_ORG_E2E_HUB).
#[test]
fn org_team_data_is_sealed_shared_by_two_members_and_its_key_travels_device_to_device() {
    let Some(world) = OrgWorld::new(&["alice", "alice-laptop", "bob", "carol", "dan"]) else {
        eprintln!("skipping org_team_data_is_sealed_...: no local port");
        return;
    };
    let fake = world.real_hub().is_none();
    let root = world.acme();
    let created = world.created.lock().unwrap().clone();
    assert!(
        created.contains("made acme's key (version 1) on this computer"),
        "{created}"
    );
    let code = created
        .lines()
        .find_map(|l| l.strip_prefix("recovery code: "))
        .expect("the recovery code is shown once")
        .to_string();
    let (bundle, _) = released_probe(&world, &root, "1.0.0");
    let title = "Order chairs for the hall";
    let id = "task-0001-xyz";

    // Carol's computer registered when she joined, and Bob's computer
    // passed it the key when he approved the release (every `krate org`
    // command does a computer's share, IC-958): her write goes straight up.
    let (out, err) = probe(
        &world,
        "carol",
        &bundle,
        &[
            "put",
            "tasks",
            id,
            &format!("title={title}"),
            "then",
            "sync",
        ],
        &[],
    );
    assert_eq!(out[0], format!("put {id} ok"), "{err}");
    assert_eq!(out[1], "sync online=true changed=false pending=0", "{err}");
    // Alice's computer is online once (any of acme's apps would do): it
    // passes the key to Carol's.
    let shown = keys_show(&world, "alice");
    assert!(
        shown.contains("this computer holds acme's key, version 1"),
        "{shown}"
    );
    let (out, err) = probe(&world, "carol", &bundle, &["sync", "then", "who"], &[]);
    assert!(out[0].ends_with("pending=0"), "{out:?} {err}");
    assert_eq!(out[1], "who carol developer Acme");

    // Dan (a recipient, by email): his computer registered when he joined,
    // and Bob's computer -- a reviewer's, not an owner's -- passed him the
    // key when Bob approved the release, so his own field goes straight up.
    let (out, err) = probe(
        &world,
        "dan",
        &bundle,
        &[
            "put",
            "tasks",
            id,
            "done?=true",
            "then",
            "sync",
            "then",
            "get",
            "tasks",
            id,
        ],
        &[],
    );
    // His sync brings Carol's field down with it.
    assert_eq!(out[1], "sync online=true changed=true pending=0", "{err}");
    assert_eq!(
        out[2],
        format!("get {id} done=true;title={title} by=dan@acme.example")
    );
    let (out, _) = probe(&world, "carol", &bundle, &["sync"], &[]);
    assert!(out[0].starts_with("sync online=true"));
    let (out, err) = probe(
        &world,
        "dan",
        &bundle,
        &["sync", "then", "get", "tasks", id],
        &[],
    );
    assert_eq!(
        out[1],
        format!("get {id} done=true;title={title} by=dan@acme.example"),
        "{out:?} {err}"
    );
    let (out, err) = probe(
        &world,
        "carol",
        &bundle,
        &["sync", "then", "get", "tasks", id, "then", "list", "tasks"],
        &[],
    );
    assert_eq!(
        out[1],
        format!("get {id} done=true;title={title} by=dan@acme.example"),
        "{out:?} {err}"
    );
    assert_eq!(out[2], format!("list {id}"));

    // The audit names each grant: Dan's from Bob's computer.
    if fake {
        let hub = world.hub.lock().unwrap();
        let acme = &hub.orgs["acme"];
        let bobs: Vec<&String> = acme
            .keys
            .devices
            .iter()
            .filter(|(_, d)| d.principal == "github:bob")
            .map(|(id, _)| id)
            .collect();
        let dans = acme
            .audit
            .iter()
            .find(|e| {
                e["action"] == "key.grant"
                    && e["reason"]
                        .as_str()
                        .is_some_and(|r| r.contains("email:dan@acme.example"))
            })
            .expect("a grant to Dan");
        assert_eq!(dans["actor"], "github:bob", "no owner involved: {dans}");
        assert!(
            dans["reason"]
                .as_str()
                .unwrap()
                .ends_with(&format!("by {}", bobs[0])),
            "{dans}"
        );
        // Nothing the devices sent carries the record's id or value, as
        // text or inside a ciphertext.
        for row in acme.data.values() {
            let inside = krate_orgkeys::b64_decode(&row.ct).expect("base64");
            assert!(
                !contains(&inside, title) && !contains(&inside, id),
                "plaintext inside a ciphertext"
            );
        }
        for body in &hub.bodies {
            assert!(
                !contains(body, title) && !contains(body, id),
                "plaintext left a device: {}",
                String::from_utf8_lossy(body)
            );
        }
        // And what the hub stores is opaque: no collection, id, field or value.
        for (key, row) in &acme.data {
            // The app id is the hub's to know; the rest is opaque.
            let stored = format!("{} {} {} {} {}", key.1, key.2, row.kind, row.ct, row.s);
            for plain in [title, id, "tasks", "title", "done"] {
                assert!(!stored.contains(plain), "{plain} in {stored}");
            }
        }
    }
    if let Ok(state) = std::env::var("KRATE_ORG_E2E_STATE") {
        let mut stored = Vec::new();
        all_bytes_under(std::path::Path::new(&state), &mut stored);
        assert!(
            !stored.is_empty(),
            "the hub's state directory has its store"
        );
        assert!(
            !contains(&stored, title) && !contains(&stored, id),
            "the hub's stored bytes hold plaintext"
        );
    }

    // A delete reaches the other computer.
    let (out, err) = probe(
        &world,
        "carol",
        &bundle,
        &["put", "tasks", "temp-2", "title=gone soon", "then", "sync"],
        &[],
    );
    assert!(out[1].ends_with("pending=0"), "{out:?} {err}");
    let (out, _) = probe(
        &world,
        "dan",
        &bundle,
        &["sync", "then", "get", "tasks", "temp-2"],
        &[],
    );
    assert_eq!(out[1], "get temp-2 title=gone soon by=carol");
    let (out, _) = probe(
        &world,
        "carol",
        &bundle,
        &["delete", "tasks", "temp-2", "then", "sync"],
        &[],
    );
    assert_eq!(out[0], "delete temp-2 ok");
    let (out, err) = probe(
        &world,
        "dan",
        &bundle,
        &["sync", "then", "get", "tasks", "temp-2"],
        &[],
    );
    assert_eq!(out[1], "get temp-2 none", "{out:?} {err}");

    // The next release of the same app id: the same data.
    let (v2, _) = released_probe(&world, &root, "1.0.1");
    let (out, err) = probe(
        &world,
        "carol",
        &v2,
        &["sync", "then", "get", "tasks", id],
        &[],
    );
    assert_eq!(
        out[1],
        format!("get {id} done=true;title={title} by=dan@acme.example"),
        "{out:?} {err}"
    );

    // Before that, the laptop is new to acme and holds no key: it says it is
    // waiting for the key, by the organisation's name, keeps the write, and
    // who() is none (K-1108).
    let (out, err) = probe(
        &world,
        "alice-laptop",
        &v2,
        &[
            "put",
            "notes",
            "n-wait",
            "title=kept",
            "then",
            "sync",
            "then",
            "status",
            "then",
            "who",
        ],
        &[],
    );
    assert_eq!(out[0], "put n-wait ok", "{err}");
    assert_eq!(out[1], "sync online=true changed=false pending=1", "{err}");
    assert_eq!(
        out[2], "status waiting-for-key pending=1 reached=true org=Acme",
        "{err}"
    );
    assert_eq!(out[3], "who none", "{err}");

    // Alice's new laptop: the recovery code restores her key, and she
    // reads the data with no other device involved.
    let restored = world.says(
        "alice-laptop",
        &["org", "keys", "recover", "acme", "--code", &code],
        true,
    );
    assert!(
        restored.contains("restored acme's key (version 1)"),
        "{restored}"
    );
    let wrong = world.krate_as(
        "alice-laptop",
        &[
            "org",
            "keys",
            "recover",
            "acme",
            "--code",
            "0000000000-0000000000-000000",
        ],
    );
    assert!(!wrong.status.success());
    assert!(
        stderr_of(&wrong).contains("does not open acme's key"),
        "{}",
        stderr_of(&wrong)
    );
    let (out, err) = probe(
        &world,
        "alice-laptop",
        &v2,
        &[
            "sync", "then", "get", "tasks", id, "then", "who", "then", "status",
        ],
        &[],
    );
    assert_eq!(
        out[1],
        format!("get {id} done=true;title={title} by=dan@acme.example"),
        "{out:?} {err}"
    );
    // The key arrived: who() fills on the same sync, and the write made
    // while waiting went up with it.
    assert_eq!(out[2], "who alice owner Acme", "{out:?} {err}");
    assert_eq!(
        out[3], "status synced pending=0 reached=true org=Acme",
        "{out:?} {err}"
    );

    // An unmarked copy of the same code is not the organisation's app.
    let unmarked = probe_bundle(&world, "unmarked.krate", "1.0.0");
    let before = world.hub.lock().unwrap().seen.len();
    let (out, err) = probe(
        &world,
        "carol",
        &unmarked,
        &[
            "put", "tasks", "t2", "title=x", "then", "sync", "then", "get", "tasks", id,
        ],
        &[],
    );
    assert_eq!(
        out,
        vec![
            "put t2 error=not-an-org-app",
            "sync error=not-an-org-app",
            &format!("get {id} error=not-an-org-app")
        ],
        "{err}"
    );
    if fake {
        assert_eq!(
            world.hub.lock().unwrap().seen.len(),
            before,
            "an unmarked app sends nothing"
        );
    }

    // Dan is removed: the app no longer opens for him and the
    // organisation's data leaves his computer. Carol's computer replaces
    // the key; what she writes next is under a version Dan never held.
    assert!(!org_data_files(&world, "dan").is_empty(), "Dan had a copy");
    world.says(
        "alice",
        &["org", "remove", "acme", "dan@acme.example"],
        true,
    );
    let (out, err) = probe(&world, "dan", &v2, &["get", "tasks", id], &[]);
    assert!(
        out.is_empty() && err.contains("removed from acme"),
        "{out:?} {err}"
    );
    assert!(
        org_data_files(&world, "dan").is_empty(),
        "the data left with him"
    );
    let (out, err) = probe(
        &world,
        "carol",
        &v2,
        &[
            "sync",
            "then",
            "put",
            "tasks",
            "after-removal-7",
            "title=Payroll moves to Friday",
            "then",
            "sync",
        ],
        &[],
    );
    assert!(out[2].ends_with("pending=0"), "{out:?} {err}");
    let shown = keys_show(&world, "carol");
    assert!(
        shown.contains("this computer holds acme's key, version 2"),
        "{shown}"
    );
    if fake {
        let hub = world.hub.lock().unwrap();
        let acme = &hub.orgs["acme"];
        let dans: Vec<&String> = acme
            .keys
            .devices
            .iter()
            .filter(|(_, d)| d.principal == "email:dan@acme.example")
            .map(|(id, _)| id)
            .collect();
        assert!(
            dans.iter().all(|d| !acme.keys.envelopes.contains_key(*d)),
            "Dan's computer holds no envelope"
        );
        let newest = acme.data.values().max_by_key(|r| r.seq).unwrap();
        assert_eq!(newest.ver, 2, "written after the removal under version 2");
    }
    // Everyone still in sees it.
    let (out, err) = probe(
        &world,
        "alice-laptop",
        &v2,
        &["sync", "then", "get", "tasks", "after-removal-7"],
        &[],
    );
    assert_eq!(
        out[1], "get after-removal-7 title=Payroll moves to Friday by=carol",
        "{out:?} {err}"
    );
}

/// The rest needs a hub that can be switched off or changed: offline edits
/// by two people merge per field, a device with a wrong clock does not win,
/// a tampered ciphertext is refused, the store's own refusals mid-run
/// (removed, outside the audience, withdrawn), and the limits.
#[test]
fn org_team_data_offline_wrong_clock_tampering_refusals_and_limits() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "carol", "dan"]) else {
        eprintln!("skipping org_team_data_offline_...: no local port");
        return;
    };
    if world.real_hub().is_some() {
        eprintln!("skipping org_team_data_offline_...: a real hub cannot be switched off");
        return;
    }
    let root = world.acme();
    let (bundle, digest) = released_probe(&world, &root, "1.0.0");
    let set = |f: &dyn Fn(&mut FakeOrgHub)| f(&mut world.hub.lock().unwrap());
    // Each opens it once (a lease, a registered computer), Alice's computer
    // passes the key, and each takes it.
    for who in ["bob", "carol", "dan"] {
        probe(&world, who, &bundle, &["sync"], &[]);
    }
    keys_show(&world, "alice");
    for who in ["bob", "carol", "dan"] {
        let (out, err) = probe(&world, who, &bundle, &["sync"], &[]);
        assert_eq!(
            out[0], "sync online=true changed=false pending=0",
            "{who}: {err}"
        );
    }

    // Offline: Carol and Dan each edit t1, a different field each.
    set(&|h| h.down = true);
    let (out, err) = probe(
        &world,
        "carol",
        &bundle,
        &[
            "put",
            "tasks",
            "t1",
            "title=Order chairs",
            "then",
            "sync",
            "then",
            "get",
            "tasks",
            "t1",
        ],
        &[],
    );
    assert_eq!(
        out,
        vec![
            "put t1 ok",
            "sync online=false changed=false pending=1",
            "get t1 title=Order chairs by=carol"
        ],
        "{err}"
    );
    assert!(err.contains("offline"), "it opened on its lease: {err}");
    let (out, err) = probe(
        &world,
        "dan",
        &bundle,
        &["put", "tasks", "t1", "n#=4", "then", "sync"],
        &[],
    );
    assert_eq!(
        out,
        vec!["put t1 ok", "sync online=false changed=false pending=1"],
        "{err}"
    );
    set(&|h| h.down = false);
    let (out, err) = probe(&world, "carol", &bundle, &["sync"], &[]);
    assert!(out[0].ends_with("pending=0"), "{out:?} {err}");
    let (out, err) = probe(
        &world,
        "dan",
        &bundle,
        &["sync", "then", "get", "tasks", "t1"],
        &[],
    );
    assert_eq!(
        out[1], "get t1 n=4;title=Order chairs by=dan@acme.example",
        "{out:?} {err}"
    );
    let (out, err) = probe(
        &world,
        "carol",
        &bundle,
        &["sync", "then", "get", "tasks", "t1"],
        &[],
    );
    assert_eq!(
        out[1], "get t1 n=4;title=Order chairs by=dan@acme.example",
        "{out:?} {err}"
    );

    // A wrong clock does not win. Bob's clock jumps a day ahead and he
    // renames t1 offline; later (by the real clock) Carol renames it
    // online. When Bob reconnects, Carol's later edit stays.
    let day = "86400000";
    set(&|h| h.down = true);
    let (out, err) = probe(
        &world,
        "bob",
        &bundle,
        &["put", "tasks", "t1", "title=Bob's title"],
        &[("KRATE_TEST_ORG_CLOCK_SKEW_MS", day)],
    );
    assert_eq!(out, vec!["put t1 ok"], "{err}");
    std::thread::sleep(Duration::from_millis(1500));
    set(&|h| h.down = false);
    let (out, err) = probe(
        &world,
        "carol",
        &bundle,
        &["put", "tasks", "t1", "title=Carol's title", "then", "sync"],
        &[],
    );
    assert!(out[1].ends_with("pending=0"), "{out:?} {err}");
    let (out, err) = probe(
        &world,
        "bob",
        &bundle,
        &["sync", "then", "get", "tasks", "t1"],
        &[("KRATE_TEST_ORG_CLOCK_SKEW_MS", day)],
    );
    assert_eq!(
        out[1], "get t1 n=4;title=Carol's title by=carol",
        "the later edit won on Bob's computer: {out:?} {err}"
    );
    let (out, _) = probe(
        &world,
        "carol",
        &bundle,
        &["sync", "then", "get", "tasks", "t1"],
        &[],
    );
    assert_eq!(
        out[1], "get t1 n=4;title=Carol's title by=carol",
        "and on Carol's"
    );
    let (out, _) = probe(
        &world,
        "bob",
        &bundle,
        &["put", "tasks", "t1", "title=Bob again", "then", "sync"],
        &[("KRATE_TEST_ORG_CLOCK_SKEW_MS", day)],
    );
    assert!(out[1].ends_with("pending=0"), "{out:?}");
    let (out, _) = probe(
        &world,
        "carol",
        &bundle,
        &["sync", "then", "get", "tasks", "t1"],
        &[],
    );
    assert_eq!(out[1], "get t1 n=4;title=Bob again by=bob");

    let (out, _) = probe(
        &world,
        "dan",
        &bundle,
        &["sync", "then", "get", "tasks", "t1"],
        &[],
    );
    assert_eq!(out[1], "get t1 n=4;title=Bob again by=bob");
    // A tampered ciphertext is refused. The hub changes one byte of the
    // stored title and serves it as new: Dan's computer does not apply it.
    {
        let mut hub = world.hub.lock().unwrap();
        let acme = hub.orgs.get_mut("acme").unwrap();
        acme.data_seq += 1;
        let seq = acme.data_seq;
        let row = acme
            .data
            .values_mut()
            .filter(|r| r.kind == "v")
            .max_by(|a, b| a.s.cmp(&b.s))
            .unwrap();
        let mut bytes = krate_orgkeys::b64_decode(&row.ct).unwrap();
        bytes[30] ^= 1;
        row.ct = krate_orgkeys::b64_encode(&bytes);
        row.s = format!("0002{}", &row.s[4..]);
        row.seq = seq;
    }
    let (out, err) = probe(
        &world,
        "dan",
        &bundle,
        &["sync", "then", "get", "tasks", "t1"],
        &[],
    );
    assert_eq!(
        out[1], "get t1 n=4;title=Bob again by=bob",
        "the tampered row was not applied: {out:?} {err}"
    );
    let mirror: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&org_data_files(&world, "dan")[0]).unwrap())
            .unwrap();
    assert!(
        mirror["rejected"].as_u64().unwrap_or(0) >= 1,
        "and it was counted: {}",
        mirror["rejected"]
    );

    // Limits, on the device: names, the record, the quota the hub states.
    let long = "i".repeat(129);
    let (out, _) = probe(
        &world,
        "carol",
        &bundle,
        &[
            "put", "tasks", &long, "a=b", "then", "fill", "tasks", "big", "body", "70000", "then",
            "fill", "tasks", "ok", "body", "60000",
        ],
        &[],
    );
    assert!(
        out[0].starts_with(&format!(
            "put {long} error=invalid a record id is at most 128 bytes"
        )),
        "{out:?}"
    );
    assert_eq!(out[1], "fill big error=too-large");
    assert_eq!(out[2], "fill ok ok");
    set(&|h| h.data_quota = 70_000);
    let (out, err) = probe(
        &world,
        "carol",
        &bundle,
        &["sync", "then", "fill", "tasks", "more", "body", "20000"],
        &[],
    );
    assert_eq!(out[1], "fill more error=over-quota", "{out:?} {err}");

    // The store's own refusals, while the app is open offline on its lease
    // and its data still reaches the hub.
    set(&|h| h.checkin_down = true);
    world.says(
        "alice",
        &["org", "group", "create", "acme", "finance"],
        true,
    );
    world.says(
        "alice",
        &[
            "org", "release", "audience", "acme", &digest, "--groups", "finance",
        ],
        true,
    );
    let (out, err) = probe(
        &world,
        "dan",
        &bundle,
        &["sync", "then", "get", "tasks", "t1"],
        &[],
    );
    assert_eq!(
        out,
        vec![
            "sync error=refused reason=not-in-audience",
            "get t1 error=refused reason=not-in-audience"
        ],
        "{err}"
    );
    world.says(
        "alice",
        &["org", "release", "audience", "acme", &digest, "--everyone"],
        true,
    );
    world.says(
        "alice",
        &["org", "remove", "acme", "dan@acme.example"],
        true,
    );
    assert!(!org_data_files(&world, "dan").is_empty());
    let (out, err) = probe(
        &world,
        "dan",
        &bundle,
        &["sync", "then", "get", "tasks", "t1"],
        &[],
    );
    assert_eq!(
        out,
        vec![
            "sync error=refused reason=removed",
            "get t1 error=refused reason=removed"
        ],
        "{err}"
    );
    assert!(
        org_data_files(&world, "dan").is_empty(),
        "the data left Dan's computer"
    );
    world.says(
        "bob",
        &[
            "org",
            "release",
            "withdraw",
            "acme",
            &digest,
            "--reason",
            "wrong totals",
        ],
        true,
    );
    let (out, err) = probe(
        &world,
        "carol",
        &bundle,
        &["sync", "then", "put", "tasks", "t9", "a=b"],
        &[],
    );
    assert_eq!(
        out,
        vec![
            "sync error=refused reason=release-withdrawn",
            "put t9 error=refused reason=release-withdrawn"
        ],
        "{err}"
    );
    let hub = world.hub.lock().unwrap();
    let denied: Vec<&str> = hub.orgs["acme"]
        .audit
        .iter()
        .filter(|e| e["action"] == "data.sync" && e["result"] == "denied")
        .filter_map(|e| e["reason"].as_str())
        .collect();
    for reason in ["not-in-audience", "release-withdrawn"] {
        assert!(denied.contains(&reason), "{reason}: {denied:?}");
    }
}

// ---------------------------------------------------------------------------
// IC-953: the company's apps on every device.

impl OrgWorld {
    /// `krate org apps --json` as `who`, parsed.
    fn company_apps(&self, who: &str) -> serde_json::Value {
        let out = self.krate_as(who, &["org", "apps", "--json"]);
        assert!(out.status.success(), "{}", stderr_of(&out));
        serde_json::from_slice(&out.stdout).expect("org apps --json is JSON")
    }

    /// The app ids and versions on `who`'s list for acme.
    fn company_versions(&self, who: &str) -> Vec<(String, String)> {
        let list = self.company_apps(who);
        list["orgs"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|o| o["slug"] == "acme")
            .flat_map(|o| o["apps"].as_array().cloned().unwrap_or_default())
            .map(|a| {
                (
                    a["app_id"].as_str().unwrap().to_string(),
                    a["version"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    }

    /// Where `who`'s Krate keeps a release ready for offline use.
    fn kept_ready(&self, who: &str, digest: &str) -> PathBuf {
        self.homes[who]
            .path()
            .join("cache/org-apps")
            .join(format!("{digest}.krate"))
    }

    /// `krate org apps acme --open <app>` as `who`, headless.
    fn open_company_app(&self, who: &str, app: &str) -> std::process::Output {
        self.krate_as(
            who,
            &[
                "org",
                "apps",
                "acme",
                "--open",
                app,
                "--",
                "--headless",
                "--auto-grant",
            ],
        )
    }
}

/// IC-953 proof: a member sees exactly their audience's apps, each kept
/// ready offline; a rollback lists and opens the older release; a withdrawn
/// release is gone from the list and from the computer; a release kept
/// ready opens offline within its lease, showing the terms first, and is
/// refused once the lease ran out.
#[test]
fn company_apps_are_listed_kept_ready_and_opened_through_the_door() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "carol", "dan"]) else {
        eprintln!("skipping company_apps_are_listed_kept_ready_and_opened_through_the_door: no local port");
        return;
    };
    let root = world.acme();
    let (_, d1, _) = world.released_app(&root, "1.0.0");
    let (_, d2, _) = world.released_app(&root, "1.0.1");
    let (_, d3, _) = world.released_app(&root, "1.0.2");
    let app = "dev.acme.demo";

    // Dan sees the release that opens now, kept ready for offline use.
    let list = world.company_apps("dan");
    let acme = &list["orgs"][0];
    assert_eq!(acme["slug"], "acme");
    assert_eq!(acme["from"], "hub");
    let apps = acme["apps"].as_array().unwrap();
    assert_eq!(apps.len(), 1, "{list}");
    assert_eq!(
        (
            apps[0]["app_id"].as_str(),
            apps[0]["version"].as_str(),
            apps[0]["digest"].as_str()
        ),
        (Some(app), Some("1.0.2"), Some(d3.as_str()))
    );
    assert!(apps[0]["size"].as_u64().unwrap() > 0);
    assert!(apps[0]["ready_offline_until"].as_u64().is_some(), "{list}");
    let kept = std::fs::read(world.kept_ready("dan", &d3)).expect("the release is kept ready");
    assert_eq!(
        format!("{:x}", Sha256::digest(&kept)),
        d3,
        "held to its digest"
    );
    let record = world
        .lease_record("dan", &d3)
        .expect("its offline access is ready");
    assert!(
        record["answer"].is_object() && record["noticed_class"].is_null(),
        "{record}"
    );
    // In words, with the line that opens it.
    let said = world.says("dan", &["org", "apps"], true);
    assert!(said.contains("Acme (acme): 1 app"), "{said}");
    assert!(said.contains("Acme Demo 1.0.2  dev.acme.demo"), "{said}");
    assert!(said.contains("works offline until"), "{said}");
    assert!(
        said.contains("Open one: krate org apps acme --open dev.acme.demo"),
        "{said}"
    );
    // Listed again, nothing new is issued: the lease held is kept.
    let issued = world.hub.lock().unwrap().leases_issued;
    world.company_apps("dan");
    if world.real_hub().is_none() {
        assert_eq!(world.hub.lock().unwrap().leases_issued, issued);
    }

    // An audience: 1.0.2 only for finance, which Dan is in and Carol is not.
    world.says(
        "alice",
        &["org", "group", "create", "acme", "finance"],
        true,
    );
    world.says(
        "alice",
        &["org", "group", "add", "acme", "finance", "dan@acme.example"],
        true,
    );
    world.says(
        "alice",
        &[
            "org", "release", "audience", "acme", &d3, "--groups", "finance",
        ],
        true,
    );
    assert_eq!(
        world.company_versions("dan"),
        [(app.to_string(), "1.0.2".to_string())]
    );
    assert!(
        world.company_versions("carol").is_empty(),
        "the release that opens now is not Carol's, so the app is not on her list"
    );
    world.says(
        "alice",
        &["org", "release", "audience", "acme", &d3, "--everyone"],
        true,
    );
    assert_eq!(
        world.company_versions("carol"),
        [(app.to_string(), "1.0.2".to_string())]
    );

    // Rollback: 1.0.1 is made current. Opening the app opens it -- through
    // `krate run`, which checks those bytes in with the hub.
    world.says(
        "alice",
        &["org", "release", "current", "acme", app, &d2],
        true,
    );
    assert!(world.lease_record("dan", &d2).is_none());
    let opened = world.open_company_app("dan", app);
    assert!(opened.status.success(), "{}", stderr_of(&opened));
    let record = world
        .lease_record("dan", &d2)
        .expect("1.0.1 was what opened");
    assert!(record["checked_at"].is_u64(), "{record}");
    assert_eq!(
        world.company_versions("dan"),
        [(app.to_string(), "1.0.1".to_string())]
    );
    assert!(
        !world.kept_ready("dan", &d3).exists(),
        "a release no longer current is not kept"
    );

    // Withdrawn: 1.0.1, then 1.0.2. Neither is listed, and the open is of
    // the newest approval left, 1.0.0.
    for digest in [&d2, &d3] {
        world.says(
            "bob",
            &[
                "org",
                "release",
                "withdraw",
                "acme",
                digest,
                "--reason",
                "wrong rates",
            ],
            true,
        );
    }
    let list = world.company_apps("dan");
    let digests: Vec<&str> = list["orgs"][0]["apps"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a["digest"].as_str())
        .collect();
    assert_eq!(
        digests,
        [d1.as_str()],
        "withdrawn releases are gone: {list}"
    );
    assert!(!world.kept_ready("dan", &d2).exists());
    assert!(world.kept_ready("dan", &d1).is_file());

    if world.real_hub().is_some() {
        // The rest stops the hub, which a real one cannot be asked to do.
        eprintln!("skipping the rest of company_apps_are_listed_...: it stops the hub, which a real hub cannot be asked to do");
        return;
    }
    // Offline: the hub is gone. The list is the one kept, and the app kept
    // ready opens on its offline access, the terms shown first.
    world.stop.store(true, std::sync::atomic::Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(100));
    let said = world.says("dan", &["org", "apps"], true);
    assert!(
        said.contains("No connection: this is the list from"),
        "{said}"
    );
    assert!(said.contains("Acme Demo 1.0.0"), "{said}");
    let offline = world.open_company_app("dan", app);
    let said = stderr_of(&offline);
    assert!(offline.status.success(), "{said}");
    assert!(
        said.contains("This is Acme's private app. Works offline until")
            && said.contains("offline: opening Acme's app on its offline access"),
        "the terms, then the offline open: {said}"
    );
    let again = world.open_company_app("dan", app);
    assert!(again.status.success());
    assert!(
        !stderr_of(&again).contains("Works offline until"),
        "the terms are shown once"
    );

    // Its lease runs out: refused, with the date.
    let lease_file = world.homes["dan"]
        .path()
        .join(".krate/leases")
        .join(format!("{d1}.json"));
    let mut record: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&lease_file).unwrap()).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let mut body = record["answer"]["lease"].clone();
    body["issued_at"] = serde_json::json!(now - 8 * 86_400);
    body["expires_at"] = serde_json::json!(now - 86_400);
    let pkcs8 = world.hub.lock().unwrap().lease_pkcs8.clone();
    record["answer"] = krate_lease::sign(&body, &pkcs8).unwrap();
    std::fs::write(&lease_file, record.to_string()).unwrap();
    let expired = world.open_company_app("dan", app);
    assert!(!expired.status.success());
    assert!(
        stderr_of(&expired).contains("its offline access ran out on"),
        "{}",
        stderr_of(&expired)
    );
    // An app never listed here does not open offline, in words.
    let never = world.open_company_app("dan", "dev.acme.other");
    assert!(!never.status.success());
    assert!(
        stderr_of(&never).contains("was never listed on this computer"),
        "{}",
        stderr_of(&never)
    );
}

/// IC-953 MDM: `[org] slug` in the machine policy puts that organisation
/// first, and signed out, the company sign-in is what is offered.
#[test]
fn the_policy_organisation_is_listed_first_and_offers_its_company_sign_in() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "carol", "dan"]) else {
        eprintln!("skipping the_policy_organisation_is_listed_first_and_offers_its_company_sign_in: no local port");
        return;
    };
    let root = world.acme();
    world.released_app(&root, "1.0.0");
    // A second organisation that sorts before acme, with Dan in it.
    world.says("alice", &["org", "create", "aaa-labs"], true);
    let said = world.says(
        "alice",
        &[
            "org",
            "invite",
            "aaa-labs",
            "--email",
            "dan@acme.example",
            "--role",
            "recipient",
        ],
        true,
    );
    let code = said
        .lines()
        .find_map(|l| l.trim().strip_prefix("krate org join aaa-labs "))
        .expect("join line")
        .trim()
        .to_string();
    world.says("dan", &["org", "join", "aaa-labs", &code], true);
    let policy = world.dir.path().join("policy.toml");
    std::fs::write(&policy, "version = 1\n\n[org]\nslug = \"acme\"\n").unwrap();
    let as_dan = |who: &str, args: &[&str], policy_path: &std::path::Path| {
        org_command(&world, who, args)
            .env("KRATE_TEST_POLICY", policy_path)
            .output()
            .expect("krate")
    };

    // Without the policy: the hub's order. With it: acme first.
    let none = world.dir.path().join("none.toml");
    std::fs::write(&none, "version = 1\n").unwrap();
    let plain: serde_json::Value =
        serde_json::from_slice(&as_dan("dan", &["org", "apps", "--json"], &none).stdout).unwrap();
    let order = |v: &serde_json::Value| -> Vec<String> {
        v["orgs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["slug"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(order(&plain), ["aaa-labs", "acme"], "{plain}");
    let first: serde_json::Value =
        serde_json::from_slice(&as_dan("dan", &["org", "apps", "--json"], &policy).stdout).unwrap();
    assert_eq!(order(&first), ["acme", "aaa-labs"], "{first}");
    assert_eq!(first["policy_org"], "acme");
    assert_eq!(first["orgs"][0]["first"], true);
    let said =
        String::from_utf8_lossy(&as_dan("dan", &["org", "apps"], &policy).stdout).to_string();
    assert!(
        said.starts_with("Acme (acme, this computer's organisation): 1 app"),
        "{said}"
    );
    // `krate policy` says it, in words and as JSON.
    let words = String::from_utf8_lossy(&as_dan("dan", &["policy"], &policy).stdout).to_string();
    assert!(words.contains("Organisation: acme."), "{words}");
    let json: serde_json::Value =
        serde_json::from_slice(&as_dan("dan", &["policy", "--json"], &policy).stdout).unwrap();
    assert_eq!(json["policy"]["org"]["slug"], "acme");

    // Signed out on a new computer: the company's own sign-in, and nothing
    // asked of the hub.
    let before = world.hub.lock().unwrap().seen.len();
    let out = as_dan("nobody", &["org", "apps"], &policy);
    assert!(!out.status.success());
    assert!(
        stderr_of(&out).contains(
            "sign in to see acme's apps: open https://krate.tech/login/?org=acme to use your \
             company sign-in"
        ),
        "{}",
        stderr_of(&out)
    );
    let quiet: serde_json::Value =
        serde_json::from_slice(&as_dan("nobody", &["org", "apps", "--json"], &policy).stdout)
            .unwrap();
    assert_eq!(quiet["signed_in"], false);
    assert_eq!(quiet["sign_in"], "https://krate.tech/login/?org=acme");
    // With the organisation's name in the policy, a person reads the name.
    let named = world.dir.path().join("named.toml");
    std::fs::write(
        &named,
        "version = 1\nname = \"Acme Labs\"\n\n[org]\nslug = \"acme\"\n",
    )
    .unwrap();
    let out = as_dan("nobody", &["org", "apps"], &named);
    assert!(
        stderr_of(&out).contains("sign in to see Acme Labs's apps"),
        "{}",
        stderr_of(&out)
    );
    let quiet: serde_json::Value =
        serde_json::from_slice(&as_dan("nobody", &["org", "apps", "--json"], &named).stdout)
            .unwrap();
    assert_eq!(quiet["policy_name"], "Acme Labs");
    if world.real_hub().is_none() {
        assert_eq!(
            world.hub.lock().unwrap().seen.len(),
            before,
            "nothing was sent"
        );
    }
}

// ---------------------------------------------------------------------------
// IC-954: provenance in the bundle.

/// `krate create` writes a build record into the app it makes: the tool
/// (Studio says so through KRATE_BUILT_BY), the provider (a person's own
/// command here, so no AI is contacted), when, the request's digest, and the
/// request itself only with --keep-prompt. `krate inspect` shows it, the
/// signature covers it, and what runs is unchanged by it.
#[test]
fn create_writes_a_build_record_that_inspect_shows_and_signing_covers() {
    if !has_cargo_component() {
        eprintln!("skipping: cargo-component not installed");
        return;
    }
    let _build_lock = cargo_build_guard();
    let work = tempfile::tempdir().expect("temp dir");
    let make = |out: &std::path::Path, keep: bool| {
        let mut command = krate();
        command
            .args([
                "create",
                "a small dashboard",
                "--author-cmd",
                "true",
                "--output",
            ])
            .arg(out)
            .env("KRATE_BUILT_BY", "studio");
        if keep {
            command.arg("--keep-prompt");
        }
        let output = command.output().expect("run create");
        // A no-op author leaves the starter: exit 6, and the file is written.
        assert_eq!(
            output.status.code(),
            Some(6),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(out.is_file());
    };
    let inspect = |path: &std::path::Path| -> serde_json::Value {
        let out = krate()
            .args(["inspect", path.to_str().unwrap(), "--format", "json"])
            .output()
            .expect("inspect");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).expect("json")
    };
    let kept = work.path().join("kept.krate");
    make(&kept, true);
    let report = inspect(&kept);
    let built = &report["provenance"];
    assert_eq!(built["v"], 1, "{report}");
    assert_eq!(built["tool"], "krate studio");
    assert_eq!(built["provider"], "author-cmd");
    assert_eq!(built["prompt"], "a small dashboard");
    assert_eq!(
        built["prompt_sha256"],
        format!("{:x}", Sha256::digest(b"a small dashboard"))
    );
    assert_eq!(built["revisions"], 0);
    assert!(built["finished_at"].as_u64() >= built["started_at"].as_u64());
    assert!(built["krate_version"]
        .as_str()
        .is_some_and(|v| !v.is_empty()));
    assert!(
        report["extensions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["owner"] == "krate" && e["name"] == "provenance"),
        "declared, as every extension is: {report}"
    );
    let text = krate()
        .args(["inspect", kept.to_str().unwrap()])
        .output()
        .expect("inspect");
    let said = String::from_utf8_lossy(&text.stdout);
    assert!(
        said.contains("built           by krate studio with author-cmd")
            && said.contains("  asked for     a small dashboard"),
        "{said}"
    );

    // Without --keep-prompt only the digest is kept, and what runs is the
    // same app either way.
    let plain = work.path().join("plain.krate");
    make(&plain, false);
    let report = inspect(&plain);
    assert!(report["provenance"]["prompt"].is_null(), "{report}");
    assert!(report["provenance"]["prompt_sha256"].is_string());
    assert_eq!(
        inspect(&kept)["identity"]["execution"],
        report["identity"]["execution"]
    );

    // Signed, the signature covers the record: it holds, and once the
    // record is changed it no longer does.
    let key = work.path().join("key.pkcs8");
    let signed = krate()
        .args([
            "sign",
            kept.to_str().unwrap(),
            "--key",
            key.to_str().unwrap(),
            "--generate-key",
            "--namespace",
            "acme/dashboard",
        ])
        .output()
        .expect("sign");
    assert!(
        signed.status.success(),
        "{}",
        String::from_utf8_lossy(&signed.stderr)
    );
    let holds = |path: &std::path::Path| {
        krate_bundle::open(path)
            .unwrap()
            .signature_verdict()
            .unwrap()
            .is_some_and(|v| v.is_genuinely_signed())
    };
    assert!(holds(&kept), "signed over its build record");
    let mut record = krate_bundle::read_build_provenance(&kept).unwrap().unwrap();
    record.provider = Some("someone-else".into());
    krate_bundle::add_build_provenance(&kept, &record).unwrap();
    assert!(!holds(&kept), "a changed build record breaks the signature");
}

// ---------------------------------------------------------------------------
// The starter apps (IC-958): Team Notes, Expenses and Stock, the three an
// organisation releases in its first minute. Each is released to acme from
// the very bundle the site serves (docs/landing/orgs/starters/), then two
// people on two computers use it through its own window, the way a person
// does: `krate run --task`, pressing what the app tells the screen reader is
// on screen and typing into it. KRATE_STARTER_SHOTS=<dir> keeps a picture of
// each screen the steps name.
// ---------------------------------------------------------------------------

/// A starter as the site serves it, signed for acme by Carol, published and
/// approved by Bob: the bundle and its digest.
fn released_starter(world: &OrgWorld, root: &std::path::Path, file: &str) -> (PathBuf, String) {
    let shipped = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/landing/orgs/starters")
        .join(file);
    let bundle = world.dir.path().join(file);
    std::fs::copy(&shipped, &bundle).unwrap_or_else(|e| panic!("{}: {e}", shipped.display()));
    world.sign_delegated(root, "carol", &bundle);
    let digest = OrgWorld::digest_in(&world.says(
        "carol",
        &["publish", bundle.to_str().unwrap(), "--org", "acme"],
        true,
    ));
    world.says("bob", &["org", "approve", "acme", &digest], true);
    (bundle, digest)
}

/// `who` opens their own copy of `bundle` and does `steps`, each a step of
/// krate-check.toml as (kind, value). A `shoot` step names a picture, kept
/// only under KRATE_STARTER_SHOTS; `look` is "dark", "phone" or both.
/// Fails with what was on screen when a step cannot be done.
fn use_starter(
    world: &OrgWorld,
    who: &str,
    bundle: &std::path::Path,
    look: Option<&str>,
    steps: &[(&str, &str)],
) {
    use_starter_while(world, who, bundle, look, steps, None);
}

/// [`use_starter`], with `meanwhile` done three seconds into the run, while
/// the window is open: what someone else does as this person works.
fn use_starter_while(
    world: &OrgWorld,
    who: &str,
    bundle: &std::path::Path,
    look: Option<&str>,
    steps: &[(&str, &str)],
    meanwhile: Option<&dyn Fn()>,
) {
    static RUNS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = RUNS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let name = bundle.file_name().unwrap().to_string_lossy().to_string();
    let copy = world.homes[who].path().join(&name);
    std::fs::copy(bundle, &copy).expect("copy");
    let shots = std::env::var_os("KRATE_STARTER_SHOTS").map(PathBuf::from);
    let mut task = format!("name = \"{who} uses {name}\"\n");
    for (kind, value) in steps {
        match *kind {
            "shoot" => {
                let Some(dir) = &shots else { continue };
                std::fs::create_dir_all(dir).expect("shots dir");
                task.push_str(&format!(
                    "[[step]]\nshoot = {:?}\n",
                    dir.join(value).to_string_lossy()
                ));
            }
            "wait_ms" => task.push_str(&format!("[[step]]\nwait_ms = {value}\n")),
            _ => task.push_str(&format!("[[step]]\n{kind} = {value:?}\n")),
        }
    }
    let file = world.dir.path().join(format!("task-{n}.toml"));
    std::fs::write(&file, &task).expect("task");
    let first = world.dir.path().join(format!("first-{n}.png"));
    let mut command = org_command(
        world,
        who,
        &[
            "run",
            copy.to_str().unwrap(),
            "--auto-grant",
            "--task",
            file.to_str().unwrap(),
            "--shoot",
            first.to_str().unwrap(),
        ],
    );
    // "dark" is the person's look; "phone" a phone held upright.
    if let Some(look) = look {
        if look.contains("dark") {
            command.env("KRATE_APPEARANCE", "dark");
        }
        if look.contains("phone") {
            command.env("KRATE_SCREEN", "390x844");
        }
    }
    let child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run");
    if let Some(meanwhile) = meanwhile {
        // Time to open and check in; the refusal can land at any point
        // after that, so the steps must not depend on when.
        thread::sleep(Duration::from_secs(3));
        meanwhile();
    }
    let out = child.wait_with_output().expect("run");
    assert!(
        out.status.success(),
        "{who} using {name}: {}\n{task}",
        stderr_of(&out)
    );
}

/// Team Notes, two people: Carol writes the first note while her computer
/// is still being let in; once Alice's computer has passed the key, it
/// reaches the team; Dan's new computer waits, then sees it with who
/// changed it, pins it and adds his own; Carol sees Dan's, finds hers by
/// search, deletes it and brings it back. Narrowed to finance, the app
/// tells Dan in words that it is not shared with him.
#[test]
fn starter_team_notes_is_shared_by_two_people_through_its_window() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "carol", "dan"]) else {
        eprintln!("skipping starter_team_notes_...: no local port");
        return;
    };
    let root = world.acme();
    let (notes, digest) = released_starter(&world, &root, "team-notes.krate");
    let title = "Order chairs for the hall";
    new_laptop(&world, "carol");
    use_starter(
        &world,
        "carol",
        &notes,
        None,
        &[
            ("expect_text", "Your team's notes are on their way"),
            // The calm line, by the organisation's name (K-1108).
            ("expect_text", "someone in Acme opens Krate and it"),
            ("shoot", "team-notes-waiting.png"),
            ("click", "Write a note"),
            ("type", title),
            ("key", "Enter"),
            // A paste with no clipboard permission, over a selection
            // (K-1109): typed text, all of it selected, replaced by what
            // the person pasted; then Shift+Home selects the last line and
            // typing replaces it.
            ("type", "a first draft"),
            ("key", "Primary+a"),
            ("paste", "Twelve, stackable.\nBob has the budget."),
            ("expect_text", "Twelve, stackable."),
            ("expect_no_text", "a first draft"),
            ("key", "Shift+Home"),
            ("shoot", "team-notes-selected.png"),
            ("type", "Bob pays."),
            ("expect_text", "Bob pays."),
            ("expect_no_text", "Bob has the budget"),
            ("shoot", "team-notes-pasted.png"),
            ("wait_ms", "900"),
            ("expect_text", title),
            ("expect_text", "Waiting for your team's key"),
        ],
    );
    keys_show(&world, "alice");
    use_starter(
        &world,
        "carol",
        &notes,
        None,
        &[
            ("expect_text", title),
            ("expect_text", "Synced"),
            ("shoot", "team-notes-one-note.png"),
        ],
    );
    new_laptop(&world, "dan");
    use_starter(
        &world,
        "dan",
        &notes,
        None,
        &[("expect_text", "Your team's notes are on their way")],
    );
    // Carol's computer, online again, lets Dan's in.
    use_starter(&world, "carol", &notes, None, &[("expect_text", title)]);
    use_starter(
        &world,
        "dan",
        &notes,
        Some("dark"),
        &[
            ("expect_text", title),
            ("expect_text", "changed by carol"),
            ("click", title),
            ("expect_text", "Changed by carol"),
            ("click", "Pin"),
            ("expect_text", "PINNED"),
            ("click", "New note"),
            ("type", "Wifi for guests"),
            ("key", "Enter"),
            ("type", "Network acme-guest, the password is on the fridge."),
            ("wait_ms", "900"),
            ("expect_text", "Wifi for guests"),
            ("shoot", "team-notes-dark.png"),
        ],
    );
    use_starter(
        &world,
        "carol",
        &notes,
        None,
        &[
            ("expect_text", "Wifi for guests"),
            ("expect_text", "changed by dan"),
            ("expect_text", "PINNED"),
            ("shoot", "team-notes-two-people.png"),
            ("click", "Search notes"),
            ("type", "chairs"),
            ("expect_text", title),
            ("expect_no_text", "Wifi for guests"),
            ("click", title),
            ("click", "Delete"),
            ("expect_text", "Undo"),
            ("expect_text", "Deleted"),
            ("click", "Undo"),
            ("expect_text", "Changed by carol"),
        ],
    );
    // On a phone: one column, the note fills the screen, Back returns.
    use_starter(
        &world,
        "carol",
        &notes,
        Some("phone"),
        &[
            ("expect_text", "Wifi for guests"),
            ("shoot", "team-notes-phone.png"),
            ("click", "Wifi for guests"),
            ("expect_text", "Back"),
            ("shoot", "team-notes-phone-note.png"),
            ("click", "Back"),
            ("expect_text", "New note"),
        ],
    );
    world.says(
        "alice",
        &["org", "group", "create", "acme", "finance"],
        true,
    );
    world.says(
        "alice",
        &["org", "group", "add", "acme", "finance", "carol"],
        true,
    );
    // Narrowed to finance while Dan has it open: the app says so, in words,
    // and offers nothing to edit.
    let narrow = || {
        world.says(
            "alice",
            &[
                "org", "release", "audience", "acme", &digest, "--groups", "finance",
            ],
            true,
        );
    };
    use_starter_while(
        &world,
        "dan",
        &notes,
        None,
        &[
            ("wait_ms", "10000"),
            ("expect_text", "has not shared Team Notes with you"),
            ("expect_no_text", "New note"),
            ("shoot", "team-notes-not-shared.png"),
        ],
        Some(&narrow),
    );
    // Opened from now on, it is refused before the window, in words; the
    // app's own "not shared with you" screen is for a narrowing that lands
    // while it is open.
    let copy = world.homes["dan"].path().join("team-notes.krate");
    let refused = org_command(
        &world,
        "dan",
        &["run", copy.to_str().unwrap(), "--auto-grant", "--headless"],
    )
    .output()
    .expect("run");
    assert!(!refused.status.success());
    assert!(
        stderr_of(&refused).contains("this app is only for the finance group in Acme"),
        "{}",
        stderr_of(&refused)
    );
}

/// Expenses, two people: a form that says what is missing; Carol's expense
/// reaches Dan with who is owed; Dan logs his own and marks Carol's paid
/// back; Carol sees both, corrects Dan's amount, and a delete comes back
/// with Undo.
#[test]
fn starter_expenses_is_shared_by_two_people_through_its_window() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "carol", "dan"]) else {
        eprintln!("skipping starter_expenses_...: no local port");
        return;
    };
    let root = world.acme();
    let (expenses, _) = released_starter(&world, &root, "expenses.krate");
    new_laptop(&world, "carol");
    use_starter(
        &world,
        "carol",
        &expenses,
        None,
        &[
            ("expect_text", "Your team's expenses are on their way"),
            ("click", "Log an expense"),
            ("click", "Save expense"),
            ("expect_text", "Enter how much it was"),
            ("shoot", "expenses-form-error.png"),
            ("type", "42.50"),
            ("click", "EUR"),
            ("click", "What was it for"),
            ("type", "Train to Leeds"),
            ("click", "Receipt note"),
            ("type", "Client visit"),
            ("click", "Save expense"),
            ("expect_text", "Train to Leeds, \u{20ac}42.50"),
        ],
    );
    keys_show(&world, "alice");
    use_starter(
        &world,
        "carol",
        &expenses,
        None,
        &[
            ("expect_text", "Synced"),
            ("expect_text", "You are owed \u{20ac}42.50"),
        ],
    );
    new_laptop(&world, "dan");
    use_starter(
        &world,
        "dan",
        &expenses,
        None,
        &[("expect_text", "Your team's expenses are on their way")],
    );
    use_starter(
        &world,
        "carol",
        &expenses,
        None,
        &[("expect_text", "Train to Leeds")],
    );
    use_starter(
        &world,
        "dan",
        &expenses,
        Some("dark"),
        &[
            ("expect_text", "carol is owed \u{20ac}42.50"),
            ("click", "Log an expense"),
            ("type", "18"),
            ("click", "EUR"),
            ("click", "What was it for"),
            ("type", "Printer ink"),
            ("key", "Enter"),
            ("expect_text", "You are owed \u{20ac}18.00"),
            ("click", "Mark paid back: Train to Leeds"),
            ("expect_text", "Paid back: Train to Leeds"),
            ("expect_no_text", "carol is owed"),
            ("shoot", "expenses-dark.png"),
        ],
    );
    use_starter(
        &world,
        "carol",
        &expenses,
        None,
        &[
            ("expect_text", "Paid back: Train to Leeds"),
            // Dan marked it, and it is still Carol's.
            ("expect_text", "Paid by you \u{b7} Client visit"),
            ("expect_text", "dan is owed \u{20ac}18.00"),
            ("click", "Printer ink"),
            ("expect_text", "Change this expense"),
            ("click", "Amount"),
            ("type", ".40"),
            ("click", "Save changes"),
            ("expect_text", "Printer ink, \u{20ac}18.40"),
            ("shoot", "expenses-two-people.png"),
            ("click", "Printer ink"),
            ("click", "Delete"),
            ("expect_text", "Undo"),
            ("expect_no_text", "Printer ink,"),
            ("click", "Undo"),
            ("expect_text", "Printer ink, \u{20ac}18.40"),
        ],
    );
    use_starter(
        &world,
        "dan",
        &expenses,
        Some("phone"),
        &[
            ("expect_text", "Printer ink, \u{20ac}18.40"),
            ("expect_text", "Paid back: Train to Leeds"),
            ("expect_text", "Paid by carol \u{b7} Client visit"),
            ("shoot", "expenses-phone.png"),
            ("click", "Log"),
            ("expect_text", "Log an expense"),
            ("shoot", "expenses-phone-form.png"),
            ("click", "Cancel"),
        ],
    );
}

/// Stock List, two people: Carol adds an item from a suggestion; Dan counts
/// two out and it is running low; Carol counts one in on top of Dan's, then
/// sets the count after a stocktake; Dan sees the same number.
#[test]
fn starter_stock_list_is_shared_by_two_people_through_its_window() {
    let Some(world) = OrgWorld::new(&["alice", "bob", "carol", "dan"]) else {
        eprintln!("skipping starter_stock_list_...: no local port");
        return;
    };
    let root = world.acme();
    let (stock, _) = released_starter(&world, &root, "stock-list.krate");
    new_laptop(&world, "carol");
    use_starter(
        &world,
        "carol",
        &stock,
        None,
        &[
            ("expect_text", "Your team's stock list is on its way"),
            ("click", "Printer paper"),
            ("expect_text", "Add an item"),
            ("type", "1"),
            ("shoot", "stock-list-form.png"),
            ("click", "Add item"),
            ("expect_text", "Printer paper, 3 left"),
        ],
    );
    keys_show(&world, "alice");
    use_starter(
        &world,
        "carol",
        &stock,
        None,
        &[
            ("expect_text", "Synced"),
            ("expect_text", "Printer paper, 3 left"),
        ],
    );
    new_laptop(&world, "dan");
    use_starter(
        &world,
        "dan",
        &stock,
        None,
        &[("expect_text", "Your team's stock list is on its way")],
    );
    use_starter(
        &world,
        "carol",
        &stock,
        None,
        &[("expect_text", "Printer paper")],
    );
    use_starter(
        &world,
        "dan",
        &stock,
        Some("dark"),
        &[
            ("expect_text", "Printer paper, 3 left"),
            ("click", "Add an item"),
            ("type", "Coffee beans"),
            ("key", "Tab"),
            ("type", "6"),
            ("key", "Tab"),
            ("type", "2"),
            ("key", "Enter"),
            ("expect_text", "Coffee beans, 6 left"),
            ("click", "One fewer Printer paper"),
            ("click", "One fewer Printer paper"),
            ("expect_text", "Printer paper, 1 left, Running low"),
            ("expect_text", "Running low (1)"),
            ("shoot", "stock-list-dark.png"),
        ],
    );
    use_starter(
        &world,
        "carol",
        &stock,
        None,
        &[
            ("expect_text", "Printer paper, 1 left"),
            ("click", "Running low (1)"),
            ("expect_no_text", "Coffee beans"),
            ("shoot", "stock-list-running-low.png"),
            ("click", "One more Printer paper"),
            ("expect_text", "Nothing is running low"),
            ("click", "Everything (2)"),
            ("expect_text", "Printer paper, 2 left"),
            ("click", "Printer paper, 2 left"),
            ("expect_text", "Change this item"),
            ("click", "How many there are"),
            ("key", "Backspace"),
            ("type", "12"),
            ("click", "Save changes"),
            ("expect_text", "Printer paper, 12 left"),
            ("shoot", "stock-list-two-people.png"),
        ],
    );
    use_starter(
        &world,
        "dan",
        &stock,
        Some("phone"),
        &[
            ("expect_text", "Printer paper, 12 left"),
            ("expect_text", "Coffee beans, 6 left"),
            ("shoot", "stock-list-phone.png"),
            ("click", "One more Coffee beans"),
            ("expect_text", "Coffee beans, 7 left"),
            // Left alone past several background syncs and one of its own.
            ("wait_ms", "25000"),
            ("expect_text", "Coffee beans, 7 left"),
            ("expect_text", "Synced"),
        ],
    );
}
