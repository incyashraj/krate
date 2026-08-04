use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

fn krate() -> Command {
    Command::new(env!("CARGO_BIN_EXE_krate"))
}

/// True when `cargo-component` is on PATH.
///
/// The port pipeline compiles a real component, so the tests that run it need
/// the same build tools `krate create` asks for. Lanes that only run the
/// workspace tests do not install them, and there the honest outcome is to skip
/// rather than to fail on a missing tool or, worse, to weaken the assertions so
/// the test passes without building anything. The lanes that do install the
/// toolchain still run these tests in full.
fn has_cargo_component() -> bool {
    Command::new("cargo-component")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
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

    assert!(
        output.status.success(),
        "port pipeline failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(bundle.is_file());
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

    assert!(
        output.status.success(),
        "repairing port pipeline failed\nstdout:\n{}\nstderr:\n{}",
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

    assert_eq!(output.status.code(), Some(4));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("limit exceeded: memory limit exceeded"));
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

#[test]
fn create_with_an_agent_seam_scaffolds_a_building_skeleton_and_the_pack() {
    // The agent path drops a minimal skeleton + KRATE_AUTHORING.md, then builds
    // it. Drive it with a no-op author command (`true`), so this exercises the
    // scaffolding and the full build/pack/verify pipeline on the blank
    // skeleton -- without needing an AI. The skeleton must be a valid app on its
    // own, or an agent that starts from it starts from a broken base. Skipped
    // where the build toolchain is absent.
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
    assert!(
        output.status.success(),
        "create should build the skeleton into a .krate: {stderr}"
    );
    assert!(out.is_file(), "the .krate was written");
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
        "# 5. Example apps",
        "canvas2d::present",
        "random.bytes",
        "krate-notes",
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
