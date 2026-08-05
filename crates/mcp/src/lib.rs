//! `krate mcp` -- a Model Context Protocol server for authoring Krate apps.
//!
//! Someone adds this once to Claude Desktop or Cursor and then just talks:
//! "build me a habit tracker and package it as a .krate". The model calls these
//! tools, iterates against the oracle, and hands back a working file. No
//! commands typed.
//!
//! Three facts shape the whole design, and they all come from what a `.krate`
//! actually is -- a compiled WebAssembly component built from hand-written
//! `#![no_std]` Rust:
//!
//! 1. There is no app schema to fill in, so there is no `create_app` returning
//!    a structure. These tools wrap the authoring loop that already exists.
//! 2. A build takes two to five minutes, so the build tools are async-shaped:
//!    start a job, get an id, poll it. A tool call that blocked that long would
//!    hit the client's timeout and be cancelled mid-build.
//! 3. The oracle is the valuable part. Anyone can prompt a model to write Rust;
//!    `krate check-app` is what turns "it wrote something" into "this builds,
//!    imports zero OS calls, runs, and paints a frame". That is `krate_check`.
//!
//! Builds run on the user's machine, never ours. Compiling model-written Rust
//! is executing model-written Rust, and a hosted build service would be an
//! endpoint whose advertised feature is running strangers' code -- which
//! contradicts the whole point of Krate.
//!
//! Transport is newline-delimited JSON-RPC 2.0 over stdio, per the MCP stdio
//! transport. See `docs/mcp-setup.md` for the client configuration.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub mod examples;
pub mod jobs;
pub mod protocol;
pub mod tools;

pub use protocol::{ToolSet, PROTOCOL_VERSION};
pub use tools::KrateTools;

/// Serve the MCP protocol on stdin/stdout until the client closes stdin.
///
/// `krate_bin` is the binary that does the real work (builds, checks, renders)
/// -- normally this same executable. `schema` generates the authoring pack;
/// it is injected so this crate does not have to depend on the CLI.
pub fn serve(krate_bin: PathBuf, schema: fn(&Path) -> String) -> Result<()> {
    let root = mcp_root()?;
    let tools = KrateTools::new(krate_bin, root, schema);
    serve_with(&tools, std::io::stdin().lock(), std::io::stdout().lock())
}

/// The serve loop, over any reader and writer, so it can be driven by a test.
pub fn serve_with(tools: &dyn ToolSet, input: impl BufRead, mut output: impl Write) -> Result<()> {
    for line in input.lines() {
        let line = line.context("read a message from the client")?;
        if line.trim().is_empty() {
            continue;
        }
        let Some(response) = protocol::handle_line(tools, &line) else {
            // A notification. Writing anything here would corrupt the stream.
            continue;
        };
        // to_string, never to_string_pretty: the stdio transport requires one
        // message per line with no embedded newline.
        let encoded = serde_json::to_string(&response).context("encode a response")?;
        writeln!(output, "{encoded}").context("write a response")?;
        output.flush().context("flush a response")?;
    }
    Ok(())
}

/// Where this server keeps builds, scratch checks, and rendered frames.
///
/// Under the user's own Krate directory rather than a temp dir, because a build
/// that took four minutes should still be there when they go looking for the
/// file, and because `/tmp` is cleaned out from under long-running work on some
/// systems.
pub fn mcp_root() -> Result<PathBuf> {
    let base = std::env::var_os("KRATE_MCP_ROOT")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("KRATE_HOME")
                .map(PathBuf::from)
                .map(|home| home.join("mcp"))
        })
        .or_else(|| home_dir().map(|home| home.join(".krate").join("mcp")))
        .unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&base)
        .with_context(|| format!("create the MCP working directory {}", base.display()))?;
    Ok(base)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    struct Fake;

    impl ToolSet for Fake {
        fn server_name(&self) -> &str {
            "fake"
        }
        fn server_version(&self) -> &str {
            "1.0.0"
        }
        fn tools(&self) -> Vec<Value> {
            vec![
                json!({"name":"noop","description":"does nothing","inputSchema":{"type":"object"}}),
            ]
        }
        fn call(&self, _name: &str, _arguments: &Value) -> Result<Value, String> {
            Ok(json!({ "ok": true }))
        }
    }

    /// Drive the loop the way a real client does: a stream of newline-delimited
    /// messages in, a stream of newline-delimited messages out.
    fn drive(input: &str) -> Vec<Value> {
        let mut output = Vec::new();
        serve_with(&Fake, std::io::Cursor::new(input), &mut output).expect("serve");
        String::from_utf8(output)
            .expect("utf8")
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("each output line is one json message"))
            .collect()
    }

    #[test]
    fn a_full_session_produces_one_response_per_request_and_none_per_notification() {
        let responses = drive(concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}"#,
            "\n",
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"noop"}}"#,
            "\n",
        ));

        // Three requests, one notification, so exactly three responses.
        assert_eq!(responses.len(), 3);
        assert_eq!(responses[0]["id"], 1);
        assert_eq!(responses[0]["result"]["protocolVersion"], "2025-11-25");
        assert_eq!(responses[1]["id"], 2);
        assert_eq!(responses[1]["result"]["tools"][0]["name"], "noop");
        assert_eq!(responses[2]["id"], 3);
        assert_eq!(responses[2]["result"]["isError"], false);
        for response in &responses {
            assert_eq!(response["jsonrpc"], "2.0");
        }
    }

    #[test]
    fn blank_lines_between_messages_are_ignored_not_answered() {
        let responses = drive("\n\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n\n");
        assert_eq!(responses.len(), 1);
    }

    #[test]
    fn closing_stdin_ends_the_loop_cleanly() {
        // The spec's shutdown for stdio: the client closes our input and waits
        // for us to exit. Hanging here is what makes a client resort to SIGKILL.
        assert!(drive("").is_empty());
    }
}
