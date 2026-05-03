use std::path::PathBuf;
use std::process::Command;

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
    assert!(stdout.contains("cargo-component"));
    assert!(stdout.contains("wasm32-wasip1"));
    assert!(stdout.contains("wasm32-wasip2"));
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
    assert!(stdout.contains("number=12.5:Decimal:"));
    assert!(stdout.contains("time-ok=true"));
    assert!(stdout.contains("mono-ok=true"));
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
    assert!(stderr.contains("fs.read:./data/**"));
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

fn configured_hello_component() -> Option<PathBuf> {
    let Some(path) = std::env::var_os("LAYER36_HELLO_WASM") else {
        eprintln!("skipping hello component test: LAYER36_HELLO_WASM is not set");
        return None;
    };

    Some(workspace_path(PathBuf::from(path)))
}

fn configured_phase2_smoke_component() -> Option<PathBuf> {
    let Some(path) = std::env::var_os("LAYER36_PHASE2_SMOKE_WASM") else {
        eprintln!("skipping Phase 2 smoke component test: LAYER36_PHASE2_SMOKE_WASM is not set");
        return None;
    };

    Some(workspace_path(PathBuf::from(path)))
}

fn expected_hello_hash() -> Option<String> {
    std::env::var("LAYER36_HELLO_SHA256")
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
}
