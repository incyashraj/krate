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
    // Go, which the pipeline cannot build. This test is about the JSON being
    // machine readable at all; the verdict just has to be the honest one.
    assert_eq!(report["verdict"], "unsupported");
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
    // guest is launched. One over the limit, each record a few bytes.
    let crowd: Vec<(String, Vec<u8>)> = (0..=krate_bundle::MAX_ENTRY_COUNT)
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
            krate_bundle::MAX_ENTRY_COUNT
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
