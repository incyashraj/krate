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

/// The most one app may cost before the loop stops, in US dollars.
///
/// Rounds alone were never a spending limit, only a proxy for one, and a bad
/// proxy: measured on this machine, the same request converged in 7 rounds on
/// Opus and took 92 on Haiku. Priced out on Opus with the pack cached, the 40
/// rounds allowed here run to about $7.72 -- an order of magnitude above the
/// $0.76 a normal build costs. Nothing was watching that.
///
/// So the ceiling is the thing actually being protected: money. It is checked
/// against the token counts the API itself returns, not an estimate, and a
/// build that reaches it stops with a sentence the person can read.
///
/// The number is chosen to be unreachable by honest builds and firm on
/// runaways: roughly four times a typical Opus build, and about the point
/// where a loop is clearly not converging rather than merely having a hard
/// day. `KRATE_BUILD_BUDGET_USD` overrides it.
const DEFAULT_BUDGET_USD: f64 = 3.50;

/// The rate card: what each model charges per million tokens, and since when
/// (IC-636, IC-764).
///
/// Each row is (name fragment, effective-from, input, output, cache write,
/// cache read). The dates are the point: the old table was a bare
/// string-match, and when Anthropic changed Sonnet 5's rates on 2026-09-01
/// the compiled numbers kept pricing calls at the old 2/10 -- every build's
/// spend understated, and the budget ceiling firing late because of it. A
/// dated row can be checked against the vendor's published change; an undated
/// number can only be trusted.
///
/// Order matters: first fragment match wins, so the more specific name goes
/// first.
const RATE_CARD: &[(&str, &str, f64, f64, f64, f64)] = &[
    ("haiku", "2025-10-01", 1.00, 5.00, 1.25, 0.10),
    // Anthropic's 2026-09-01 change: 3/15, five-minute cache writes 3.75,
    // cache reads 0.30. The stale 2/10 row this replaces is the exact
    // understatement IC-636 records.
    ("sonnet-5", "2026-09-01", 3.00, 15.00, 3.75, 0.30),
    ("sonnet", "2025-05-01", 3.00, 15.00, 3.75, 0.30),
    ("gpt-4o", "2024-08-01", 2.50, 10.00, 2.50, 1.25),
    ("opus", "2025-05-01", 5.00, 25.00, 6.25, 0.50),
];

/// A model's rates, and whether they came from its own row.
///
/// An unknown model gets Opus's prices -- the most expensive of the family --
/// because guessing low on a model we do not recognise would quietly raise
/// the ceiling on exactly the runs we understand least. The caller is told it
/// is a guess, so the evidence can say "priced as unknown" rather than
/// presenting the guess as the vendor's rate.
fn prices(model: &str) -> ((f64, f64, f64, f64), bool) {
    for (fragment, _, inp, out, cw, cr) in RATE_CARD {
        if model.contains(fragment) {
            return ((*inp, *out, *cw, *cr), true);
        }
    }
    ((5.00, 25.00, 6.25, 0.50), false)
}

/// What the run has cost so far, in dollars, from the API's own counts.
#[derive(Default)]
struct Spend {
    input: u64,
    output: u64,
    cache_write: u64,
    cache_read: u64,
}

impl Spend {
    /// Add one reply's usage, and say whether there was any to add.
    ///
    /// This used to count a missing field as zero, on the theory that a
    /// billing detail must never stop somebody's app being made. That theory
    /// is the hole IC-764 names: a vendor change that drops or renames the
    /// usage block makes every subsequent call cost $0.00 in our arithmetic,
    /// and the ceiling never fires while real money leaves. A reply that
    /// cannot be priced has to stop the loop BEFORE the next call -- the
    /// caller does that; this reports the fact.
    fn add(&mut self, vendor: ApiVendor, reply: &serde_json::Value) -> bool {
        let usage = &reply["usage"];
        if !usage.is_object() {
            return false;
        }
        let n = |key: &str| usage.get(key).and_then(|v| v.as_u64()).unwrap_or(0);
        match vendor {
            ApiVendor::Anthropic => {
                self.input += n("input_tokens");
                self.output += n("output_tokens");
                self.cache_write += n("cache_creation_input_tokens");
                self.cache_read += n("cache_read_input_tokens");
                // Zero input AND zero output is not a real reply's usage;
                // it is the shape of a renamed field.
                usage.get("input_tokens").is_some() || usage.get("output_tokens").is_some()
            }
            ApiVendor::OpenAi => {
                self.input += n("prompt_tokens");
                self.output += n("completion_tokens");
                usage.get("prompt_tokens").is_some() || usage.get("completion_tokens").is_some()
            }
        }
    }

    fn dollars(&self, model: &str) -> f64 {
        let ((inp, out, cw, cr), _known) = prices(model);
        (self.input as f64 * inp
            + self.output as f64 * out
            + self.cache_write as f64 * cw
            + self.cache_read as f64 * cr)
            / 1_000_000.0
    }

    /// The most the NEXT call could add, in dollars, overstated on purpose.
    ///
    /// The ceiling used to be checked only after a call had been paid for,
    /// which reads honestly for small rounds and stops reading at all when
    /// one round is expensive: a long conversation resent uncached is input
    /// tokens alone worth dollars. Before each call the worst case is
    /// reserved against the budget -- the request's bytes at a deliberately
    /// low 3 bytes per token (fewer bytes per token means MORE tokens, so
    /// this overstates), plus the full output allowance -- and a call whose
    /// worst case cannot fit does not start.
    fn worst_next_call(&self, model: &str, request_bytes: usize, max_tokens: u64) -> f64 {
        let ((inp, out, _cw, _cr), _known) = prices(model);
        let est_input_tokens = (request_bytes as f64) / 3.0;
        (est_input_tokens * inp + max_tokens as f64 * out) / 1_000_000.0
    }
}

fn budget_usd() -> f64 {
    std::env::var("KRATE_BUILD_BUDGET_USD")
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| *v > 0.0)
        .unwrap_or(DEFAULT_BUDGET_USD)
}

/// Cap on what a read_file tool call returns, so one enormous file cannot
/// eat the whole context window.
const MAX_READ_BYTES: usize = 60_000;

/// The model each vendor defaults to, with the date somebody last confirmed
/// it against the live API (IC-762).
///
/// The previous default here was `claude-sonnet-4-20250514`, which Anthropic
/// retired on 2026-06-15 -- so `krate create --agent anthropic` failed with a
/// raw 404 for everyone who had not set the env var, for months, and nothing
/// in this file could say why. A default that names a dated snapshot retires
/// with it; these entries carry a validated-on date so the next person can
/// see how stale the claim is, and the retirement error below says what to do
/// instead of handing back the API's JSON.
///
/// Deliberately the strong coding model rather than the cheap one, per the
/// founder's quality-over-spend call: the loop's cost is dominated by rounds,
/// a weaker model spends more of them (measured: 7 rounds on Opus vs 92 on
/// Haiku for the same app), and the budget ceiling above is the real limit.
const MODEL_DEFAULTS: &[(ApiVendor, &str, &str)] = &[
    (
        ApiVendor::Anthropic,
        "claude-opus-5",
        "validated 2026-09-06",
    ),
    (ApiVendor::OpenAi, "gpt-4o", "validated 2026-09-06"),
];

/// The catalog default for a vendor, before any env override.
fn default_model(vendor: ApiVendor) -> &'static str {
    MODEL_DEFAULTS
        .iter()
        .find(|(v, _, _)| *v == vendor)
        .map(|(_, model, _)| *model)
        .unwrap_or("")
}

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
    default_model(vendor).to_string()
}

/// Is this API error the model itself not existing -- retired, or misspelled?
///
/// A 404 from either vendor's messages endpoint means the model, not the
/// route: the URL is fixed and correct. A 400 whose body names the model is
/// the other spelling of the same fact (Anthropic uses it for some invalid
/// model strings).
fn looks_like_unknown_model(code: u16, detail: &str, model: &str) -> bool {
    code == 404 || (code == 400 && detail.contains(model))
}

/// Did this batch of tool calls end in a state the check has verified?
///
/// Each event is (tool name, was it a passing check_app). The rule is the
/// acceptance-order rule (IC-767): only a check that comes after the final
/// mutation says anything about what is on disk. A passing check followed by
/// a write is a verdict about a tree that no longer exists.
fn batch_ends_verified(events: &[(String, bool)]) -> bool {
    let mut verified = false;
    for (name, passing_check) in events {
        if *passing_check {
            verified = true;
        } else if name == "write_file" {
            verified = false;
        }
    }
    verified
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

    // The lexical check above is not containment on its own (IC-768). The
    // model cannot make a symlink through write_file, but check_app runs a
    // full cargo build in this directory, and anything that build executes
    // can drop one -- after which `src/lib.rs` is a perfectly relative path
    // whose middle or end walks out of the app. So every component that
    // exists is checked for being a link, from the app root down.
    // Strip against whichever spelling of the root the path carries. On
    // macOS the temp dir is `/var/...` while its canonical form is
    // `/private/var/...`; stripping only the canonical form silently skipped
    // this walk for every app under /tmp, and the directory-link escape
    // below went straight through it.
    let mut walk = app_dir.to_path_buf();
    if let Ok(relative) = normalized
        .strip_prefix(&root)
        .or_else(|_| normalized.strip_prefix(app_dir))
    {
        for part in relative.components() {
            walk.push(part.as_os_str());
            match std::fs::symlink_metadata(&walk) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    return Err(format!(
                        "{raw} goes through a symlink, which could point outside the app"
                    ));
                }
                // Nothing at this component: the rest of the path does not
                // exist yet, so there is nothing left that could be a link.
                Err(_) => break,
                Ok(_) => {}
            }
        }
    }
    Ok(normalized)
}

/// Read a file the model asked for, without following a final symlink.
///
/// `resolve_in_app` vets every component that existed when it looked; this
/// closes the gap between that check and the open. On unix the open itself
/// refuses a symlink (O_NOFOLLOW), so a link swapped into place after the
/// check is refused rather than followed.
fn read_in_app(target: &Path) -> std::io::Result<String> {
    #[cfg(unix)]
    {
        use std::io::Read;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(target)?;
        let mut text = String::new();
        file.read_to_string(&mut text)?;
        Ok(text)
    }
    #[cfg(not(unix))]
    {
        std::fs::read_to_string(target)
    }
}

/// Write a file the model asked for: atomically, and never through a link.
///
/// The bytes go to a temporary sibling first and are renamed into place, so
/// an interrupted write leaves the old file or the complete new one, never a
/// torn half (the same discipline pack uses, IC-861). Rename has the second
/// property this needs: if something swapped a symlink into place after the
/// checks, rename replaces the link itself rather than writing through it.
fn write_in_app(target: &Path, contents: &str) -> std::io::Result<()> {
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string());
    let staging = target.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&staging, contents)?;
    match std::fs::rename(&staging, target) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = std::fs::remove_file(&staging);
            Err(err)
        }
    }
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
                    match write_in_app(&target, contents) {
                        Ok(()) => {
                            crate::report_progress_note(&format!("writing {path}"));
                            // The digest goes into the transcript with the
                            // path, so what was written is checkable later
                            // rather than a size someone has to trust.
                            let digest = {
                                use sha2::{Digest, Sha256};
                                let mut hasher = Sha256::new();
                                hasher.update(contents.as_bytes());
                                format!("{:x}", hasher.finalize())
                            };
                            format!(
                                "wrote {path} ({} bytes, sha256:{})",
                                contents.len(),
                                &digest[..16]
                            )
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
                Ok(target) => match read_in_app(&target) {
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
                // The authoring pack, marked cacheable.
                //
                // It is ~21,000 tokens and it is byte-identical on every
                // round of the fix-it loop, so without this it is bought at
                // full price eight or more times to build one app. Written
                // once on the first round and read at a tenth of the price
                // after that, which is close to half the cost of a build --
                // on any model, with nothing given up, because the bytes the
                // model sees do not change.
                //
                // This helps the FIRST person too, which is the part that is
                // easy to get wrong: the saving is within one build, not
                // across builds. Round 1 writes the cache and rounds 2..n
                // read it. Cross-build reuse would need another build inside
                // the five-minute window, which is not the common case and is
                // not what this is for.
                "system": [{
                    "type": "text",
                    "text": system,
                    "cache_control": { "type": "ephemeral" }
                }],
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
            // A model that no longer exists -- retired, or misspelled in the
            // env var -- comes back as a 404 (or a 400 naming the model), and
            // the raw body reads like the request was malformed. This exact
            // failure shipped: the old default model was retired on
            // 2026-06-15 and every run without the env var died here with an
            // unexplained JSON blob (IC-762). Say what happened and what to
            // do. Never substitute a different model silently: which model
            // wrote an app is part of the app's evidence.
            let model = model_for(vendor);
            if looks_like_unknown_model(code, &detail, &model) {
                anyhow::bail!(
                    "{} does not serve the model `{model}` (it may have been \
                     retired). Set {} to a current model and try again, or \
                     update Krate for a newer default.",
                    vendor.label(),
                    match vendor {
                        ApiVendor::Anthropic => "KRATE_ANTHROPIC_MODEL",
                        ApiVendor::OpenAi => "KRATE_OPENAI_MODEL",
                    }
                );
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

    let budget = budget_usd();
    let mut spend = Spend::default();

    for round in 0..MAX_ROUNDS {
        // Reserve the worst the next call could cost BEFORE making it
        // (IC-636). The after-the-call check below is still the accounting of
        // record; this stops a call whose worst case cannot fit the budget
        // from starting at all -- a long conversation resent uncached is
        // dollars of input alone, and "overshoot by one round" stops being an
        // honest reading when one round is that round.
        let request_bytes = serde_json::to_string(&messages)
            .map(|body| body.len())
            .unwrap_or(0)
            + system.len();
        let worst = spend.worst_next_call(&model, request_bytes, 8192);
        let so_far = spend.dollars(&model);
        if so_far + worst > budget {
            anyhow::bail!(
                "the next round could cost up to ${worst:.2} on top of the \
                 ${so_far:.2} already spent, which would pass the ${budget:.2} \
                 ceiling -- stopping before spending it. The request likely \
                 needs to be smaller or clearer.",
            );
        }

        let reply = call_api(vendor, &key, &model, &messages, &system)?;
        if !spend.add(vendor, &reply) {
            // A reply with no usage numbers cannot be priced, and pricing is
            // the only guard on the money. Stop before the next call rather
            // than count it as free (IC-764).
            anyhow::bail!(
                "{} returned a reply without usage counts, so this run can no \
                 longer be priced. Stopping before another call is made; \
                 ${:.2} was spent up to this point.",
                vendor.label(),
                spend.dollars(&model)
            );
        }

        // Checked after the call that was already paid for as well: the
        // ceiling is about not STARTING more work once the build has clearly
        // stopped converging, and the round just spent must be admitted.
        let so_far = spend.dollars(&model);
        if so_far >= budget {
            anyhow::bail!(
                "this app has taken more work than we allow for one build \
                 (${so_far:.2} of a ${budget:.2} ceiling, {n} rounds). It is \
                 usually a sign the request needs to be smaller or clearer. \
                 Nothing further was spent.",
                n = round + 1
            );
        }

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
        let mut events = Vec::new();
        for call in &calls {
            let output = run_tool(&app_path, &krate_bin, call);
            events.push((
                call.name.clone(),
                call.name == "check_app" && output.starts_with("check-app PASSED"),
            ));
            results.push((call.id.clone(), output));
        }

        // A pass only counts if nothing wrote after it (IC-767). The model
        // may batch its tool calls in any order it likes, and a reply of
        // [check_app, write_file] used to run the check, see it pass, run
        // the write -- mutating the tree the check had approved -- and
        // return success on code no check ever saw. Whether that ordering is
        // confusion or intent, the answer is the same: the verdict belongs
        // to the bytes on disk, and a later write takes it back.
        if batch_ends_verified(&events) {
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

    /// A symlink inside the app must not carry a read or a write outside it
    /// (IC-768). The model cannot make a symlink through write_file, but
    /// check_app runs a full cargo build in the app directory, and anything
    /// that build executes can drop one -- after which the lexical check
    /// passes and the escape is one ordinary tool call away.
    #[cfg(unix)]
    #[test]
    fn a_symlink_inside_the_app_cannot_reach_outside_it() {
        let outside = tempfile::tempdir().expect("outside");
        let secret = outside.path().join("secret.txt");
        std::fs::write(&secret, "the host's file").expect("seed");

        let app = tempfile::tempdir().expect("app");
        std::os::unix::fs::symlink(&secret, app.path().join("link.rs")).expect("file link");
        std::os::unix::fs::symlink(outside.path(), app.path().join("dir")).expect("dir link");

        let read = run_tool(
            app.path(),
            "krate-not-used",
            &ToolCall {
                id: "t1".into(),
                name: "read_file".into(),
                input: serde_json::json!({"path": "link.rs"}),
            },
        );
        assert!(
            !read.contains("the host's file"),
            "read_file followed a symlink out of the app: {read}"
        );

        let write = run_tool(
            app.path(),
            "krate-not-used",
            &ToolCall {
                id: "t2".into(),
                name: "write_file".into(),
                input: serde_json::json!({"path": "link.rs", "contents": "overwritten"}),
            },
        );
        assert_eq!(
            std::fs::read_to_string(&secret).expect("outside file"),
            "the host's file",
            "write_file reached through a symlink and changed a file outside the app: {write}"
        );

        // And through a symlinked directory: the final component is an
        // ordinary name, the escape is in the middle.
        let via_dir = run_tool(
            app.path(),
            "krate-not-used",
            &ToolCall {
                id: "t3".into(),
                name: "write_file".into(),
                input: serde_json::json!({"path": "dir/planted.txt", "contents": "outside"}),
            },
        );
        assert!(
            !outside.path().join("planted.txt").exists(),
            "write_file planted a file outside the app through a linked directory: {via_dir}"
        );
    }

    /// A write lands whole or not at all, leaves no staging litter, and the
    /// reply records the digest of what was written -- so the transcript says
    /// what went into the file, not just how big it was (IC-768, test 1628's
    /// shape: interruption can leave the old file or the complete new one,
    /// never a torn half, because the bytes travel via a renamed sibling).
    #[test]
    fn a_write_is_atomic_recorded_and_leaves_no_litter() {
        let app = tempfile::tempdir().expect("app");
        let reply = run_tool(
            app.path(),
            "krate-not-used",
            &ToolCall {
                id: "t1".into(),
                name: "write_file".into(),
                input: serde_json::json!({"path": "src/lib.rs", "contents": "fn main() {}"}),
            },
        );
        assert!(
            reply.contains("sha256:"),
            "the reply must carry a digest: {reply}"
        );
        assert_eq!(
            std::fs::read_to_string(app.path().join("src/lib.rs")).expect("written"),
            "fn main() {}"
        );
        let litter: Vec<_> = std::fs::read_dir(app.path().join("src"))
            .expect("list")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(litter.is_empty(), "staging litter left behind: {litter:?}");
    }

    /// Test 1611's shape: the defaults come from the reviewed catalog, and
    /// every default has its own row in the price table -- an unknown-model
    /// fallback price on the default itself would mean the budget ceiling is
    /// enforced against guessed numbers on every ordinary run.
    #[test]
    fn the_default_models_are_current_and_priced() {
        // The retired default this replaces. If it ever comes back, the exact
        // failure IC-762 records comes back with it: months of raw 404s.
        for (_, model, validated) in MODEL_DEFAULTS {
            assert_ne!(
                *model, "claude-sonnet-4-20250514",
                "the retired default returned"
            );
            assert!(
                validated.starts_with("validated 20"),
                "{model} has no validation date -- nobody can tell how stale it is"
            );
        }
        assert_eq!(default_model(ApiVendor::Anthropic), "claude-opus-5");
        // The default is priced from its own rate-card row, not through the
        // unknown-model fallback: a default whose price is a guess would mean
        // the budget ceiling runs on guessed numbers for every ordinary run.
        let (rates, known) = prices(default_model(ApiVendor::Anthropic));
        assert!(known, "the default model must have its own rate-card row");
        assert_eq!(rates, (5.00, 25.00, 6.25, 0.50));
    }

    /// Test 1612's shape: a simulated retirement is told apart from other API
    /// failures, in both spellings the vendors use, and ordinary errors are
    /// not misread as one.
    #[test]
    fn a_retired_model_is_recognised_not_guessed_at() {
        assert!(looks_like_unknown_model(
            404,
            "not_found_error",
            "claude-opus-5"
        ));
        assert!(looks_like_unknown_model(
            400,
            r#"{"error":{"message":"model: claude-opus-5 is not a valid model"}}"#,
            "claude-opus-5"
        ));
        // A 400 about something else entirely is not a retirement.
        assert!(!looks_like_unknown_model(
            400,
            r#"{"error":{"message":"max_tokens is too large"}}"#,
            "claude-opus-5"
        ));
        assert!(!looks_like_unknown_model(
            500,
            "server error",
            "claude-opus-5"
        ));
    }

    /// Test 1630's shape: a reply that batches a passing check with a later
    /// write cannot conclude the run (IC-767). Whether the ordering is model
    /// confusion or intent, a verdict about a tree that a later write
    /// replaced is not a verdict about the app.
    #[test]
    fn a_pass_followed_by_a_write_does_not_conclude_the_run() {
        let ev = |name: &str, pass: bool| (name.to_string(), pass);

        // The attack/confusion shape: check passes, then a write lands.
        assert!(!batch_ends_verified(&[
            ev("check_app", true),
            ev("write_file", false)
        ]));
        // Write first, then the check: the check saw the final tree.
        assert!(batch_ends_verified(&[
            ev("write_file", false),
            ev("check_app", true)
        ]));
        // A pass with a later read is fine -- reads mutate nothing.
        assert!(batch_ends_verified(&[
            ev("check_app", true),
            ev("read_file", false)
        ]));
        // Pass, write, pass again: the second check re-verified the tree.
        assert!(batch_ends_verified(&[
            ev("check_app", true),
            ev("write_file", false),
            ev("check_app", true),
        ]));
        // A failing check concludes nothing, and neither does no check.
        assert!(!batch_ends_verified(&[ev("check_app", false)]));
        assert!(!batch_ends_verified(&[ev("write_file", false)]));
        assert!(!batch_ends_verified(&[]));
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

    #[test]
    fn spend_is_read_from_the_api_not_guessed() {
        // The four numbers Anthropic returns, including both cache fields --
        // a build that only counted input and output would under-report and
        // the ceiling would sit higher than it says.
        let reply = serde_json::json!({
            "usage": {
                "input_tokens": 1_000,
                "output_tokens": 2_000,
                "cache_creation_input_tokens": 21_000,
                "cache_read_input_tokens": 100_000,
            }
        });
        let mut spend = Spend::default();
        spend.add(ApiVendor::Anthropic, &reply);
        spend.add(ApiVendor::Anthropic, &reply);

        // Opus prices, doubled for two replies:
        //   input  2,000 * $5.00   = $0.010
        //   output 4,000 * $25.00  = $0.100
        //   write 42,000 * $6.25   = $0.2625
        //   read 200,000 * $0.50   = $0.100
        let dollars = spend.dollars("claude-opus-5");
        assert!(
            (dollars - 0.4725).abs() < 1e-6,
            "expected $0.4725, got ${dollars}"
        );

        // A cheaper model costs less for the very same tokens.
        assert!(spend.dollars("claude-haiku-4-5") < dollars);
    }

    #[test]
    fn an_unknown_model_is_priced_as_the_dearest_one() {
        // Guessing low on a model we do not recognise would quietly raise the
        // real ceiling on exactly the runs we understand least -- and the
        // caller is told it was a guess, so evidence never presents the
        // fallback as the vendor's own rate.
        let (unknown_rates, known) = prices("something-new-we-have-not-seen");
        assert!(!known, "an unrecognised model must be reported as a guess");
        let (opus_rates, opus_known) = prices("claude-opus-5");
        assert!(opus_known);
        assert_eq!(unknown_rates, opus_rates);
    }

    /// A reply that cannot be priced must say so, not count as free
    /// (IC-764). The old behaviour treated missing usage as zero, which
    /// meant a vendor renaming the usage block made every later call cost
    /// $0.00 in our arithmetic while real money left -- the ceiling never
    /// fires on numbers that never move.
    #[test]
    fn a_reply_without_usage_is_unpriceable_not_free() {
        let mut spend = Spend::default();
        assert!(
            !spend.add(ApiVendor::Anthropic, &serde_json::json!({})),
            "no usage block at all must be reported unpriceable"
        );
        assert!(
            !spend.add(
                ApiVendor::Anthropic,
                &serde_json::json!({"usage": {"renamed_tokens": 5}})
            ),
            "a usage block with none of the known fields is the renamed-field shape"
        );
        // And a real reply still prices.
        assert!(spend.add(
            ApiVendor::Anthropic,
            &serde_json::json!({"usage": {"input_tokens": 10, "output_tokens": 5}})
        ));
        assert!(spend.dollars("claude-opus-5") > 0.0);
    }

    /// The reservation stops a call whose worst case cannot fit the budget,
    /// and overstates on purpose: a long conversation resent uncached is
    /// dollars of input alone (IC-636).
    #[test]
    fn the_worst_case_of_the_next_call_is_reserved_before_it() {
        let spend = Spend::default();
        // A 3 MB conversation at 3 bytes/token is ~1M input tokens: $5 on
        // Opus input alone, plus the full output allowance.
        let worst = spend.worst_next_call("claude-opus-5", 3_000_000, 8192);
        assert!(
            worst > 5.0,
            "a huge request must reserve past the default ceiling, got ${worst:.2}"
        );
        // A small first round fits comfortably.
        let small = spend.worst_next_call("claude-opus-5", 30_000, 8192);
        assert!(
            small < 1.0,
            "an ordinary round must not be blocked by the reservation, got ${small:.2}"
        );
    }
}
