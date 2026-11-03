//! The seam where running programs plugs in (IC-977).
//!
//! Everything here is what the editor needs from "run a program on this
//! computer" and nothing more: start one in the project folder, read what it
//! prints as it prints it, write to it, wait for it, stop it. [`Runner`] is
//! that contract. Today the only runner is [`NoRunner`], which answers every
//! call with [`NEEDS_PROCESS`]; when `krate:process` lands, a runner built on
//! it replaces it in `Workspace::runner` and every action below starts
//! working with no other change.
//!
//! What the actions run is decided here too ([`Tool::program`],
//! [`Tool::args`]), and what comes back is read here: [`parse_cargo`] turns
//! cargo's and rustc's output into [`Diagnostic`]s for the Problems panel, so
//! jumping to an error is already wired and tested.

use alloc::string::String;
use alloc::vec::Vec;

/// What the person sees while a program cannot be run yet.
pub const NEEDS_PROCESS: &str = "needs the run-programs ability (coming)";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Build,
    Run,
    Test,
    Check,
    Clippy,
    Format,
    /// rust-analyzer: completion, hover, go to definition.
    Complete,
    Hover,
    Definition,
    /// A debug adapter (lldb-dap / CodeLLDB).
    Debug,
    /// The person's own shell in a pseudo-terminal.
    Terminal,
}

impl Tool {
    pub const ALL: [Tool; 11] = [
        Tool::Build,
        Tool::Run,
        Tool::Test,
        Tool::Check,
        Tool::Clippy,
        Tool::Format,
        Tool::Complete,
        Tool::Hover,
        Tool::Definition,
        Tool::Debug,
        Tool::Terminal,
    ];

    pub const fn title(self) -> &'static str {
        match self {
            Tool::Build => "Build (cargo build)",
            Tool::Run => "Run (cargo run)",
            Tool::Test => "Test (cargo test)",
            Tool::Check => "Check (cargo check)",
            Tool::Clippy => "Lint (cargo clippy)",
            Tool::Format => "Format file (rustfmt)",
            Tool::Complete => "Complete name (rust-analyzer)",
            Tool::Hover => "Show type and docs (rust-analyzer)",
            Tool::Definition => "Go to definition across files (rust-analyzer)",
            Tool::Debug => "Start debugging",
            Tool::Terminal => "New terminal",
        }
    }

    /// The program the manifest will name (`process.run:<program>`).
    pub fn program(self) -> &'static str {
        match self {
            Tool::Build | Tool::Run | Tool::Test | Tool::Check | Tool::Clippy => "cargo",
            Tool::Format => "rustfmt",
            Tool::Complete | Tool::Hover | Tool::Definition => "rust-analyzer",
            Tool::Debug => "lldb-dap",
            Tool::Terminal => "shell",
        }
    }

    /// The arguments, with `file` the project-relative file in focus.
    pub fn args(self, file: Option<&str>) -> Vec<String> {
        let words: &[&str] = match self {
            Tool::Build => &["build", "--message-format=short"],
            Tool::Run => &["run"],
            Tool::Test => &["test"],
            Tool::Check => &["check", "--message-format=short"],
            Tool::Clippy => &["clippy", "--message-format=short"],
            Tool::Format => &["--edition", "2021"],
            _ => &[],
        };
        let mut v: Vec<String> = words.iter().map(|w| String::from(*w)).collect();
        if self == Tool::Format {
            if let Some(f) = file {
                v.push(String::from(f));
            }
        }
        v
    }

    /// Whether its output feeds the Problems panel.
    pub fn reports_problems(self) -> bool {
        matches!(self, Tool::Build | Tool::Check | Tool::Clippy | Tool::Test)
    }
}

/// Something a running program did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobEvent {
    Output { job: u64, text: String, stderr: bool },
    Exited { job: u64, code: i32 },
}

/// Running programs, as the editor needs it. Paths never appear: the
/// program runs in the project folder the person picked, which the host
/// resolves from the token the editor holds.
pub trait Runner {
    /// Whether this runner can start `program` at all on this computer.
    fn available(&self, program: &str) -> Result<(), String>;
    /// Start `program` with `args` in the project folder.
    fn start(&mut self, program: &str, args: &[String]) -> Result<u64, String>;
    /// What happened since the last call (non-blocking).
    fn poll(&mut self) -> Vec<JobEvent>;
    fn write(&mut self, job: u64, bytes: &[u8]) -> Result<(), String>;
    fn kill(&mut self, job: u64);
}

/// The runner while `krate:process` does not exist yet.
#[derive(Debug, Default)]
pub struct NoRunner;

impl Runner for NoRunner {
    fn available(&self, _program: &str) -> Result<(), String> {
        Err(String::from(NEEDS_PROCESS))
    }
    fn start(&mut self, _program: &str, _args: &[String]) -> Result<u64, String> {
        Err(String::from(NEEDS_PROCESS))
    }
    fn poll(&mut self) -> Vec<JobEvent> {
        Vec::new()
    }
    fn write(&mut self, _job: u64, _bytes: &[u8]) -> Result<(), String> {
        Err(String::from(NEEDS_PROCESS))
    }
    fn kill(&mut self, _job: u64) {}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Note,
}

/// One problem a build reported, pinned to a place in a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub file: String,
    /// Zero-based, ready for the editor.
    pub line: usize,
    pub col: usize,
    pub severity: Severity,
    pub message: String,
    /// `E0308` and the like.
    pub code: Option<String>,
}

fn severity_of(word: &str) -> Option<Severity> {
    match word {
        "error" => Some(Severity::Error),
        "warning" => Some(Severity::Warning),
        "note" | "help" => Some(Severity::Note),
        _ => None,
    }
}

/// `error[E0308]: mismatched types` -> (Error, Some("E0308"), "mismatched types")
fn header(line: &str) -> Option<(Severity, Option<String>, String)> {
    let (head, msg) = line.split_once(": ")?;
    let (word, code) = match head.split_once('[') {
        Some((w, rest)) => (w, rest.strip_suffix(']').map(String::from)),
        None => (head, None),
    };
    Some((severity_of(word.trim())?, code, String::from(msg.trim())))
}

/// `src/main.rs:4:18` -> ("src/main.rs", 3, 17)
fn location(s: &str) -> Option<(String, usize, usize)> {
    let s = s.trim();
    let mut parts = s.rsplitn(3, ':');
    let col: usize = parts.next()?.trim().parse().ok()?;
    let line: usize = parts.next()?.trim().parse().ok()?;
    let file = parts.next()?;
    if file.is_empty() {
        return None;
    }
    Some((String::from(file), line.saturating_sub(1), col.saturating_sub(1)))
}

/// Read cargo/rustc output -- the long form (`error: ...` then `--> file:l:c`)
/// and `--message-format=short` (`file:l:c: error: ...`) alike.
pub fn parse_cargo(output: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let mut open: Option<(Severity, Option<String>, String)> = None;
    for raw in output.lines() {
        let line = raw.trim_end();
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("--> ") {
            if let (Some((sev, code, msg)), Some((file, l, c))) = (open.take(), location(rest)) {
                out.push(Diagnostic { file, line: l, col: c, severity: sev, message: msg, code });
            }
            continue;
        }
        if !line.starts_with(' ') {
            if let Some(h) = header(line) {
                // A summary like "error: could not compile" has no place.
                open = if h.2.starts_with("could not compile") || h.2.contains("generated ") { None } else { Some(h) };
                continue;
            }
            // Short form: `src/main.rs:4:18: error[E0308]: mismatched types`
            let mut pieces = line.splitn(4, ':');
            if let (Some(f), Some(l), Some(c), Some(rest)) = (pieces.next(), pieces.next(), pieces.next(), pieces.next()) {
                if let (Ok(l), Ok(c)) = (l.trim().parse::<usize>(), c.trim().parse::<usize>()) {
                    if let Some((sev, code, msg)) = header(rest.trim_start()) {
                        out.push(Diagnostic {
                            file: String::from(f),
                            line: l.saturating_sub(1),
                            col: c.saturating_sub(1),
                            severity: sev,
                            message: msg,
                            code,
                        });
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_form_errors_and_warnings() {
        let text = "   Compiling tour v0.1.0\nwarning: unused variable: `x`\n --> src/main.rs:3:9\n  |\n3 |     let x = 1;\n  |         ^\n\nerror[E0308]: mismatched types\n  --> src/shapes.rs:12:18\n   |\nerror: could not compile `tour` (bin \"tour\") due to 1 previous error\n";
        let d = parse_cargo(text);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].severity, Severity::Warning);
        assert_eq!((d[0].file.as_str(), d[0].line, d[0].col), ("src/main.rs", 2, 8));
        assert_eq!(d[1].code.as_deref(), Some("E0308"));
        assert_eq!(d[1].message, "mismatched types");
        assert_eq!((d[1].file.as_str(), d[1].line, d[1].col), ("src/shapes.rs", 11, 17));
    }

    #[test]
    fn short_form() {
        let d = parse_cargo("src/main.rs:4:18: error[E0425]: cannot find value `y` in this scope\nsrc/lib.rs:1:1: warning: missing docs\n");
        assert_eq!(d.len(), 2);
        assert_eq!((d[0].line, d[0].col, d[0].severity), (3, 17, Severity::Error));
        assert_eq!(d[1].severity, Severity::Warning);
    }

    #[test]
    fn every_action_names_its_program_and_has_no_runner_yet() {
        let mut r = NoRunner;
        for t in Tool::ALL {
            assert!(!t.title().is_empty());
            assert!(!t.program().is_empty());
            assert_eq!(r.start(t.program(), &t.args(Some("src/main.rs"))), Err(String::from(NEEDS_PROCESS)));
        }
        assert_eq!(Tool::Format.args(Some("src/main.rs")).last().map(String::as_str), Some("src/main.rs"));
        assert!(r.poll().is_empty());
    }
}
