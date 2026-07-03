use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

fn layer36() -> Command {
    Command::new(env!("CARGO_BIN_EXE_layer36"))
}

#[test]
fn help_lists_phase_1_commands() {
    let output = layer36().arg("--help").output().expect("run layer36 help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Commands:"));
    assert!(stdout.contains("run"));
    assert!(stdout.contains("version"));
    assert!(stdout.contains("doctor"));
    assert!(stdout.contains("manifest"));
}

#[test]
fn version_prints_runtime_metadata() {
    let output = layer36()
        .arg("version")
        .output()
        .expect("run layer36 version");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("layer36"));
    assert!(stdout.contains("wasmtime  43.0.2"));
    assert!(stdout.contains("rustc"));
    assert!(stdout.contains("commit"));
}

#[test]
fn doctor_lists_phase_1_tooling() {
    let output = layer36()
        .arg("doctor")
        .output()
        .expect("run layer36 doctor");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Layer36 doctor"));
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
            world = "layer36:app/cli@0.1.0"

            [[capabilities]]
            cap = "io.stdout"
            rationale = "Print output"
            required = true
        "#,
    )
    .expect("write manifest");

    let output = layer36()
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
            world = "layer36:app/cli@0.1.0"

            [[capabilities]]
            cap = "io.stdout"
            rationale = "Print output"
            required = true
        "#,
    )
    .expect("write manifest");

    let output = layer36()
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
            world = "layer36:app/gui@0.2.0"

            [[capabilities]]
            cap = "io.stdout"
            rationale = "Debug output while the GUI runtime is in draft"
            required = true
        "#,
    )
    .expect("write manifest");

    let output = layer36()
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
    assert!(stdout.contains("world           layer36:app/gui@0.2.0"));
    assert!(stdout.contains("world status    Phase 3 GUI draft"));
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
            world = "layer36:app/cli@0.1.0"

            [[capabilities]]
            cap = "FS.read:./data/**"
            rationale = "Read data"
            required = true
        "#,
    )
    .expect("write manifest");

    let output = layer36()
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
            world = "layer36:app/cli@0.1.0"

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

    let output = layer36()
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
            world = "layer36:app/cli@0.1.0"

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

    let output = layer36()
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
    let output = layer36()
        .args(["manifest", "capabilities"])
        .output()
        .expect("run manifest capabilities");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Layer36 capabilities"));
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
            world = "layer36:app/gui@0.2.0"
        "#,
    )
    .expect("write manifest");

    let output = layer36()
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
fn manifest_capabilities_json_lists_phase_2_cap_table() {
    let output = layer36()
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
    let output = layer36()
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

    let check = layer36()
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

    let output = layer36()
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

    let second = layer36()
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
        "apps/layer36-clock/manifest.toml",
        "apps/layer36-cat/manifest.toml",
        "apps/layer36-curl/manifest.toml",
    ] {
        let manifest = workspace_path(PathBuf::from(manifest));
        let output = layer36()
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
    let output = layer36()
        .args(["run", "/definitely/not/a/component.wasm"])
        .output()
        .expect("run layer36 with missing input");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("input file does not exist"));
}

#[test]
fn run_rejects_empty_app_argument_before_runtime() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let wasm_path = dir.path().join("app.wasm");
    std::fs::write(&wasm_path, b"not actually wasm").expect("write wasm placeholder");

    let output = layer36()
        .arg("run")
        .arg(&wasm_path)
        .arg("--")
        .arg("")
        .output()
        .expect("run layer36 with empty app arg");

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

    let output = layer36()
        .arg("run")
        .arg(&wasm_path)
        .arg("--")
        .arg("bad\narg")
        .output()
        .expect("run layer36 with newline app arg");

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
            "skipping oversized raw-args spawn on Windows: the OS command-line limit is lower than Layer36's 64 KiB raw-args guard"
        );
        return;
    }

    let dir = tempfile::tempdir().expect("create temp dir");
    let wasm_path = dir.path().join("app.wasm");
    std::fs::write(&wasm_path, b"not actually wasm").expect("write wasm placeholder");
    let oversized = "x".repeat((64 * 1024) + 1);

    let output = layer36()
        .arg("run")
        .arg(&wasm_path)
        .arg("--")
        .arg(oversized)
        .output()
        .expect("run layer36 with oversized app arg payload");

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

    let mut cmd = layer36();
    cmd.arg("run").arg(&wasm_path).arg("--");
    for _ in 0..1025 {
        cmd.arg("x");
    }
    let output = cmd
        .output()
        .expect("run layer36 with too many app arguments");

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

    let output = layer36()
        .args(["run"])
        .arg(path)
        .output()
        .expect("run layer36 hello component");

    assert!(
        output.status.success(),
        "layer36 run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.lines().collect::<Vec<_>>(), ["Hello, Layer36!"]);
}

#[test]
fn configured_phase2_smoke_component_runs_through_uapi() {
    let Some(path) = configured_phase2_smoke_component() else {
        return;
    };

    let dir = tempfile::tempdir().expect("create temp dir");
    std::fs::write(
        dir.path().join("phase2-smoke-input.txt"),
        "Layer36 Phase 2 input\n",
    )
    .expect("write Phase 2 smoke input");

    let output = layer36()
        .current_dir(dir.path())
        .args(["run", "--grant", "fs.read:phase2-smoke-input.txt"])
        .arg(path)
        .output()
        .expect("run layer36 Phase 2 smoke component");

    assert!(
        output.status.success(),
        "layer36 run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("phase2-smoke ok"));
    assert!(stdout.contains("file=Layer36 Phase 2 input"));
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
        "Layer36 Phase 2 input\n",
    )
    .expect("write Phase 2 smoke input");

    let output = layer36()
        .current_dir(dir.path())
        .args(["run"])
        .arg(path)
        .output()
        .expect("run layer36 Phase 2 smoke component without grant");

    assert_eq!(output.status.code(), Some(25));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("phase2-smoke permission denied: fs.read"));
}

#[test]
fn configured_layer36_clock_component_uses_fixed_test_time() {
    let Some(path) = configured_layer36_clock_component() else {
        return;
    };

    let output = layer36()
        .args(["run", "--test-time", "1234567890"])
        .arg(path)
        .output()
        .expect("run layer36-clock component");

    assert!(
        output.status.success(),
        "layer36-clock failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("app=layer36-clock"));
    assert!(stdout.contains("timezone="));
    assert!(stdout.contains("locale="));
    assert!(stdout.contains("date=1970-01-15 06:56"));
}

#[test]
fn configured_layer36_clock_component_matches_deterministic_fixture_snapshot() {
    let Some(path) = configured_layer36_clock_component() else {
        return;
    };

    let output = layer36()
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
        .expect("run layer36-clock component with deterministic locale/timezone");

    assert!(
        output.status.success(),
        "layer36-clock deterministic snapshot failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        concat!(
            "app=layer36-clock\n",
            "timezone=UTC\n",
            "locale=en-US\n",
            "date=1970-01-15 06:56\n"
        )
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn configured_layer36_clock_component_applies_positive_timezone_offset() {
    let Some(path) = configured_layer36_clock_component() else {
        return;
    };

    let output = layer36()
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
        .expect("run layer36-clock with positive timezone offset");

    assert!(
        output.status.success(),
        "layer36-clock timezone offset run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("timezone=UTC+05:30"));
    assert!(stdout.contains("date=1970-01-15 12:26"));
    assert!(output.stderr.is_empty());
}

#[test]
fn configured_layer36_clock_component_applies_negative_timezone_offset() {
    let Some(path) = configured_layer36_clock_component() else {
        return;
    };

    let output = layer36()
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
        .expect("run layer36-clock with negative timezone offset");

    assert!(
        output.status.success(),
        "layer36-clock negative timezone offset run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("timezone=UTC-01:00"));
    assert!(stdout.contains("date=1969-12-31 23:00"));
    assert!(output.stderr.is_empty());
}

#[test]
fn configured_layer36_clock_component_runs_with_sample_manifest_auto_grant() {
    let Some(path) = configured_layer36_clock_component() else {
        return;
    };

    let output = layer36()
        .args([
            "run",
            "--auto-grant",
            "--manifest",
            sample_manifest("layer36-clock")
                .to_str()
                .expect("manifest path"),
            "--test-time",
            "1234567890",
        ])
        .arg(path)
        .output()
        .expect("run layer36-clock with sample manifest");

    assert!(
        output.status.success(),
        "layer36-clock manifest run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("app=layer36-clock"));
    assert!(stdout.contains("date=1970-01-15 06:56"));
}

#[test]
fn configured_layer36_cat_component_reads_granted_files() {
    let Some(path) = configured_layer36_cat_component() else {
        return;
    };

    let dir = tempfile::tempdir().expect("create temp dir");
    let fixtures = dir.path().join("fixtures");
    std::fs::create_dir(&fixtures).expect("create fixtures dir");
    std::fs::write(fixtures.join("a.txt"), "hello from A\n").expect("write fixture A");
    std::fs::write(fixtures.join("b.txt"), "hello from B\n").expect("write fixture B");

    let output = layer36()
        .current_dir(dir.path())
        .args(["run", "--grant", "fs.read:fixtures/**"])
        .arg(path)
        .args(["--", "fixtures/a.txt", "fixtures/b.txt"])
        .output()
        .expect("run layer36-cat component");

    assert!(
        output.status.success(),
        "layer36-cat failed\nstdout:\n{}\nstderr:\n{}",
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
fn configured_layer36_cat_component_reads_from_sandbox_root() {
    let Some(path) = configured_layer36_cat_component() else {
        return;
    };

    let dir = tempfile::tempdir().expect("create temp dir");
    let fixtures = dir.path().join("fixtures");
    std::fs::create_dir(&fixtures).expect("create fixtures dir");
    std::fs::write(fixtures.join("a.txt"), "hello from sandbox\n").expect("write fixture A");

    let output = layer36()
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
        .expect("run layer36-cat component with sandbox root");

    assert!(
        output.status.success(),
        "layer36-cat sandbox-root run failed\nstdout:\n{}\nstderr:\n{}",
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
fn configured_layer36_cat_component_runs_with_sample_manifest_auto_grant() {
    let Some(path) = configured_layer36_cat_component() else {
        return;
    };

    let dir = tempfile::tempdir().expect("create temp dir");
    let fixtures = dir.path().join("fixtures");
    std::fs::create_dir(&fixtures).expect("create fixtures dir");
    std::fs::write(fixtures.join("a.txt"), "hello from manifest cat\n").expect("write fixture A");

    let output = layer36()
        .current_dir(dir.path())
        .args([
            "run",
            "--auto-grant",
            "--manifest",
            sample_manifest("layer36-cat")
                .to_str()
                .expect("manifest path"),
        ])
        .arg(path)
        .args(["--", "./fixtures/a.txt"])
        .output()
        .expect("run layer36-cat with sample manifest");

    assert!(
        output.status.success(),
        "layer36-cat manifest run failed\nstdout:\n{}\nstderr:\n{}",
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
fn configured_layer36_cat_component_denies_missing_file_grant() {
    let Some(path) = configured_layer36_cat_component() else {
        return;
    };

    let dir = tempfile::tempdir().expect("create temp dir");
    let fixtures = dir.path().join("fixtures");
    std::fs::create_dir(&fixtures).expect("create fixtures dir");
    std::fs::write(fixtures.join("secret.txt"), "not granted\n").expect("write fixture");

    let output = layer36()
        .current_dir(dir.path())
        .args(["run"])
        .arg(path)
        .args(["--", "fixtures/secret.txt"])
        .output()
        .expect("run layer36-cat component without grant");

    assert_eq!(output.status.code(), Some(5));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("layer36-cat: permission denied: fixtures/secret.txt"));
}

#[test]
fn configured_layer36_cat_component_denies_file_outside_granted_glob() {
    let Some(path) = configured_layer36_cat_component() else {
        return;
    };

    let dir = tempfile::tempdir().expect("create temp dir");
    let fixtures = dir.path().join("fixtures");
    std::fs::create_dir_all(fixtures.join("public")).expect("create public fixtures dir");
    std::fs::write(fixtures.join("secret.txt"), "not granted\n").expect("write fixture");

    let output = layer36()
        .current_dir(dir.path())
        .args(["run", "--grant", "fs.read:fixtures/public/**"])
        .arg(path)
        .args(["--", "fixtures/secret.txt"])
        .output()
        .expect("run layer36-cat component outside granted glob");

    assert_eq!(output.status.code(), Some(5));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("layer36-cat: permission denied: fixtures/secret.txt"));
}

#[test]
fn configured_layer36_curl_component_fetches_granted_http_url() {
    let Some(path) = configured_layer36_curl_component() else {
        return;
    };

    let body = b"hello from curl\n";
    let Some((addr, server)) = spawn_http_fixture(body) else {
        return;
    };
    let url = format!("http://{addr}/fixture.txt");

    let output = layer36()
        .args(["run", "--grant", &format!("net.connect:{addr}")])
        .arg(path)
        .args(["--", &url])
        .output()
        .expect("run layer36-curl component");
    let accepted = server.join().expect("HTTP fixture thread completed");
    if !accepted {
        eprintln!("skipping layer36-curl success fixture: runtime could not connect to localhost fixture in this environment");
        return;
    }

    assert!(
        output.status.success(),
        "layer36-curl failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, body);
    assert!(output.stderr.is_empty());
}

#[test]
fn configured_layer36_curl_component_rejects_response_above_cli_limit() {
    let Some(path) = configured_layer36_curl_component() else {
        return;
    };

    let body = b"too large for this run\n";
    let Some((addr, server)) = spawn_http_fixture(body) else {
        return;
    };
    let url = format!("http://{addr}/fixture.txt");

    let output = layer36()
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
        .expect("run layer36-curl component with tiny HTTP response limit");
    let accepted = server.join().expect("HTTP fixture thread completed");
    if !accepted {
        eprintln!("skipping layer36-curl response-limit fixture: runtime could not connect to localhost fixture in this environment");
        return;
    }

    assert_eq!(output.status.code(), Some(21));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("layer36-curl: response too large"));
}

#[test]
fn configured_layer36_curl_component_runs_with_sample_manifest_auto_grant() {
    let Some(path) = configured_layer36_curl_component() else {
        return;
    };

    let body = b"hello from manifest curl\n";
    let Some((addr, server)) = spawn_http_fixture(body) else {
        return;
    };
    let url = format!("http://{addr}/fixture.txt");

    let output = layer36()
        .args([
            "run",
            "--auto-grant",
            "--manifest",
            sample_manifest("layer36-curl")
                .to_str()
                .expect("manifest path"),
        ])
        .arg(path)
        .args(["--", &url])
        .output()
        .expect("run layer36-curl with sample manifest");
    let accepted = server.join().expect("HTTP fixture thread completed");
    if !accepted {
        eprintln!("skipping layer36-curl manifest fixture: runtime could not connect to localhost fixture in this environment");
        return;
    }

    assert!(
        output.status.success(),
        "layer36-curl manifest run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, body);
    assert!(output.stderr.is_empty());
}

#[test]
fn configured_layer36_curl_component_denies_missing_net_grant() {
    let Some(path) = configured_layer36_curl_component() else {
        return;
    };

    let output = layer36()
        .args(["run"])
        .arg(path)
        .args(["--", "http://127.0.0.1:80/blocked"])
        .output()
        .expect("run layer36-curl component without grant");

    assert_eq!(output.status.code(), Some(5));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("layer36-curl: permission denied"));
}

#[test]
fn configured_layer36_curl_component_reports_connect_failure() {
    let Some(path) = configured_layer36_curl_component() else {
        return;
    };

    let Some(addr) = reserve_unused_local_addr() else {
        return;
    };
    let url = format!("http://{addr}/unreachable");

    let output = layer36()
        .args(["run", "--grant", &format!("net.connect:{addr}")])
        .arg(path)
        .args(["--", &url])
        .output()
        .expect("run layer36-curl against unused local port");

    assert_eq!(output.status.code(), Some(21));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("layer36-curl: connection failed"));
}

#[test]
fn configured_layer36_curl_component_reports_dns_failure() {
    let Some(path) = configured_layer36_curl_component() else {
        return;
    };

    let host = "layer36-does-not-exist.invalid";
    let url = format!("http://{host}/unreachable");

    let output = layer36()
        .args(["run", "--grant", &format!("net.connect:{host}:80")])
        .arg(path)
        .args(["--", &url])
        .output()
        .expect("run layer36-curl against unresolved host");

    assert_eq!(output.status.code(), Some(21));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("layer36-curl: dns lookup failed")
            || stderr.contains("layer36-curl: connection failed"),
        "unexpected stderr for unresolved host path: {stderr}"
    );
}

#[test]
fn configured_layer36_curl_component_reports_protocol_error() {
    let Some(path) = configured_layer36_curl_component() else {
        return;
    };

    let Some((addr, server)) = spawn_malformed_http_fixture(b"NOT-HTTP\r\n\r\n") else {
        return;
    };
    let url = format!("http://{addr}/malformed");

    let output = layer36()
        .args(["run", "--grant", &format!("net.connect:{addr}")])
        .arg(path)
        .args(["--", &url])
        .output()
        .expect("run layer36-curl against malformed HTTP fixture");
    let accepted = server
        .join()
        .expect("malformed HTTP fixture thread completed");
    if !accepted {
        eprintln!("skipping layer36-curl protocol fixture: runtime could not connect to localhost fixture in this environment");
        return;
    }

    assert_eq!(output.status.code(), Some(21));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("layer36-curl: protocol error"));
}

#[test]
fn configured_layer36_curl_component_reports_timeout() {
    let Some(path) = configured_layer36_curl_component() else {
        return;
    };

    let Some((addr, server)) = spawn_stalling_http_fixture(Duration::from_millis(1500)) else {
        return;
    };
    let url = format!("http://{addr}/stall");

    let output = layer36()
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
        .expect("run layer36-curl against stalling HTTP fixture");
    let accepted = server
        .join()
        .expect("stalling HTTP fixture thread completed");
    if !accepted {
        eprintln!("skipping layer36-curl timeout fixture: runtime could not connect to localhost fixture in this environment");
        return;
    }

    assert_eq!(output.status.code(), Some(21));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("layer36-curl: request timed out"));
}

#[test]
fn configured_layer36_go_clock_component_matches_deterministic_fixture_snapshot() {
    let Some(path) = configured_go_component(
        "LAYER36_GO_CLOCK_WASM",
        "layer36-go-clock component test",
        "layer36_go_clock.wasm",
    ) else {
        return;
    };

    let output = layer36()
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
        .expect("run layer36-go-clock component with deterministic locale/timezone");

    assert!(
        output.status.success(),
        "layer36-go-clock deterministic snapshot failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        concat!(
            "app=layer36-go-clock\n",
            "locale=en-US\n",
            "timezone=UTC\n",
            "date=1970-01-15 06:56\n"
        )
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn configured_layer36_go_cat_component_reads_granted_files() {
    let Some(path) = configured_go_component(
        "LAYER36_GO_CAT_WASM",
        "layer36-go-cat component test",
        "layer36_go_cat.wasm",
    ) else {
        return;
    };

    let dir = tempfile::tempdir().expect("create temp dir");
    let fixtures = dir.path().join("fixtures");
    std::fs::create_dir(&fixtures).expect("create fixtures dir");
    std::fs::write(fixtures.join("a.txt"), "hello from go A\n").expect("write fixture A");
    std::fs::write(fixtures.join("b.txt"), "hello from go B\n").expect("write fixture B");

    let output = layer36()
        .current_dir(dir.path())
        .args(["run", "--grant", "fs.read:fixtures/**"])
        .arg(path)
        .args(["--", "fixtures/a.txt", "fixtures/b.txt"])
        .output()
        .expect("run layer36-go-cat component");

    assert!(
        output.status.success(),
        "layer36-go-cat failed\nstdout:\n{}\nstderr:\n{}",
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
fn configured_layer36_go_curl_component_fetches_granted_http_url() {
    let Some(path) = configured_go_component(
        "LAYER36_GO_CURL_WASM",
        "layer36-go-curl component test",
        "layer36_go_curl.wasm",
    ) else {
        return;
    };

    let body = b"hello from go curl\n";
    let Some((addr, server)) = spawn_http_fixture(body) else {
        return;
    };
    let url = format!("http://{addr}/fixture.txt");

    let output = layer36()
        .args(["run", "--grant", &format!("net.connect:{addr}")])
        .arg(path)
        .args(["--", &url])
        .output()
        .expect("run layer36-go-curl component");
    let accepted = server.join().expect("HTTP fixture thread completed");
    if !accepted {
        eprintln!("skipping layer36-go-curl fixture: runtime could not connect to localhost fixture in this environment");
        return;
    }

    assert!(
        output.status.success(),
        "layer36-go-curl failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, body);
    assert!(output.stderr.is_empty());
}

#[test]
fn configured_layer36_go_curl_component_denies_missing_grant() {
    let Some(path) = configured_go_component(
        "LAYER36_GO_CURL_WASM",
        "layer36-go-curl component test",
        "layer36_go_curl.wasm",
    ) else {
        return;
    };

    let output = layer36()
        .arg("run")
        .arg(path)
        .args(["--", "http://example.com/"])
        .output()
        .expect("run layer36-go-curl component without grant");

    assert_eq!(output.status.code(), Some(5));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("layer36-go-curl: permission denied"));
}

#[test]
fn configured_layer36_go_curl_component_reports_unresolved_host() {
    let Some(path) = configured_go_component(
        "LAYER36_GO_CURL_WASM",
        "layer36-go-curl component test",
        "layer36_go_curl.wasm",
    ) else {
        return;
    };

    let host = "layer36-does-not-exist.invalid";
    let url = format!("http://{host}/unreachable");

    let output = layer36()
        .args(["run", "--grant", &format!("net.connect:{host}:80")])
        .arg(path)
        .args(["--", &url])
        .output()
        .expect("run layer36-go-curl against unresolved host");

    assert_eq!(output.status.code(), Some(21));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("layer36-go-curl: dns lookup failed")
            || stderr.contains("layer36-go-curl: connection failed")
            || stderr.contains("layer36-go-curl: fetch failed"),
        "unexpected unresolved-host stderr: {stderr}"
    );
}

#[test]
fn configured_layer36_ts_clock_component_matches_deterministic_fixture_snapshot() {
    let Some(path) = configured_ts_component(
        "LAYER36_TS_CLOCK_WASM",
        "layer36-ts-clock component test",
        "layer36_ts_clock.wasm",
    ) else {
        return;
    };

    let output = layer36()
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
        .expect("run layer36-ts-clock component with deterministic locale/timezone");

    assert!(
        output.status.success(),
        "layer36-ts-clock deterministic snapshot failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        concat!(
            "app=layer36-ts-clock\n",
            "locale=en-US\n",
            "timezone=UTC\n",
            "date=1970-01-15 06:56\n"
        )
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn configured_layer36_ts_cat_component_reads_granted_files() {
    let Some(path) = configured_ts_component(
        "LAYER36_TS_CAT_WASM",
        "layer36-ts-cat component test",
        "layer36_ts_cat.wasm",
    ) else {
        return;
    };

    let dir = tempfile::tempdir().expect("create temp dir");
    let fixtures = dir.path().join("fixtures");
    std::fs::create_dir(&fixtures).expect("create fixtures dir");
    std::fs::write(fixtures.join("a.txt"), "hello from ts A\n").expect("write fixture A");
    std::fs::write(fixtures.join("b.txt"), "hello from ts B\n").expect("write fixture B");

    let output = layer36()
        .current_dir(dir.path())
        .args(["run", "--grant", "fs.read:fixtures/**"])
        .arg(path)
        .args(["--", "fixtures/a.txt", "fixtures/b.txt"])
        .output()
        .expect("run layer36-ts-cat component");

    assert!(
        output.status.success(),
        "layer36-ts-cat failed\nstdout:\n{}\nstderr:\n{}",
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
fn configured_layer36_ts_curl_component_fetches_granted_http_url() {
    let Some(path) = configured_ts_component(
        "LAYER36_TS_CURL_WASM",
        "layer36-ts-curl component test",
        "layer36_ts_curl.wasm",
    ) else {
        return;
    };

    let body = b"hello from ts curl\n";
    let Some((addr, server)) = spawn_http_fixture(body) else {
        return;
    };
    let url = format!("http://{addr}/fixture.txt");

    let output = layer36()
        .args(["run", "--grant", &format!("net.connect:{addr}")])
        .arg(path)
        .args(["--", &url])
        .output()
        .expect("run layer36-ts-curl component");
    let accepted = server.join().expect("HTTP fixture thread completed");
    if !accepted {
        eprintln!("skipping layer36-ts-curl fixture: runtime could not connect to localhost fixture in this environment");
        return;
    }

    assert!(
        output.status.success(),
        "layer36-ts-curl failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, body);
    assert!(output.stderr.is_empty());
}

#[test]
fn configured_layer36_ts_curl_component_denies_missing_grant() {
    let Some(path) = configured_ts_component(
        "LAYER36_TS_CURL_WASM",
        "layer36-ts-curl component test",
        "layer36_ts_curl.wasm",
    ) else {
        return;
    };

    let output = layer36()
        .arg("run")
        .arg(path)
        .args(["--", "http://example.com/"])
        .output()
        .expect("run layer36-ts-curl component without grant");

    assert_eq!(output.status.code(), Some(5));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("layer36-ts-curl: permission denied"));
}

#[test]
fn configured_layer36_ts_curl_component_reports_unresolved_host() {
    let Some(path) = configured_ts_component(
        "LAYER36_TS_CURL_WASM",
        "layer36-ts-curl component test",
        "layer36_ts_curl.wasm",
    ) else {
        return;
    };

    let host = "layer36-does-not-exist.invalid";
    let url = format!("http://{host}/unreachable");

    let output = layer36()
        .args(["run", "--grant", &format!("net.connect:{host}:80")])
        .arg(path)
        .args(["--", &url])
        .output()
        .expect("run layer36-ts-curl against unresolved host");

    assert_eq!(output.status.code(), Some(21));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("layer36-ts-curl: dns lookup failed")
            || stderr.contains("layer36-ts-curl: connection failed")
            || stderr.contains("layer36-ts-curl: fetch failed"),
        "unexpected unresolved-host stderr: {stderr}"
    );
}

#[test]
fn configured_layer36_ts_curl_component_reports_invalid_url() {
    let Some(path) = configured_ts_component(
        "LAYER36_TS_CURL_WASM",
        "layer36-ts-curl component test",
        "layer36_ts_curl.wasm",
    ) else {
        return;
    };

    let output = layer36()
        .args(["run", "--grant", "net.connect:*:*"])
        .arg(path)
        .args(["--", "not-a-url"])
        .output()
        .expect("run layer36-ts-curl against invalid URL");

    assert_eq!(output.status.code(), Some(20));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("layer36-ts-curl: invalid url"));
}

#[test]
fn language_variants_curl_permission_denied_matches_rust_go_ts() {
    let Some(rust_path) = configured_layer36_curl_component() else {
        return;
    };
    let Some(go_path) = configured_go_component(
        "LAYER36_GO_CURL_WASM",
        "layer36-go-curl component test",
        "layer36_go_curl.wasm",
    ) else {
        eprintln!("skipping language variant curl denial parity: Go fixture is unavailable");
        return;
    };
    let Some(ts_path) = configured_ts_component(
        "LAYER36_TS_CURL_WASM",
        "layer36-ts-curl component test",
        "layer36_ts_curl.wasm",
    ) else {
        eprintln!(
            "skipping language variant curl denial parity: TypeScript fixture is unavailable"
        );
        return;
    };

    let run_without_grant = |path: &PathBuf, label: &str| {
        let output = layer36()
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

    let rust_stderr = run_without_grant(&rust_path, "layer36-curl");
    let go_stderr = run_without_grant(&go_path, "layer36-go-curl");
    let ts_stderr = run_without_grant(&ts_path, "layer36-ts-curl");
    assert!(rust_stderr.contains("permission denied"));
    assert!(go_stderr.contains("permission denied"));
    assert!(ts_stderr.contains("permission denied"));
}

#[test]
fn language_variants_curl_invalid_url_matches_rust_go_ts() {
    let Some(rust_path) = configured_layer36_curl_component() else {
        return;
    };
    let Some(go_path) = configured_go_component(
        "LAYER36_GO_CURL_WASM",
        "layer36-go-curl component test",
        "layer36_go_curl.wasm",
    ) else {
        eprintln!("skipping language variant curl invalid-url parity: Go fixture is unavailable");
        return;
    };
    let Some(ts_path) = configured_ts_component(
        "LAYER36_TS_CURL_WASM",
        "layer36-ts-curl component test",
        "layer36_ts_curl.wasm",
    ) else {
        eprintln!(
            "skipping language variant curl invalid-url parity: TypeScript fixture is unavailable"
        );
        return;
    };

    let run_invalid_url = |path: &PathBuf, label: &str| {
        let output = layer36()
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

    let rust_stderr = run_invalid_url(&rust_path, "layer36-curl");
    let go_stderr = run_invalid_url(&go_path, "layer36-go-curl");
    let ts_stderr = run_invalid_url(&ts_path, "layer36-ts-curl");
    assert!(rust_stderr.contains("invalid url"));
    assert!(go_stderr.contains("invalid url"));
    assert!(ts_stderr.contains("invalid url"));
}

#[test]
fn language_variants_curl_unresolved_host_matches_rust_go_ts() {
    let Some(rust_path) = configured_layer36_curl_component() else {
        return;
    };
    let Some(go_path) = configured_go_component(
        "LAYER36_GO_CURL_WASM",
        "layer36-go-curl component test",
        "layer36_go_curl.wasm",
    ) else {
        eprintln!(
            "skipping language variant curl unresolved-host parity: Go fixture is unavailable"
        );
        return;
    };
    let Some(ts_path) = configured_ts_component(
        "LAYER36_TS_CURL_WASM",
        "layer36-ts-curl component test",
        "layer36_ts_curl.wasm",
    ) else {
        eprintln!(
            "skipping language variant curl unresolved-host parity: TypeScript fixture is unavailable"
        );
        return;
    };

    let host = "layer36-does-not-exist.invalid";
    let url = format!("http://{host}/unreachable");
    let grant = format!("net.connect:{host}:80");

    let run_unresolved = |path: &PathBuf, label: &str| {
        let output = layer36()
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

    let rust_stderr = run_unresolved(&rust_path, "layer36-curl");
    let go_stderr = run_unresolved(&go_path, "layer36-go-curl");
    let ts_stderr = run_unresolved(&ts_path, "layer36-ts-curl");

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
fn configured_layer36_go_curl_component_reports_invalid_url() {
    let Some(path) = configured_go_component(
        "LAYER36_GO_CURL_WASM",
        "layer36-go-curl component test",
        "layer36_go_curl.wasm",
    ) else {
        return;
    };

    let output = layer36()
        .args(["run", "--grant", "net.connect:*:*"])
        .arg(path)
        .args(["--", "not-a-url"])
        .output()
        .expect("run layer36-go-curl against invalid URL");

    assert_eq!(output.status.code(), Some(20));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("layer36-go-curl: invalid url"));
}

#[test]
fn language_variants_clock_output_matches_across_rust_go_ts() {
    let Some(rust_path) = configured_layer36_clock_component() else {
        return;
    };
    let Some(go_path) = configured_go_component(
        "LAYER36_GO_CLOCK_WASM",
        "layer36-go-clock component test",
        "layer36_go_clock.wasm",
    ) else {
        eprintln!("skipping language variant clock parity: Go fixture is unavailable");
        return;
    };
    let Some(ts_path) = configured_ts_component(
        "LAYER36_TS_CLOCK_WASM",
        "layer36-ts-clock component test",
        "layer36_ts_clock.wasm",
    ) else {
        eprintln!("skipping language variant clock parity: TypeScript fixture is unavailable");
        return;
    };

    let run_clock = |path: &PathBuf, label: &str| {
        let output = layer36()
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

    let rust_stdout = run_clock(&rust_path, "layer36-clock");
    let go_stdout = run_clock(&go_path, "layer36-go-clock");
    let ts_stdout = run_clock(&ts_path, "layer36-ts-clock");

    assert_eq!(go_stdout, rust_stdout, "Go clock output drifted from Rust");
    assert_eq!(
        ts_stdout, rust_stdout,
        "TypeScript clock output drifted from Rust"
    );
}

#[test]
fn language_variants_cat_output_matches_across_rust_go_ts() {
    let Some(rust_path) = configured_layer36_cat_component() else {
        return;
    };
    let Some(go_path) = configured_go_component(
        "LAYER36_GO_CAT_WASM",
        "layer36-go-cat component test",
        "layer36_go_cat.wasm",
    ) else {
        eprintln!("skipping language variant cat parity: Go fixture is unavailable");
        return;
    };
    let Some(ts_path) = configured_ts_component(
        "LAYER36_TS_CAT_WASM",
        "layer36-ts-cat component test",
        "layer36_ts_cat.wasm",
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
        let output = layer36()
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

    let rust_stdout = run_cat(&rust_path, "layer36-cat");
    let go_stdout = run_cat(&go_path, "layer36-go-cat");
    let ts_stdout = run_cat(&ts_path, "layer36-ts-cat");

    assert_eq!(go_stdout, rust_stdout, "Go cat output drifted from Rust");
    assert_eq!(
        ts_stdout, rust_stdout,
        "TypeScript cat output drifted from Rust"
    );
}

#[test]
fn language_variants_curl_output_matches_across_rust_go_ts() {
    let Some(rust_path) = configured_layer36_curl_component() else {
        return;
    };
    let Some(go_path) = configured_go_component(
        "LAYER36_GO_CURL_WASM",
        "layer36-go-curl component test",
        "layer36_go_curl.wasm",
    ) else {
        eprintln!("skipping language variant curl parity: Go fixture is unavailable");
        return;
    };
    let Some(ts_path) = configured_ts_component(
        "LAYER36_TS_CURL_WASM",
        "layer36-ts-curl component test",
        "layer36_ts_curl.wasm",
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
        let output = layer36()
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

    let rust_stdout = run_curl(&rust_path, "layer36-curl");
    let go_stdout = run_curl(&go_path, "layer36-go-curl");
    let ts_stdout = run_curl(&ts_path, "layer36-ts-curl");

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

    let output = layer36()
        .args(["run", "--fuel", "1"])
        .arg(path)
        .output()
        .expect("run layer36 hello component with low fuel");

    assert_eq!(output.status.code(), Some(4));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("limit exceeded: fuel exhausted"));
}

#[test]
fn memory_limit_exits_with_limit_code() {
    let Some(path) = configured_hello_component() else {
        return;
    };

    let output = layer36()
        .args(["run", "--mem-limit", "0"])
        .arg(path)
        .output()
        .expect("run layer36 hello component with low memory");

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
            world = "layer36:app/cli@0.1.0"

            [[capabilities]]
            cap = "fs.read:./data/**"
            rationale = "Read data"
            required = true
        "#,
    )
    .expect("write manifest");

    let output = layer36()
        .arg("run")
        .arg(&wasm_path)
        .output()
        .expect("run layer36 with sidecar manifest");

    assert_eq!(output.status.code(), Some(5));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("permission denied"));
    assert!(stderr.contains("fs.read:data/**"));
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
            world = "layer36:app/cli@0.1.0"
        "#,
    )
    .expect("write manifest");

    let output = layer36()
        .arg("run")
        .arg(&wasm_path)
        .output()
        .expect("run layer36 with mismatched sidecar manifest");

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
            world = "layer36:app/cli@0.1.0"

            [[capabilities]]
            cap = "fs.read:./data/**"
            rationale = "Read data"
            required = true
        "#,
    )
    .expect("write manifest");

    let output = layer36()
        .args(["run", "--grant", "fs.read:./data/**"])
        .arg(&wasm_path)
        .output()
        .expect("run layer36 with granted sidecar manifest");

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid wasm component"));
}

#[test]
fn run_dump_caps_prints_effective_policy_without_running_component() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let wasm_path = dir.path().join("app.wasm");
    std::fs::write(&wasm_path, b"not actually wasm").expect("write wasm placeholder");

    let output = layer36()
        .args(["run", "--dump-caps", "--grant", "fs.read:./data/**"])
        .arg(&wasm_path)
        .output()
        .expect("run layer36 dump caps");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains("Effective capabilities"));
    assert!(stdout.contains("io.stdout"));
    assert!(stdout.contains("fs.read:data/**"));
    assert!(!stderr.contains("invalid wasm component"));
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
            world = "layer36:app/cli@0.1.0"

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

    let output = layer36()
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
        .expect("run layer36 dump caps json");

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
            world = "layer36:app/cli@0.1.0"

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

    let output = layer36()
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
    assert!(log.contains("Layer36 grant log"));
    assert!(log.contains("app id           com.example.audit"));
    assert!(log.contains("app name         Audit"));
    assert!(log.contains("manifest world   layer36:app/cli@0.1.0"));
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
            world = "layer36:app/cli@0.1.0"

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

    let output = layer36()
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
    assert!(lines[0].contains(r#""event":"layer36.grants""#));
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
            world = "layer36:app/cli@0.1.0"

            [[capabilities]]
            cap = "net.connect:api.example.com:443"
            rationale = "Sync data"
            required = true
        "#,
    )
    .expect("write manifest");

    let output = layer36()
        .args(["run", "--auto-grant"])
        .arg(&wasm_path)
        .output()
        .expect("run layer36 with auto-grant");

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
            world = "layer36:app/cli@0.1.0"

            [[capabilities]]
            cap = "fs.read:./data/**"
            rationale = "Read data"
            required = true
        "#,
    )
    .expect("write manifest");

    let mut child = layer36()
        .args(["run", "--prompt"])
        .arg(&wasm_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn layer36 with prompt");

    child
        .stdin
        .as_mut()
        .expect("child stdin")
        .write_all(b"a\n")
        .expect("write prompt response");

    let output = child.wait_with_output().expect("wait for prompt run");

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Requests the following capabilities"));
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

fn configured_hello_component() -> Option<PathBuf> {
    configured_component_from_env("LAYER36_HELLO_WASM", "hello component test")
}

fn configured_phase2_smoke_component() -> Option<PathBuf> {
    configured_component_from_env("LAYER36_PHASE2_SMOKE_WASM", "Phase 2 smoke component test")
}

fn configured_layer36_clock_component() -> Option<PathBuf> {
    configured_component_from_env("LAYER36_CLOCK_WASM", "layer36-clock component test")
}

fn configured_layer36_cat_component() -> Option<PathBuf> {
    configured_component_from_env("LAYER36_CAT_WASM", "layer36-cat component test")
}

fn configured_layer36_curl_component() -> Option<PathBuf> {
    configured_component_from_env("LAYER36_CURL_WASM", "layer36-curl component test")
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
    std::env::var("LAYER36_HELLO_SHA256")
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
}
