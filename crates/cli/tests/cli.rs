use std::io::{Read, Write};
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
    assert!(stdout.contains("- fs.read:./notes/**"));
    assert!(stdout.contains("default grant        no"));
    assert!(stdout.contains("launch grant needed  yes"));
    assert!(stdout.contains("resource             ./notes/**"));
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
    assert!(stdout.contains(r#""capability": "io.stdout""#));
    assert!(stdout.contains(r#""default_grant": true"#));
    assert!(stdout.contains(r#""launch_grant_needed": false"#));
    assert!(stdout.contains(r#""capability": "fs.read:./notes/**""#));
    assert!(stdout.contains(r#""module": "fs""#));
    assert!(stdout.contains(r#""action": "read""#));
    assert!(stdout.contains(r#""resource": "./notes/**""#));
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
    assert!(stdout.contains("Phase 2 capabilities"));
    assert!(stdout.contains("io.args"));
    assert!(stdout.contains("fs.read:<path-glob>"));
    assert!(stdout.contains("net.connect:<host>:<port>"));
    assert!(stdout.contains("locale.format"));
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
    assert!(stdout.contains("cap = \"fs.read:./notes/**\""));
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
    let (addr, server) = spawn_http_fixture(body);
    let url = format!("http://{addr}/fixture.txt");

    let output = layer36()
        .args(["run", "--grant", &format!("net.connect:{addr}")])
        .arg(path)
        .args(["--", &url])
        .output()
        .expect("run layer36-curl component");
    server.join().expect("HTTP fixture thread completed");

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
    let (addr, server) = spawn_http_fixture(body);
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
    server.join().expect("HTTP fixture thread completed");

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
    let (addr, server) = spawn_http_fixture(body);
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
    server.join().expect("HTTP fixture thread completed");

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
fn configured_layer36_go_clock_component_matches_deterministic_fixture_snapshot() {
    let Some(path) =
        configured_component_from_env("LAYER36_GO_CLOCK_WASM", "layer36-go-clock component test")
    else {
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
    let Some(path) =
        configured_component_from_env("LAYER36_GO_CAT_WASM", "layer36-go-cat component test")
    else {
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
    let Some(path) =
        configured_component_from_env("LAYER36_GO_CURL_WASM", "layer36-go-curl component test")
    else {
        return;
    };

    let body = b"hello from go curl\n";
    let (addr, server) = spawn_http_fixture(body);
    let url = format!("http://{addr}/fixture.txt");

    let output = layer36()
        .args(["run", "--grant", &format!("net.connect:{addr}")])
        .arg(path)
        .args(["--", &url])
        .output()
        .expect("run layer36-go-curl component");
    server.join().expect("HTTP fixture thread completed");

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
fn configured_layer36_ts_clock_component_matches_deterministic_fixture_snapshot() {
    let Some(path) =
        configured_component_from_env("LAYER36_TS_CLOCK_WASM", "layer36-ts-clock component test")
    else {
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
    let Some(path) =
        configured_component_from_env("LAYER36_TS_CAT_WASM", "layer36-ts-cat component test")
    else {
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
    let Some(path) =
        configured_component_from_env("LAYER36_TS_CURL_WASM", "layer36-ts-curl component test")
    else {
        return;
    };

    let body = b"hello from ts curl\n";
    let (addr, server) = spawn_http_fixture(body);
    let url = format!("http://{addr}/fixture.txt");

    let output = layer36()
        .args(["run", "--grant", &format!("net.connect:{addr}")])
        .arg(path)
        .args(["--", &url])
        .output()
        .expect("run layer36-ts-curl component");
    server.join().expect("HTTP fixture thread completed");

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
    assert!(stdout.contains("fs.read:./data/**"));
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
    assert!(stdout.contains(r#""fs.read:./data/**""#));
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
    assert!(log.contains("  - fs.read:./data/**"));
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
    assert!(lines[0].contains(r#""fs.read:./data/**""#));
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
    assert!(stderr.contains("fs.read:./data/**"));
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

fn configured_component_from_env(env: &str, label: &str) -> Option<PathBuf> {
    let Some(path) = std::env::var_os(env) else {
        eprintln!("skipping {label}: {env} is not set");
        return None;
    };

    Some(workspace_path(PathBuf::from(path)))
}

fn spawn_http_fixture(body: &'static [u8]) -> (SocketAddr, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind HTTP fixture");
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
                    assert!(
                        Instant::now() < deadline,
                        "timed out waiting for HTTP fixture connection"
                    );
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
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .expect("write HTTP fixture headers");
        stream
            .write_all(body)
            .expect("write HTTP fixture response body");
    });

    (addr, handle)
}

fn expected_hello_hash() -> Option<String> {
    std::env::var("LAYER36_HELLO_SHA256")
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
}
