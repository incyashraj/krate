//! `KRATE_TRACE`: a timing-and-events spine for a build, for studying the
//! authoring pipeline.
//!
//! The agent transcript already records WHAT the model did -- every tool call,
//! every message. What it does not record is the pipeline's own timing: when a
//! phase began and ended, when each `check-app` ran and what it said, when a
//! repair round fired. This module adds exactly that, and nothing else, so a
//! build writes its own review sheet instead of being reconstructed by hand.
//!
//! It is off unless `KRATE_TRACE` names a file. When on, every event is one
//! JSON object on its own line (JSONL), appended, flushed, best-effort: a
//! failed write never disturbs a build. Timestamps are milliseconds since the
//! process started, so gaps between events read directly as "how long did this
//! take".
//!
//! Read it back with `krate study-report <trace.jsonl>`.

use std::fs::OpenOptions;
use std::io::Write;
use std::sync::OnceLock;
use std::time::Instant;

/// The wall-clock origin for this process, set on first use so every event
/// shares one zero. `Instant` rather than a system clock: monotonic, immune to
/// the wall clock being adjusted mid-build.
fn origin() -> Instant {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    *ORIGIN.get_or_init(Instant::now)
}

/// The trace file path, resolved once. `None` means tracing is off, and every
/// entry point below short-circuits to nothing.
fn path() -> Option<&'static str> {
    static PATH: OnceLock<Option<String>> = OnceLock::new();
    PATH.get_or_init(|| std::env::var("KRATE_TRACE").ok().filter(|p| !p.is_empty()))
        .as_deref()
}

/// Is tracing on for this process? Cheap; call it to guard building a payload.
pub fn enabled() -> bool {
    path().is_some()
}

/// Append one event. `kind` is the event name; `fields` are already-formatted
/// `"key": value` JSON fragments (the caller owns escaping, which for our
/// controlled values is a non-issue -- see `jstr`). Best-effort: any failure is
/// swallowed so tracing can never break a build.
pub fn event(kind: &str, fields: &[(&str, String)]) {
    let Some(path) = path() else { return };
    let ms = origin().elapsed().as_millis();
    let mut line = format!("{{\"t\":{ms},\"kind\":{}", jstr(kind));
    for (k, v) in fields {
        line.push_str(&format!(",{}:{}", jstr(k), v));
    }
    line.push_str("}\n");
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(line.as_bytes());
    }
}

/// A JSON string literal from a Rust string, escaping the characters JSON
/// forbids in a string. Small and dependency-free on purpose: this file must
/// not pull serde into the trace path.
pub fn jstr(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A JSON number field. Convenience so call sites read cleanly.
pub fn num(n: impl std::fmt::Display) -> String {
    n.to_string()
}

// ---- the events the pipeline emits ------------------------------------------
//
// Each is a thin, named wrapper so the call sites read as English and the event
// vocabulary lives in one place. Adding an event is adding a function here.

/// A build began. Names the request, the provider, and the app directory.
pub fn build_start(request: &str, provider: &str, app_dir: &str) {
    event(
        "build.start",
        &[
            ("request", jstr(request)),
            ("provider", jstr(provider)),
            ("app_dir", jstr(app_dir)),
        ],
    );
}

/// A named pipeline phase opened (authoring, building, packing, verifying).
pub fn phase(name: &str) {
    event("phase", &[("name", jstr(name))]);
}

/// One tool call the agent made, as the pipeline sees it: the plain-English
/// step it maps to (writing code, reading the pack, running check-app), plus
/// the raw tool name and any path/command. This is the "what it read / where it
/// thought" spine -- the gap to the previous event is the think/act time.
pub fn tool_call(step: &str, tool: &str, detail: Option<&str>) {
    let mut fields = vec![("step", jstr(step)), ("tool", jstr(tool))];
    if let Some(d) = detail {
        fields.push(("detail", jstr(d)));
    }
    event("tool", &fields);
}

/// A `check-app` run finished, with its verdict. `ok` is the headline; `stage`
/// names where it stopped on failure (build / imports / run / usability), and
/// `detail` carries the one-line reason. The count of these is the iteration
/// count; the stages are where the loop spent its rounds.
pub fn check_app(ok: bool, stage: Option<&str>, detail: Option<&str>) {
    let mut fields = vec![("ok", num(ok))];
    if let Some(s) = stage {
        fields.push(("stage", jstr(s)));
    }
    if let Some(d) = detail {
        fields.push(("detail", jstr(d)));
    }
    event("check_app", &fields);
}

/// Krate's post-agent auto-repair opened a round.
pub fn repair(round: u32, of: u32, because: &str) {
    event(
        "repair",
        &[
            ("round", num(round)),
            ("of", num(of)),
            ("because", jstr(because)),
        ],
    );
}

/// The build ended. `outcome` is one of ok / failed / refused / stalled; the
/// rest is the summary the study row needs.
pub fn build_end(outcome: &str, detail: Option<&str>) {
    let mut fields = vec![("outcome", jstr(outcome))];
    if let Some(d) = detail {
        fields.push(("detail", jstr(d)));
    }
    event("build.end", &fields);
}
