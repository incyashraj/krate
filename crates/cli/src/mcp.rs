//! The Krate MCP server: authoring plus sandboxed execution, over stdio.
//!
//! Two families of tool, deliberately in one server, because a model that just
//! built an app should be able to run it without the user wiring up a second
//! connector:
//!
//!  - **Authoring** (`krate_schema`, `krate_examples`, `krate_start_build`,
//!    `krate_build_status`, `krate_check`, `krate_package`, `krate_run`), from
//!    the `krate-mcp` crate. These wrap the authoring loop -- the context pack,
//!    `krate create`, and the `check-app` oracle -- so someone can get a
//!    working `.krate` by talking.
//!  - **Execution** (`run_component`, `inspect_bundle`), below. An agent
//!    framework supplies a component and grants; Krate runs it inside the
//!    capability sandbox and returns a `krate.run.v1` report.
//!
//! This file owns only the execution half and the composition. The protocol
//! itself lives in `krate_mcp::protocol`, tested on its own.
//!
//! Transport: newline-delimited JSON-RPC 2.0 over stdio. See
//! `docs/mcp-setup.md` for the client configuration.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use krate_manifest::Manifest;
use krate_mcp::{KrateTools, ToolSet};
use krate_policy::SessionPolicy;
use krate_runtime::{embed, Config};
use serde_json::{json, Value};

use crate::authoring_context;

pub fn serve() -> Result<()> {
    // The binary that runs builds and checks is this one. Resolving it here,
    // rather than trusting a bare `krate` on PATH, means the server always
    // drives the Krate it is part of -- an installed older release on PATH was
    // exactly how a "fixed" bug appeared to come back.
    let krate_bin = std::env::current_exe().context("locate the krate binary")?;
    let authoring = KrateTools::new(
        krate_bin,
        krate_mcp::mcp_root()?,
        authoring_context::generate,
    );
    let tools = KrateServer { authoring };

    // Say what this is when a person runs it by hand.
    //
    // An MCP server reads JSON-RPC from stdin and writes it to stdout, so with
    // nothing connected it sits there producing nothing. That looks exactly
    // like a hang, and the first person to try it reasonably concluded it was
    // broken. When stdin is a terminal there is no client, so print the
    // explanation; when it is a pipe -- which is every real client -- print
    // nothing, because stray output on the wrong stream corrupts the protocol.
    // This goes to stderr regardless, which clients ignore.
    if std::io::stdin().is_terminal() {
        eprintln!("Krate MCP server: waiting for an AI client on stdin.");
        eprintln!();
        eprintln!("Nothing will happen here. This command is not meant to be run by hand --");
        eprintln!("it is what Claude Desktop or Cursor starts for you in the background.");
        eprintln!();
        eprintln!("To connect it, see https://krate.tech/docs/pages/build-an-app.html");
        eprintln!("To make an app right now, run:  krate create \"your app\" --output app.krate --agent claude");
        eprintln!();
        eprintln!("Press Ctrl-C to stop.");
    }

    krate_mcp::serve_with(&tools, std::io::stdin().lock(), std::io::stdout().lock())
}

/// The whole server: the authoring tools plus the execution tools.
struct KrateServer {
    authoring: KrateTools,
}

impl ToolSet for KrateServer {
    fn server_name(&self) -> &str {
        "krate"
    }

    fn server_version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }

    fn instructions(&self) -> Option<String> {
        // The authoring instructions plus one line on the execution half, so a
        // model knows both halves exist before it starts.
        self.authoring.instructions().map(|text| {
            format!(
                "{text}\n\nThis server can also run components directly: `inspect_bundle` shows \
                 what a .krate asks for without executing it, and `run_component` runs one with \
                 exactly the permissions you name and nothing else."
            )
        })
    }

    fn tools(&self) -> Vec<Value> {
        let mut tools = self.authoring.tools();
        tools.extend(execution_tools());
        tools
    }

    fn call(&self, name: &str, arguments: &Value) -> std::result::Result<Value, String> {
        match name {
            // A run that ends badly is still a successful *call*: the report is
            // the answer, and it carries the exit class and the remedy the
            // model needs. Only a call that could not produce a report at all
            // (a missing file, a bad URL) is an error.
            "run_component" => run_component_tool(arguments).map_err(|err| format!("{err:#}")),
            "inspect_bundle" => inspect_bundle_tool(arguments).map_err(|err| format!("{err:#}")),
            other => self.authoring.call(other, arguments),
        }
    }
}

fn execution_tools() -> Vec<Value> {
    vec![
        json!({
            "name": "run_component",
            "description": "Run a Krate WebAssembly component inside the capability sandbox. \
                Grants are explicit: nothing is prompted, and the component cannot touch \
                files, network, or anything else that was not granted. Returns a \
                krate.run.v1 report with the exit classification, effective grants, \
                denied capabilities, duration, and captured stdout.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "bundle": {
                        "type": "string",
                        "description": "Path to a .krate bundle, or an https URL to one. A bundle carries its own manifest, so component_path and manifest_path are not needed with it. Fetching grants nothing: a downloaded app has the same authority a local one has, which is none until granted."
                    },
                    "insecure_http": {
                        "type": "boolean",
                        "description": "Allow fetching a bundle over plain http. Only for local test servers; https is required otherwise."
                    },
                    "component_path": {
                        "type": "string",
                        "description": "Path to the .wasm component file. Use this or `bundle`."
                    },
                    "manifest_path": {
                        "type": "string",
                        "description": "Path to the app's manifest.toml. Required for auto_grant and denial reporting."
                    },
                    "grants": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Capability strings to grant, e.g. \"fs.read:data/**\"."
                    },
                    "auto_grant": {
                        "type": "boolean",
                        "description": "Grant everything the manifest declares."
                    },
                    "app_args": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Arguments passed to the component."
                    },
                    "sandbox_root": {
                        "type": "string",
                        "description": "Directory that relative filesystem grants resolve against. Defaults to the manifest's directory, else the component's."
                    }
                },
                "required": []
            }
        }),
        json!({
            "name": "inspect_bundle",
            "description": "Read a .krate bundle's identity and the capabilities it requests, without running it. \
                Use this before run_component to decide whether an app should be executed at all, \
                and which of its requests to grant. Nothing is executed and nothing is granted.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "bundle": {
                        "type": "string",
                        "description": "Path to a .krate bundle, or an https URL to one."
                    },
                    "insecure_http": {
                        "type": "boolean",
                        "description": "Allow plain http. Only for local test servers."
                    }
                },
                "required": ["bundle"]
            }
        }),
    ]
}

/// Report what a bundle is and what it wants, without running it.
///
/// This exists so an agent can decide *before* execution. Reading a bundle
/// executes no code and grants nothing.
fn inspect_bundle_tool(arguments: &Value) -> Result<Value> {
    let target = arguments
        .get("bundle")
        .and_then(Value::as_str)
        .context("inspect_bundle needs bundle")?;
    let allow_http = arguments
        .get("insecure_http")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let bundle = if krate_bundle::is_url(target) {
        krate_bundle::fetch(target, allow_http).with_context(|| format!("fetch {target}"))?
    } else {
        krate_bundle::open(Path::new(target)).with_context(|| format!("open {target}"))?
    };

    let manifest = bundle.manifest();
    let requests: Vec<Value> = manifest
        .capabilities
        .iter()
        .map(|request| {
            json!({
                "capability": request.cap,
                "rationale": request.rationale,
                "required": request.required,
            })
        })
        .collect();

    Ok(json!({
        "schema": "krate.inspect.v1",
        "source": target,
        "app": {
            "id": manifest.app.id,
            "name": manifest.app.name,
            "version": manifest.app.version,
            "world": manifest.app.world,
        },
        "requests": requests,
        "note": "Nothing was executed and nothing was granted. Pass the capabilities \
                you decide to allow to run_component in `grants`.",
    }))
}

/// Execute the component and build the krate.run.v1 report.
fn run_component_tool(arguments: &Value) -> Result<Value> {
    // `bundle` is the shareable form: one file, or a URL to one, carrying the
    // component and the permissions it asks for. It is resolved into the same
    // component + manifest pair the sidecar path uses, so everything below is
    // identical either way, and a fetched bundle gets no authority for having
    // been fetched.
    let bundle_target = arguments.get("bundle").and_then(Value::as_str);
    let opened = match bundle_target {
        Some(target) if krate_bundle::is_url(target) => {
            let allow_http = arguments
                .get("insecure_http")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Some(
                krate_bundle::fetch(target, allow_http)
                    .with_context(|| format!("fetch {target}"))?,
            )
        }
        Some(target) => {
            Some(krate_bundle::open(Path::new(target)).with_context(|| format!("open {target}"))?)
        }
        None => None,
    };

    let (component_path, manifest_path) = match &opened {
        Some(bundle) => (
            bundle
                .component_path()
                .to_str()
                .context("bundle path is not utf8")?,
            bundle.manifest_path().to_str(),
        ),
        None => (
            arguments
                .get("component_path")
                .and_then(Value::as_str)
                .context("run_component needs component_path or bundle")?,
            arguments.get("manifest_path").and_then(Value::as_str),
        ),
    };
    let auto_grant = arguments
        .get("auto_grant")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let grants: Vec<String> = string_list(arguments.get("grants"))?;
    let app_args: Vec<String> = string_list(arguments.get("app_args"))?;

    let manifest = match manifest_path {
        Some(path) => {
            let source =
                std::fs::read_to_string(path).with_context(|| format!("read manifest {path}"))?;
            Some(Manifest::parse(&source).context("parse manifest")?)
        }
        None => None,
    };

    let mut policy = SessionPolicy::from_cli_grants(&grants).context("parse grants")?;
    if auto_grant {
        if let Some(manifest) = &manifest {
            let declared = SessionPolicy::allow_all_declared(manifest)
                .context("grant declared capabilities")?;
            policy = SessionPolicy::from_grants(
                policy
                    .grants()
                    .iter()
                    .cloned()
                    .chain(declared.grants().iter().cloned()),
            );
        }
    }

    let app = manifest.as_ref().map(|manifest| {
        json!({
            "id": manifest.app.id,
            "name": manifest.app.name,
            "version": manifest.app.version,
            "world": manifest.app.world,
        })
    });
    let granted: Vec<String> = policy.grants().iter().map(|cap| cap.to_string()).collect();

    // Refuse before running when required capabilities are missing, exactly
    // like the CLI, so agents see the denial as data instead of a trap.
    if let Some(manifest) = &manifest {
        let missing = policy
            .missing_required_for_manifest(manifest)
            .context("check required capabilities")?;
        if !missing.is_empty() {
            let denied: Vec<String> = missing.iter().map(|cap| cap.to_string()).collect();
            return Ok(json!({
                "schema": "krate.run.v1",
                "app": app,
                "capabilities": { "granted": granted, "denied": denied.clone() },
                "exit": {
                    "code": 5,
                    "class": "permission-denied",
                    "message": "missing required capabilities",
                },
                // A refusal an agent cannot act on is just a failure. Name the
                // exact retry so the model does not have to infer it.
                "remedy": {
                    "action": "grant-and-retry",
                    "grants": denied,
                    "note": "Call run_component again with these strings in `grants`. \
                            Each one is narrow: granting it allows only what it names.",
                },
                "duration_ms": Value::Null,
                "stdout": "",
            }));
        }
    }

    let sandbox_root = arguments
        .get("sandbox_root")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .or_else(|| {
            manifest_path
                .map(Path::new)
                .or(Some(Path::new(component_path)))
                .and_then(|path| path.parent())
                .map(Path::to_path_buf)
        })
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from("."));

    let component = std::fs::read(component_path)
        .with_context(|| format!("read component {component_path}"))?;

    let config = Config {
        session_policy: policy,
        app_args,
        sandbox_root,
        ..Config::default()
    };

    let report = match embed::run_component(&component, &config) {
        Ok(outcome) => json!({
            "schema": "krate.run.v1",
            "app": app,
            "capabilities": { "granted": granted, "denied": [] },
            "exit": {
                "code": outcome.exit_code(),
                "class": outcome.exit_class().as_str(),
                "message": Value::Null,
            },
            "duration_ms": outcome.duration().as_millis() as u64,
            "stdout": outcome.stdout_lossy(),
        }),
        Err(err) => {
            let class = match &err {
                krate_runtime::RuntimeError::InvalidComponent(_) => "invalid-component",
                krate_runtime::RuntimeError::Trap(_) => "trap",
                _ => "runtime-error",
            };
            json!({
                "schema": "krate.run.v1",
                "app": app,
                "capabilities": { "granted": granted, "denied": [] },
                "exit": {
                    "code": Value::Null,
                    "class": class,
                    "message": err.to_string(),
                },
                "duration_ms": Value::Null,
                "stdout": "",
            })
        }
    };

    Ok(report)
}

fn string_list(value: Option<&Value>) -> Result<Vec<String>> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_string)
                    .context("expected a string list")
            })
            .collect(),
        Some(_) => anyhow::bail!("expected a string list"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The composed server, pointed at a scratch root so nothing touches the
    /// user's real Krate directory.
    fn server(root: &Path) -> KrateServer {
        KrateServer {
            authoring: KrateTools::new(
                root.join("krate"),
                root.to_path_buf(),
                authoring_context::generate,
            ),
        }
    }

    fn call(server: &KrateServer, line: &str) -> Value {
        krate_mcp::protocol::handle_line(server, line).expect("response expected")
    }

    #[test]
    fn initialize_reports_the_protocol_version_and_the_server_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = server(dir.path());
        let init = call(
            &server,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}"#,
        );
        assert_eq!(init["result"]["serverInfo"]["name"], "krate");
        assert_eq!(
            init["result"]["protocolVersion"],
            krate_mcp::PROTOCOL_VERSION
        );
        // The instructions are the one chance to steer a model before it
        // guesses, so they must actually arrive.
        assert!(init["result"]["instructions"]
            .as_str()
            .expect("instructions")
            .contains("krate_schema"));
    }

    #[test]
    fn both_families_of_tool_are_advertised_together() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = server(dir.path());
        let list = call(&server, r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        let names: Vec<&str> = list["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .map(|tool| tool["name"].as_str().expect("name"))
            .collect();
        // Authoring: build an app by talking.
        assert!(names.contains(&"krate_schema"));
        assert!(names.contains(&"krate_start_build"));
        assert!(names.contains(&"krate_check"));
        // Execution: then run what you built, or inspect someone else's.
        assert!(names.contains(&"run_component"));
        assert!(names.contains(&"inspect_bundle"));
        assert_eq!(names.len(), 9);
    }

    #[test]
    fn notifications_get_no_response() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = server(dir.path());
        assert!(krate_mcp::protocol::handle_line(
            &server,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#
        )
        .is_none());
    }

    #[test]
    fn unknown_methods_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = server(dir.path());
        let response = call(&server, r#"{"jsonrpc":"2.0","id":3,"method":"nope"}"#);
        assert_eq!(response["error"]["code"], -32601);
    }

    #[test]
    fn invalid_component_is_classified_not_crashed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = server(dir.path());
        let wasm = dir.path().join("bogus.wasm");
        std::fs::write(&wasm, b"not a component").expect("write bogus wasm");

        let request = json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "run_component",
                "arguments": { "component_path": wasm.to_string_lossy() },
            },
        });
        let response = call(&server, &request.to_string());
        // Not a crash and not a protocol error: a report the model can read.
        let report: Value = response["result"]["structuredContent"].clone();
        assert_eq!(report["schema"], "krate.run.v1");
        assert_eq!(report["exit"]["class"], "invalid-component");
    }
}

#[cfg(test)]
mod bundle_tool_tests {
    use super::*;

    const MANIFEST: &str = r#"
[app]
id = "com.example.agent"
name = "Agent Demo"
version = "0.1.0"
entry = "code.wasm"
world = "krate:app/cli@0.1.0"

[[capabilities]]
cap = "fs.read:./data/**"
rationale = "Read the input file"
required = true
"#;

    fn packed_bundle(dir: &Path) -> std::path::PathBuf {
        let manifest = dir.join("manifest.toml");
        std::fs::write(&manifest, MANIFEST).expect("write manifest");
        let component = dir.join("code.wasm");
        std::fs::write(&component, b"\0asm\x01\0\0\0").expect("write component");
        let bundle = dir.join("demo.krate");
        krate_bundle::pack(&manifest, &component, &bundle).expect("pack");
        bundle
    }

    #[test]
    fn the_execution_tools_are_valid_mcp_tool_definitions() {
        for tool in execution_tools() {
            let name = tool["name"].as_str().expect("tool name");
            assert!(["run_component", "inspect_bundle"].contains(&name));
            // inputSchema MUST be a JSON Schema object, never null, or a strict
            // client rejects the whole tool list.
            assert_eq!(tool["inputSchema"]["type"], "object", "{name}");
            assert!(tool["description"].as_str().expect("description").len() > 80);
        }
    }

    #[test]
    fn inspect_reports_requests_without_running_anything() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bundle = packed_bundle(dir.path());

        let report = inspect_bundle_tool(&json!({ "bundle": bundle.to_str().expect("utf8") }))
            .expect("inspect");

        assert_eq!(report["app"]["id"], "com.example.agent");
        assert_eq!(report["requests"][0]["capability"], "fs.read:./data/**");
        assert_eq!(report["requests"][0]["rationale"], "Read the input file");
        // The component here is not a runnable module. Inspect still succeeds,
        // which is the point: deciding does not require executing.
        assert_eq!(report["schema"], "krate.inspect.v1");
    }

    #[test]
    fn a_denied_run_carries_the_retry_that_would_succeed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bundle = packed_bundle(dir.path());

        let report =
            run_component_tool(&json!({ "bundle": bundle.to_str().expect("utf8") })).expect("run");

        assert_eq!(report["exit"]["class"], "permission-denied");
        let denied = report["capabilities"]["denied"]
            .as_array()
            .expect("denied array");
        // The remedy must name exactly what was refused, so an agent can
        // re-issue the call without inferring anything.
        assert_eq!(
            &report["remedy"]["grants"],
            &report["capabilities"]["denied"]
        );
        assert_eq!(report["remedy"]["action"], "grant-and-retry");
        assert!(!denied.is_empty());
    }

    #[test]
    fn fetching_a_bundle_over_plain_http_is_refused() {
        let err = run_component_tool(&json!({ "bundle": "http://127.0.0.1:1/app.krate" }))
            .expect_err("plain http must be refused");
        assert!(err.to_string().contains("fetch"));
    }
}
