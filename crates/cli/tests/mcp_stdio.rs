//! Drive `krate mcp` over stdio exactly the way Claude Desktop or Cursor does.
//!
//! The unit tests in `krate-mcp` prove the protocol layer and the job store in
//! isolation. This proves the thing a user actually gets: launch the real
//! binary as a subprocess, write newline-delimited JSON-RPC to its stdin, read
//! responses from its stdout, and go all the way from `initialize` to a
//! finished `.krate` on disk.
//!
//! The build here uses the built-in template generator, not an AI. That is
//! deliberate: the test must not need a model, an API key, or five minutes, and
//! the plumbing being tested -- start a job, poll it, package the result -- is
//! identical either way.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// The binary under test, from the same target directory cargo built it into.
fn krate_bin() -> PathBuf {
    let mut path = std::env::current_exe().expect("test binary path");
    path.pop(); // the deps/ directory
    if path.ends_with("deps") {
        path.pop();
    }
    path.join(format!("krate{}", std::env::consts::EXE_SUFFIX))
}

/// Whether a real build can run here. Without cargo-component there is no
/// toolchain to compile a component with, so the build half is skipped rather
/// than reported as a failure of this code.
fn has_cargo_component() -> bool {
    Command::new("cargo-component")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// One connected MCP server, spoken to the way a client speaks to it.
struct Client {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl Client {
    fn launch(root: &std::path::Path) -> Self {
        let mut child = Command::new(krate_bin())
            .arg("mcp")
            // Keep every build, scratch check, and frame inside the test's own
            // directory rather than the user's real ~/.krate.
            .env("KRATE_MCP_ROOT", root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // The spec allows a server to log freely on stderr, and a client
            // may ignore it. Inherit so a failure here is visible in the test
            // output instead of vanishing.
            .stderr(Stdio::inherit())
            .spawn()
            .expect("launch `krate mcp`");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        Self {
            child,
            stdin,
            stdout,
            next_id: 0,
        }
    }

    /// Send a request and read its response.
    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let encoded = serde_json::to_string(&message).expect("encode");
        assert!(
            !encoded.contains('\n'),
            "a client must never send an embedded newline"
        );
        writeln!(self.stdin, "{encoded}").expect("write request");
        self.stdin.flush().expect("flush");

        let mut line = String::new();
        self.stdout.read_line(&mut line).expect("read response");
        assert!(
            !line.trim().is_empty(),
            "the server closed the stream instead of answering {method}"
        );
        let response: Value = serde_json::from_str(line.trim())
            .unwrap_or_else(|err| panic!("response to {method} was not JSON: {err}\n{line}"));
        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["id"], id, "response id must match the request id");
        response
    }

    /// Send a notification, which by definition gets no response.
    fn notify(&mut self, method: &str) {
        let message = json!({ "jsonrpc": "2.0", "method": method });
        writeln!(
            self.stdin,
            "{}",
            serde_json::to_string(&message).expect("encode")
        )
        .expect("write notification");
        self.stdin.flush().expect("flush");
    }

    /// Call a tool and return its result, insisting it did not error.
    fn call_tool(&mut self, name: &str, arguments: Value) -> Value {
        let response = self.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        );
        assert!(
            response.get("error").is_none(),
            "{name} failed at the protocol level: {}",
            response["error"]
        );
        let result = &response["result"];
        assert_eq!(
            result["isError"], false,
            "{name} reported an error: {}",
            result["content"][0]["text"]
        );
        result.clone()
    }

    /// Call a tool expecting it to report a tool-execution error, and return
    /// the text the model would read.
    fn call_tool_expecting_error(&mut self, name: &str, arguments: Value) -> String {
        let response = self.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        );
        assert!(
            response.get("error").is_none(),
            "{name} must not be a protocol error"
        );
        assert_eq!(
            response["result"]["isError"], true,
            "{name} should have failed"
        );
        response["result"]["content"][0]["text"]
            .as_str()
            .expect("error text")
            .to_string()
    }

    /// Shut down the way the spec says a client does: close stdin, then wait.
    fn shutdown(mut self) {
        drop(self.stdin);
        let status = self.child.wait().expect("wait for the server to exit");
        assert!(
            status.success(),
            "the server must exit cleanly when its stdin is closed, got {status}"
        );
    }
}

#[test]
fn a_client_can_initialize_list_tools_and_call_the_cheap_ones() {
    let root = tempfile::tempdir().expect("tempdir");
    let mut client = Client::launch(root.path());

    // 1. The handshake.
    let init = client.request(
        "initialize",
        json!({
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": { "name": "krate-test-client", "version": "1.0.0" },
        }),
    );
    assert_eq!(init["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(init["result"]["serverInfo"]["name"], "krate");
    assert!(init["result"]["capabilities"]["tools"].is_object());

    // 2. The initialized notification. Nothing may come back for it -- if the
    //    server answered, the next read below would get the wrong message and
    //    every subsequent id would be off by one.
    client.notify("notifications/initialized");

    // 3. Discovery.
    let list = client.request("tools/list", json!({}));
    let tools = list["result"]["tools"].as_array().expect("tools");
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().expect("name"))
        .collect();
    for expected in [
        "krate_schema",
        "krate_examples",
        "krate_start_build",
        "krate_build_status",
        "krate_check",
        "krate_package",
        "krate_run",
    ] {
        assert!(
            names.contains(&expected),
            "tools/list is missing {expected}"
        );
    }

    // 4. The teaching tools. These are what a model calls first, and they must
    //    return real content rather than a promise of it.
    let schema = client.call_tool("krate_schema", json!({}));
    let pack = schema["structuredContent"]["authoring_pack"]
        .as_str()
        .expect("the authoring pack");
    assert!(
        pack.contains("no_std") && pack.contains("krate:"),
        "the pack must carry the real authoring rules"
    );

    let examples = client.call_tool("krate_examples", json!({ "kind": "gui" }));
    let list = examples["structuredContent"]["examples"]
        .as_array()
        .expect("examples");
    assert!(!list.is_empty());
    for example in list {
        let lib = example["files"]["src/lib.rs"].as_str().expect("lib.rs");
        // A whole file, not a snippet: this is the point of the tool.
        assert!(
            lib.len() > 1000,
            "{} is too short to be a whole app",
            example["name"]
        );
        assert!(lib.contains("#![no_std]"));
    }

    // 5. A bad call must come back as a tool error the model can act on, not a
    //    protocol error it cannot see.
    let message = client.call_tool_expecting_error("krate_start_build", json!({}));
    assert!(
        message.contains("description"),
        "the error must name what is missing: {message}"
    );

    // 6. An unknown tool is the one case that IS a protocol error.
    let response = client.request("tools/call", json!({ "name": "krate_nonsense" }));
    assert_eq!(response["error"]["code"], -32601);
    assert!(
        response["error"]["message"]
            .as_str()
            .expect("message")
            .contains("krate_schema"),
        "an unknown tool should name the real ones"
    );

    client.shutdown();
}

#[test]
fn a_client_can_drive_a_build_from_start_to_a_finished_krate_file() {
    if !has_cargo_component() {
        eprintln!("skipping: cargo-component is not installed, so no component can be built");
        return;
    }

    let root = tempfile::tempdir().expect("tempdir");
    let mut client = Client::launch(root.path());

    client.request(
        "initialize",
        json!({ "protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": { "name": "t", "version": "1" } }),
    );
    client.notify("notifications/initialized");

    // Start the build. The whole reason for the job shape: this must return
    // immediately, long before the build could possibly be finished.
    let started_at = Instant::now();
    let started = client.call_tool(
        "krate_start_build",
        json!({ "description": "a checklist app that saves locally", "name": "todo-list" }),
    );
    let start_took = started_at.elapsed();
    assert!(
        start_took < Duration::from_secs(20),
        "krate_start_build must not block on the build; it took {start_took:?}"
    );
    let job_id = started["structuredContent"]["job_id"]
        .as_str()
        .expect("a job id")
        .to_string();
    assert_eq!(started["structuredContent"]["status"], "running");
    assert_eq!(started["structuredContent"]["name"], "todo-list");

    // Poll, exactly as a model would.
    let deadline = Instant::now() + Duration::from_secs(600);
    let final_status = loop {
        let status = client.call_tool("krate_build_status", json!({ "job_id": &job_id }));
        let phase = status["structuredContent"]["status"]
            .as_str()
            .expect("a status")
            .to_string();
        // Whatever the phase, the report must be usable: it always says what
        // to do next, so a model is never left guessing.
        assert!(
            status["structuredContent"]["next"].is_string(),
            "every status must tell the model what to do next"
        );
        if phase != "running" {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "the build never finished within ten minutes"
        );
        std::thread::sleep(Duration::from_secs(2));
    };

    assert_eq!(
        final_status["structuredContent"]["status"], "succeeded",
        "the build failed: {}",
        final_status["structuredContent"]["error"]
    );

    // Package it. This is the thing the person actually receives.
    let package = client.call_tool(
        "krate_package",
        json!({ "job_id": &job_id, "include_base64": true }),
    );
    let path = package["structuredContent"]["path"]
        .as_str()
        .expect("a path");
    let bundle = PathBuf::from(path);
    assert!(bundle.is_file(), "the .krate must exist at {path}");
    let on_disk = std::fs::metadata(&bundle).expect("metadata").len();
    assert_eq!(
        package["structuredContent"]["bytes"]
            .as_u64()
            .expect("bytes"),
        on_disk,
        "the reported size must match the file on disk"
    );
    assert!(on_disk > 0);

    // The base64 must decode back to exactly the file, or a client without
    // filesystem access gets a corrupt app.
    let encoded = package["structuredContent"]["base64"]
        .as_str()
        .expect("base64");
    assert_eq!(
        decode_base64(encoded),
        std::fs::read(&bundle).expect("read bundle"),
        "the inline base64 must be the bundle byte for byte"
    );

    // The permissions the app asks for must be reported: this is what the
    // person is agreeing to when they open it.
    let permissions = package["structuredContent"]["requested_permissions"]
        .as_array()
        .expect("requested permissions");
    assert!(!permissions.is_empty());

    // And the app must really run: hand it to the execution half of the same
    // server, which is the strongest evidence the file works.
    let ran = client.call_tool(
        "run_component",
        json!({ "bundle": path, "auto_grant": true, "app_args": ["quick"] }),
    );
    assert_eq!(ran["structuredContent"]["schema"], "krate.run.v1");

    client.shutdown();
}

#[test]
fn the_oracle_reports_a_broken_app_with_the_stage_and_the_fix() {
    if !has_cargo_component() {
        eprintln!("skipping: cargo-component is not installed");
        return;
    }

    let root = tempfile::tempdir().expect("tempdir");
    let mut client = Client::launch(root.path());
    client.request(
        "initialize",
        json!({ "protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": { "name": "t", "version": "1" } }),
    );
    client.notify("notifications/initialized");

    // Source that cannot possibly build. What matters is not that it fails but
    // that the failure names the stage and carries a fix -- that is what makes
    // the loop closeable by a model rather than a dead end.
    let broken = client.call_tool(
        "krate_check",
        json!({
            "files": {
                "Cargo.toml": "[package]\nname = \"broken\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
                "src/lib.rs": "this is not rust at all",
            }
        }),
    );
    let verdict = &broken["structuredContent"];
    assert_eq!(verdict["ok"], false);
    let stage = verdict["stage"].as_str().expect("a stage");
    // manifest.toml is missing, so it must stop at layout and say so.
    assert_eq!(
        stage, "layout",
        "check-app should fail at the first missing thing"
    );
    assert!(
        verdict["fix"]
            .as_str()
            .expect("a fix")
            .contains("manifest.toml"),
        "the fix must name the missing file: {}",
        verdict["fix"]
    );
    assert!(verdict["next"].is_string());

    client.shutdown();
}

/// Decode standard base64, for checking what the server encoded.
fn decode_base64(text: &str) -> Vec<u8> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut lookup = [255u8; 256];
    for (index, byte) in ALPHABET.iter().enumerate() {
        lookup[*byte as usize] = index as u8;
    }

    let mut out = Vec::new();
    let mut accumulator: u32 = 0;
    let mut bits = 0;
    for byte in text.bytes() {
        if byte == b'=' {
            break;
        }
        let value = lookup[byte as usize];
        assert_ne!(value, 255, "not base64: {}", byte as char);
        accumulator = (accumulator << 6) | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((accumulator >> bits) as u8);
        }
    }
    out
}

#[cfg(test)]
mod decoder_tests {
    use super::decode_base64;

    #[test]
    fn the_test_decoder_is_itself_correct() {
        // A broken decoder here would make the round-trip check above pass
        // against a corrupt encoder, so pin it to the RFC 4648 vectors.
        assert_eq!(decode_base64(""), b"");
        assert_eq!(decode_base64("Zg=="), b"f");
        assert_eq!(decode_base64("Zm8="), b"fo");
        assert_eq!(decode_base64("Zm9v"), b"foo");
        assert_eq!(decode_base64("Zm9vYg=="), b"foob");
        assert_eq!(decode_base64("Zm9vYmE="), b"fooba");
        assert_eq!(decode_base64("Zm9vYmFy"), b"foobar");
        assert_eq!(decode_base64("AP+A"), [0x00, 0xff, 0x80]);
    }
}

/// The server must refuse a request Krate cannot serve, before starting a build.
///
/// This matters more over MCP than at the command line. Someone who typed
/// `krate create` knows what they asked for and reads the output. A model that
/// gets a job id polls it, sees every mechanical stage pass -- builds, imports
/// zero OS calls, runs, paints a frame -- and tells the person their email app
/// is ready. None of those stages is about whether the app does what was asked.
///
/// The pairing is the point: "download my email" is refused, "a full email
/// client" is not. A screen matching the topic rather than the impossible
/// action would fail the second half and make Krate look incapable.
#[test]
fn the_server_refuses_what_krate_cannot_build_but_not_what_it_can() {
    let root = tempfile::tempdir().expect("tempdir");
    let mut client = Client::launch(root.path());
    client.request(
        "initialize",
        json!({
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": { "name": "krate-test-client", "version": "1.0.0" },
        }),
    );

    let refusal = client.call_tool_expecting_error(
        "krate_start_build",
        json!({ "description": "download my email and show me the unread ones" }),
    );
    assert!(
        refusal.contains("cannot build"),
        "the refusal must say so plainly: {refusal}"
    );
    assert!(
        refusal.contains("instead"),
        "a refusal without a next step reads as a dead end: {refusal}"
    );

    // The near-miss a topic matcher would get wrong.
    let started = client.call_tool(
        "krate_start_build",
        json!({ "description": "a full email client with fake sample messages" }),
    );
    assert!(
        started["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("job_id"),
        "a buildable request must start a job: {started}"
    );

    client.shutdown();
}
