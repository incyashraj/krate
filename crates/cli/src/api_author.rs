//! Authoring against a model API instead of a coding CLI.
//!
//! Every other provider in Krate is a program on PATH that already knows how
//! to edit files and run commands. This one is not: it is an HTTP endpoint
//! that can only produce text. So the agent loop that a CLI gives us for
//! free has to be written here.
//!
//! ## The loop
//!
//! The model gets the same authoring prompt every CLI provider gets, plus a
//! small set of tools it can call: write a file, read a file, and run
//! `krate check-app`. It writes code, asks for the check, reads the errors,
//! and fixes them. That is exactly what the CLI agents do; the difference is
//! that here Krate is the one executing the tool calls.
//!
//! ## What it may touch
//!
//! Every path is resolved inside the app directory and refused if it escapes.
//! The only command that can be run is this binary's own `check-app`, never
//! an arbitrary shell line. A CLI agent is sandboxed by its own permission
//! system; this loop is sandboxed by having no general command tool at all.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::api_key::{self, ApiVendor};

/// How many times the model may act before the loop gives up. A build that
/// has not converged in this many rounds is not one round from converging,
/// and a runaway loop against a paid API is the person's money.
const MAX_ROUNDS: usize = 40;

/// Cap on what a read_file tool call returns, so one enormous file cannot
/// eat the whole context window.
const MAX_READ_BYTES: usize = 60_000;

/// The model each vendor uses. Deliberately the strong coding model rather
/// than the cheap one: the loop's cost is dominated by rounds, and a weaker
/// model spends more of them.
fn model_for(vendor: ApiVendor) -> String {
    let env = match vendor {
        ApiVendor::Anthropic => "KRATE_ANTHROPIC_MODEL",
        ApiVendor::OpenAi => "KRATE_OPENAI_MODEL",
    };
    if let Ok(name) = std::env::var(env) {
        if !name.trim().is_empty() {
            return name.trim().to_string();
        }
    }
    match vendor {
        ApiVendor::Anthropic => "claude-sonnet-4-20250514".to_string(),
        ApiVendor::OpenAi => "gpt-4o".to_string(),
    }
}

/// A tool call the model asked for.
struct ToolCall {
    id: String,
    name: String,
    input: serde_json::Value,
}

/// Resolve a model-supplied path inside the app directory, or refuse it.
///
/// The model is told to use relative paths, but it is a language model and
/// will occasionally produce `../` or an absolute path. Containment is
/// checked here rather than trusted there.
fn resolve_in_app(app_dir: &Path, raw: &str) -> Result<PathBuf, String> {
    let candidate = Path::new(raw);
    if candidate.is_absolute() {
        return Err(format!(
            "{raw} is an absolute path; use a path inside the app"
        ));
    }
    let joined = app_dir.join(candidate);
    // Compare against the app dir after normalising `..` lexically. The file
    // may not exist yet, so canonicalize() is not available for the target.
    let mut normalized = PathBuf::new();
    for part in joined.components() {
        match part {
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    return Err(format!("{raw} points outside the app"));
                }
            }
            std::path::Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    let root = app_dir
        .canonicalize()
        .unwrap_or_else(|_| app_dir.to_path_buf());
    if !normalized.starts_with(&root) && !normalized.starts_with(app_dir) {
        return Err(format!("{raw} points outside the app"));
    }
    Ok(normalized)
}

/// Run one tool call and return what the model should see.
fn run_tool(app_dir: &Path, krate_bin: &str, call: &ToolCall) -> String {
    match call.name.as_str() {
        "write_file" => {
            let path = call.input["path"].as_str().unwrap_or_default();
            let contents = call.input["contents"].as_str().unwrap_or_default();
            match resolve_in_app(app_dir, path) {
                Err(why) => format!("refused: {why}"),
                Ok(target) => {
                    if let Some(parent) = target.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    match std::fs::write(&target, contents) {
                        Ok(()) => {
                            crate::report_progress_note(&format!("writing {path}"));
                            format!("wrote {path} ({} bytes)", contents.len())
                        }
                        Err(err) => format!("could not write {path}: {err}"),
                    }
                }
            }
        }
        "read_file" => {
            let path = call.input["path"].as_str().unwrap_or_default();
            match resolve_in_app(app_dir, path) {
                Err(why) => format!("refused: {why}"),
                Ok(target) => match std::fs::read_to_string(&target) {
                    Ok(mut text) => {
                        if text.len() > MAX_READ_BYTES {
                            text.truncate(MAX_READ_BYTES);
                            text.push_str("\n... (truncated)");
                        }
                        text
                    }
                    Err(err) => format!("could not read {path}: {err}"),
                },
            }
        }
        "check_app" => {
            crate::report_progress_note("checking it builds");
            let out = std::process::Command::new(krate_bin)
                .args(["check-app", "."])
                .current_dir(app_dir)
                .output();
            match out {
                Ok(out) => {
                    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
                    text.push_str(&String::from_utf8_lossy(&out.stderr));
                    // The tail carries the errors; the head is mostly banner.
                    if text.len() > MAX_READ_BYTES {
                        let cut = text.len() - MAX_READ_BYTES;
                        text = format!("... (earlier output trimmed)\n{}", &text[cut..]);
                    }
                    if out.status.success() {
                        format!("check-app PASSED\n{text}")
                    } else {
                        format!("check-app FAILED\n{text}")
                    }
                }
                Err(err) => format!("could not run check-app: {err}"),
            }
        }
        other => format!("unknown tool: {other}"),
    }
}

/// The tools, in the JSON shape each vendor expects.
fn tool_schema(vendor: ApiVendor) -> serde_json::Value {
    let tools = serde_json::json!([
        {
            "name": "write_file",
            "description": "Write a file inside the app directory, creating or replacing it. Use relative paths like src/lib.rs.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Relative path inside the app, e.g. src/lib.rs"},
                    "contents": {"type": "string", "description": "The complete new contents of the file"}
                },
                "required": ["path", "contents"]
            }
        },
        {
            "name": "read_file",
            "description": "Read a file inside the app directory.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Relative path inside the app"}
                },
                "required": ["path"]
            }
        },
        {
            "name": "check_app",
            "description": "Build the app, check it imports only krate:* interfaces, run it, and confirm it paints a frame. This is the oracle: an app is finished when this passes. Call it after every change.",
            "input_schema": {"type": "object", "properties": {}}
        }
    ]);
    match vendor {
        ApiVendor::Anthropic => tools,
        // OpenAI wraps each tool and calls the schema `parameters`.
        ApiVendor::OpenAi => serde_json::Value::Array(
            tools
                .as_array()
                .unwrap()
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t["name"],
                            "description": t["description"],
                            "parameters": t["input_schema"],
                        }
                    })
                })
                .collect(),
        ),
    }
}

/// One HTTP round trip to the vendor.
fn call_api(
    vendor: ApiVendor,
    key: &str,
    model: &str,
    messages: &[serde_json::Value],
    system: &str,
) -> Result<serde_json::Value> {
    let (url, body, auth_header, auth_value) = match vendor {
        ApiVendor::Anthropic => (
            "https://api.anthropic.com/v1/messages",
            serde_json::json!({
                "model": model,
                "max_tokens": 8192,
                "system": system,
                "tools": tool_schema(vendor),
                "messages": messages,
            }),
            "x-api-key",
            key.to_string(),
        ),
        ApiVendor::OpenAi => {
            // OpenAI carries the system prompt as the first message.
            let mut full = vec![serde_json::json!({"role": "system", "content": system})];
            full.extend_from_slice(messages);
            (
                "https://api.openai.com/v1/chat/completions",
                serde_json::json!({
                    "model": model,
                    "tools": tool_schema(vendor),
                    "messages": full,
                }),
                "Authorization",
                format!("Bearer {key}"),
            )
        }
    };

    let mut request = ureq::post(url)
        .set("Content-Type", "application/json")
        .set(auth_header, &auth_value);
    if vendor == ApiVendor::Anthropic {
        request = request.set("anthropic-version", "2023-06-01");
    }

    // ureq is built here without its json feature, so the body is serialized
    // and the reply parsed with serde_json directly.
    let payload = serde_json::to_string(&body).context("could not encode the request")?;
    match request.send_string(&payload) {
        Ok(response) => {
            let text = response
                .into_string()
                .context("could not read the model's reply")?;
            serde_json::from_str::<serde_json::Value>(&text)
                .context("the model's reply was not JSON")
        }
        Err(ureq::Error::Status(code, response)) => {
            let detail = response
                .into_string()
                .unwrap_or_else(|_| "no detail".to_string());
            // 401 is nearly always a bad or expired key, and saying so beats
            // handing back a raw API error body.
            if code == 401 || code == 403 {
                anyhow::bail!(
                    "{} rejected the API key. Check it in Settings, or set {}.",
                    vendor.label(),
                    vendor.env_var()
                );
            }
            if code == 429 {
                anyhow::bail!("{} is rate limiting this key right now.", vendor.label());
            }
            anyhow::bail!("{} returned {code}: {detail}", vendor.label())
        }
        Err(err) => Err(anyhow::anyhow!("could not reach {}: {err}", vendor.label())),
    }
}

/// Pull the assistant's text and tool calls out of a vendor's reply shape.
fn parse_reply(vendor: ApiVendor, reply: &serde_json::Value) -> (String, Vec<ToolCall>, String) {
    let mut text = String::new();
    let mut calls = Vec::new();
    let stop;
    match vendor {
        ApiVendor::Anthropic => {
            stop = reply["stop_reason"].as_str().unwrap_or("").to_string();
            if let Some(blocks) = reply["content"].as_array() {
                for block in blocks {
                    match block["type"].as_str() {
                        Some("text") => text.push_str(block["text"].as_str().unwrap_or("")),
                        Some("tool_use") => calls.push(ToolCall {
                            id: block["id"].as_str().unwrap_or("").to_string(),
                            name: block["name"].as_str().unwrap_or("").to_string(),
                            input: block["input"].clone(),
                        }),
                        _ => {}
                    }
                }
            }
        }
        ApiVendor::OpenAi => {
            let choice = &reply["choices"][0];
            stop = choice["finish_reason"].as_str().unwrap_or("").to_string();
            text.push_str(choice["message"]["content"].as_str().unwrap_or(""));
            if let Some(tool_calls) = choice["message"]["tool_calls"].as_array() {
                for call in tool_calls {
                    // OpenAI sends arguments as a JSON *string*.
                    let raw = call["function"]["arguments"].as_str().unwrap_or("{}");
                    calls.push(ToolCall {
                        id: call["id"].as_str().unwrap_or("").to_string(),
                        name: call["function"]["name"].as_str().unwrap_or("").to_string(),
                        input: serde_json::from_str(raw).unwrap_or(serde_json::json!({})),
                    });
                }
            }
        }
    }
    (text, calls, stop)
}

/// Author an app by talking to a model API.
///
/// Mirrors `run_provider_author`: same app directory, same request, same
/// authoring prompt. Returns the process exit code the caller expects.
pub fn run(vendor: ApiVendor, app_dir: &str, request: &str) -> Result<u8> {
    let (key, source) = api_key::load(vendor).ok_or_else(|| {
        anyhow::anyhow!(
            "no {} API key. Add one in Studio's settings, or set {}.",
            vendor.label(),
            vendor.env_var()
        )
    })?;
    let model = model_for(vendor);
    crate::report_progress_note(&format!("using {} ({})", model, source.describe()));

    let krate_bin = std::env::current_exe()
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "krate".to_string());
    let app_path = PathBuf::from(app_dir);

    // The same authoring prompt every CLI provider gets, so an app built
    // through the API is built to the same instructions as one built through
    // claude or codex. Inlined, because this path has no file-reading agent
    // to go and fetch the pack itself.
    let system = crate::claude_author_prompt_with(app_dir, request, &krate_bin, true);

    let mut messages: Vec<serde_json::Value> = vec![serde_json::json!({
        "role": "user",
        "content": format!(
            "Build this app: {request}\n\nWrite the code with write_file, then call \
             check_app. Keep fixing what check_app reports until it passes. \
             When it passes, reply with the single word DONE."
        )
    })];

    for round in 0..MAX_ROUNDS {
        let reply = call_api(vendor, &key, &model, &messages, &system)?;
        let (text, calls, stop) = parse_reply(vendor, &reply);

        if !text.trim().is_empty() {
            let first = text.trim().lines().next().unwrap_or("").to_string();
            if !first.is_empty() {
                crate::report_progress_note(&first);
            }
        }

        if calls.is_empty() {
            // No tools asked for: the model thinks it is finished. Trust
            // check-app rather than the model's word.
            let verdict = run_tool(
                &app_path,
                &krate_bin,
                &ToolCall {
                    id: String::new(),
                    name: "check_app".to_string(),
                    input: serde_json::json!({}),
                },
            );
            if verdict.starts_with("check-app PASSED") {
                return Ok(0);
            }
            if round + 1 >= MAX_ROUNDS {
                anyhow::bail!("the model stopped before the app passed check-app");
            }
            messages.push(serde_json::json!({"role": "assistant", "content": text}));
            messages.push(serde_json::json!({
                "role": "user",
                "content": format!("Not finished. {verdict}\n\nKeep going."),
            }));
            continue;
        }

        // Record what the model said, in the shape that vendor expects back.
        match vendor {
            ApiVendor::Anthropic => {
                messages.push(serde_json::json!({
                    "role": "assistant",
                    "content": reply["content"].clone(),
                }));
            }
            ApiVendor::OpenAi => {
                messages.push(reply["choices"][0]["message"].clone());
            }
        }

        let mut results = Vec::new();
        let mut passed = false;
        for call in &calls {
            let output = run_tool(&app_path, &krate_bin, call);
            if call.name == "check_app" && output.starts_with("check-app PASSED") {
                passed = true;
            }
            results.push((call.id.clone(), output));
        }

        if passed {
            return Ok(0);
        }

        match vendor {
            ApiVendor::Anthropic => {
                let blocks: Vec<serde_json::Value> = results
                    .iter()
                    .map(|(id, out)| {
                        serde_json::json!({
                            "type": "tool_result",
                            "tool_use_id": id,
                            "content": out,
                        })
                    })
                    .collect();
                messages.push(serde_json::json!({"role": "user", "content": blocks}));
            }
            ApiVendor::OpenAi => {
                for (id, out) in &results {
                    messages.push(serde_json::json!({
                        "role": "tool",
                        "tool_call_id": id,
                        "content": out,
                    }));
                }
            }
        }

        let _ = stop;
        let _ = std::io::stdout().flush();
    }

    anyhow::bail!("the app did not pass check-app within {MAX_ROUNDS} rounds")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The containment check is the whole sandbox for this provider, since
    /// it has no general command tool. A model that asks for `../` or an
    /// absolute path must be refused rather than trusted.
    #[test]
    fn paths_outside_the_app_are_refused() {
        let dir = std::env::temp_dir().join("krate-api-author-test");
        let _ = std::fs::create_dir_all(&dir);
        assert!(resolve_in_app(&dir, "../escape.rs").is_err());
        assert!(resolve_in_app(&dir, "src/../../escape.rs").is_err());
        assert!(resolve_in_app(&dir, "/etc/passwd").is_err());
        assert!(resolve_in_app(&dir, "src/lib.rs").is_ok());
        assert!(resolve_in_app(&dir, "./src/app.rs").is_ok());
    }

    /// Each vendor wants a different tool envelope; sending Anthropic's
    /// shape to OpenAI is a 400 that reads as "the model refused".
    #[test]
    fn each_vendor_gets_its_own_tool_shape() {
        let anthropic = tool_schema(ApiVendor::Anthropic);
        assert_eq!(anthropic[0]["name"], "write_file");
        assert!(anthropic[0]["input_schema"].is_object());

        let openai = tool_schema(ApiVendor::OpenAi);
        assert_eq!(openai[0]["type"], "function");
        assert_eq!(openai[0]["function"]["name"], "write_file");
        assert!(openai[0]["function"]["parameters"].is_object());
    }

    /// An OpenAI tool call carries its arguments as a JSON string, and a
    /// parser that assumes an object silently gets empty input for every
    /// call: the model appears to write empty files.
    #[test]
    fn openai_tool_arguments_arrive_as_a_string_and_are_parsed() {
        let reply = serde_json::json!({
            "choices": [{
                "finish_reason": "tool_calls",
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "function": {
                            "name": "write_file",
                            "arguments": "{\"path\":\"src/lib.rs\",\"contents\":\"fn main() {}\"}"
                        }
                    }]
                }
            }]
        });
        let (_, calls, stop) = parse_reply(ApiVendor::OpenAi, &reply);
        assert_eq!(stop, "tool_calls");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].input["path"], "src/lib.rs");
        assert_eq!(calls[0].input["contents"], "fn main() {}");
    }

    /// Anthropic returns content blocks; text and tool_use are interleaved.
    #[test]
    fn anthropic_blocks_split_into_text_and_calls() {
        let reply = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [
                {"type": "text", "text": "Writing the app now."},
                {"type": "tool_use", "id": "tu_1", "name": "write_file",
                 "input": {"path": "src/lib.rs", "contents": "code"}}
            ]
        });
        let (text, calls, stop) = parse_reply(ApiVendor::Anthropic, &reply);
        assert_eq!(stop, "tool_use");
        assert_eq!(text, "Writing the app now.");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "write_file");
        assert_eq!(calls[0].input["path"], "src/lib.rs");
    }
}
