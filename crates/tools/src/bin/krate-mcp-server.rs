//! Minimal MCP server exposing Krate component execution to agents.
//!
//! One tool, `run_component`: an MCP-capable agent framework supplies a
//! component path, optional manifest, and grants; Krate executes the
//! component inside the capability sandbox through the embedding API and
//! returns a `krate.run.v1`-shaped report (exit class, granted/denied
//! capabilities, duration, captured stdout).
//!
//! Scope is deliberately bounded (Plan/Phase-3-Plan.md P3-EMB-03): no agent
//! orchestration, no model calls, no tool registry. Krate executes
//! artifacts safely; the agent ecosystem does the rest.
//!
//! Transport: newline-delimited JSON-RPC 2.0 over stdio, per the MCP stdio
//! transport. Wire an agent at it with e.g.
//! `claude mcp add krate -- target/debug/krate-mcp-server`.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use krate_manifest::Manifest;
use krate_policy::SessionPolicy;
use krate_runtime::{embed, Config};
use serde_json::{json, Value};

const SERVER_NAME: &str = "krate";
const PROTOCOL_VERSION: &str = "2024-11-05";

fn main() -> Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let line = line.context("read MCP request line")?;
        if line.trim().is_empty() {
            continue;
        }
        let Some(response) = handle_message(&line) else {
            continue;
        };
        writeln!(out, "{response}").context("write MCP response")?;
        out.flush().context("flush MCP response")?;
    }

    Ok(())
}

/// Handle one JSON-RPC message; `None` means no response (notification).
fn handle_message(raw: &str) -> Option<Value> {
    let message: Value = match serde_json::from_str(raw) {
        Ok(message) => message,
        Err(err) => {
            return Some(error_response(
                Value::Null,
                -32700,
                &format!("parse error: {err}"),
            ))
        }
    };

    let method = message.get("method").and_then(Value::as_str)?.to_string();
    let id = message.get("id").cloned();
    let params = message.get("params").cloned().unwrap_or(Value::Null);

    // Notifications carry no id and get no response.
    let id = id?;

    let result = match method.as_str() {
        "initialize" => json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": {
                "name": SERVER_NAME,
                "version": env!("CARGO_PKG_VERSION"),
            },
        }),
        "ping" => json!({}),
        "tools/list" => tools_list(),
        "tools/call" => match tools_call(&params) {
            Ok(result) => result,
            Err(err) => return Some(error_response(id, -32602, &err.to_string())),
        },
        _ => {
            return Some(error_response(
                id,
                -32601,
                &format!("unknown method {method}"),
            ))
        }
    };

    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
}

fn tools_list() -> Value {
    json!({
        "tools": [{
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
        }, {
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
        }]
    })
}

fn tools_call(params: &Value) -> Result<Value> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .context("tools/call needs a name")?;
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

    match name {
        "run_component" => {
            let report = run_component_tool(&arguments)?;
            Ok(json!({
                "content": [{ "type": "text", "text": report.to_string() }],
                "isError": report["exit"]["class"] != "success",
            }))
        }
        "inspect_bundle" => {
            let report = inspect_bundle_tool(&arguments)?;
            Ok(json!({
                "content": [{ "type": "text", "text": report.to_string() }],
                "isError": false,
            }))
        }
        other => anyhow::bail!("unknown tool {other}"),
    }
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
            Some(krate_bundle::fetch(target, allow_http).with_context(|| format!("fetch {target}"))?)
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

    fn call(line: &str) -> Value {
        handle_message(line).expect("response expected")
    }

    #[test]
    fn initialize_and_list_shape() {
        let init = call(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#);
        assert_eq!(init["result"]["serverInfo"]["name"], "krate");

        let list = call(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        assert_eq!(list["result"]["tools"][0]["name"], "run_component");
    }

    #[test]
    fn notifications_get_no_response() {
        assert!(
            handle_message(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none()
        );
    }

    #[test]
    fn unknown_methods_error() {
        let response = call(r#"{"jsonrpc":"2.0","id":3,"method":"nope"}"#);
        assert_eq!(response["error"]["code"], -32601);
    }

    #[test]
    fn invalid_component_is_classified_not_crashed() {
        let dir = std::env::temp_dir().join("krate-mcp-test");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let wasm = dir.join("bogus.wasm");
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
        let response = call(&request.to_string());
        let text = response["result"]["content"][0]["text"]
            .as_str()
            .expect("text content");
        let report: Value = serde_json::from_str(text).expect("parse report");
        assert_eq!(report["schema"], "krate.run.v1");
        assert_eq!(report["exit"]["class"], "invalid-component");
        assert_eq!(response["result"]["isError"], true);
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
    fn both_tools_are_advertised() {
        let list = tools_list();
        let names: Vec<&str> = list["tools"]
            .as_array()
            .expect("tools array")
            .iter()
            .map(|tool| tool["name"].as_str().expect("tool name"))
            .collect();
        assert!(names.contains(&"run_component"));
        assert!(names.contains(&"inspect_bundle"));
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

        let report = run_component_tool(&json!({ "bundle": bundle.to_str().expect("utf8") }))
            .expect("run");

        assert_eq!(report["exit"]["class"], "permission-denied");
        let denied = report["capabilities"]["denied"]
            .as_array()
            .expect("denied array");
        // The remedy must name exactly what was refused, so an agent can
        // re-issue the call without inferring anything.
        assert_eq!(&report["remedy"]["grants"], &report["capabilities"]["denied"]);
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
