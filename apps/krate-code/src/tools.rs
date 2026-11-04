//! Running programs (IC-977): the contract, the actions, and reading what
//! cargo says.
//!
//! [`Runner`] is everything the editor needs from "run a program on this
//! computer": start one in the project folder, open a terminal, read what it
//! prints as it prints it, write to it, resize it, stop it. The guest's
//! runner is built on `krate:process`; [`NoRunner`] stands in where there is
//! none (the tour's in-memory folder, a unit test), and the tests carry a
//! scripted one.
//!
//! What each action runs is decided here ([`Tool::program`], [`Tool::args`]),
//! and what comes back is read here: [`CargoReader`] turns cargo's JSON
//! messages (and, for older output, [`parse_cargo`] its text) into
//! [`Diagnostic`]s for the Problems panel and text for the Output panel.

use crate::json::{self, Value};
use alloc::string::String;
use alloc::vec::Vec;

/// What the person sees when the open folder cannot run programs.
pub const NEEDS_FOLDER: &str =
    "the tour lives in memory, so there is nothing on disk to build -- open a folder (Ctrl+O) or make a new project";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Build,
    Run,
    Test,
    Check,
    Clippy,
    Format,
    /// rust-analyzer: completion, hover, go to definition, references,
    /// rename, code actions.
    Complete,
    Hover,
    Definition,
    References,
    RenameSymbol,
    CodeAction,
    /// A debug adapter (lldb-dap).
    Debug,
    Continue,
    StepOver,
    StepIn,
    StepOut,
    StopDebug,
    /// The person's own shell in a pseudo-terminal.
    Terminal,
    /// Stop the build or run that is going.
    Stop,
}

impl Tool {
    pub const ALL: [Tool; 20] = [
        Tool::Build,
        Tool::Run,
        Tool::Test,
        Tool::Check,
        Tool::Clippy,
        Tool::Format,
        Tool::Complete,
        Tool::Hover,
        Tool::Definition,
        Tool::References,
        Tool::RenameSymbol,
        Tool::CodeAction,
        Tool::Debug,
        Tool::Continue,
        Tool::StepOver,
        Tool::StepIn,
        Tool::StepOut,
        Tool::StopDebug,
        Tool::Terminal,
        Tool::Stop,
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
            Tool::Definition => "Go to definition (rust-analyzer)",
            Tool::References => "Find references (rust-analyzer)",
            Tool::RenameSymbol => "Rename symbol (rust-analyzer)",
            Tool::CodeAction => "Quick fixes and code actions (rust-analyzer)",
            Tool::Debug => "Start debugging (lldb-dap)",
            Tool::Continue => "Debug: continue",
            Tool::StepOver => "Debug: step over",
            Tool::StepIn => "Debug: step into",
            Tool::StepOut => "Debug: step out",
            Tool::StopDebug => "Debug: stop",
            Tool::Terminal => "New terminal",
            Tool::Stop => "Stop the running build or program",
        }
    }

    /// The program the manifest names (`process.run:<program>`).
    pub fn program(self) -> &'static str {
        match self {
            Tool::Build | Tool::Run | Tool::Test | Tool::Check | Tool::Clippy | Tool::Stop => {
                "cargo"
            }
            Tool::Format => "rustfmt",
            Tool::Complete
            | Tool::Hover
            | Tool::Definition
            | Tool::References
            | Tool::RenameSymbol
            | Tool::CodeAction => "rust-analyzer",
            Tool::Debug
            | Tool::Continue
            | Tool::StepOver
            | Tool::StepIn
            | Tool::StepOut
            | Tool::StopDebug => "lldb-dap",
            Tool::Terminal => "shell",
        }
    }

    /// The arguments a cargo action runs with. JSON messages carry each
    /// problem's place exactly and the text cargo would have printed.
    pub fn args(self) -> Vec<String> {
        let verb = match self {
            Tool::Build | Tool::Debug => "build",
            Tool::Run => "run",
            Tool::Test => "test",
            Tool::Check => "check",
            Tool::Clippy => "clippy",
            _ => return Vec::new(),
        };
        [
            verb,
            "--message-format=json-diagnostic-rendered-ansi",
            "--color=always",
        ]
        .iter()
        .map(|w| String::from(*w))
        .collect()
    }

    /// Whether this is a cargo job whose output feeds the Problems panel.
    pub fn is_cargo(self) -> bool {
        matches!(
            self,
            Tool::Build | Tool::Run | Tool::Test | Tool::Check | Tool::Clippy
        )
    }

    /// The word for the running indicator.
    pub fn doing(self) -> &'static str {
        match self {
            Tool::Build | Tool::Debug => "Building",
            Tool::Run => "Running",
            Tool::Test => "Testing",
            Tool::Check => "Checking",
            Tool::Clippy => "Linting",
            _ => "Working",
        }
    }
}

/// Something a running program did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobEvent {
    Output {
        job: u64,
        bytes: Vec<u8>,
        stderr: bool,
    },
    /// The program ended and every byte it wrote has been handed over.
    Exited {
        job: u64,
        code: Option<i32>,
        signal: Option<i32>,
    },
}

/// Why a program could not start, in words for the person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    /// It is not installed on this computer.
    Missing(String),
    /// The person (or the organisation) did not allow it.
    Refused(String),
    /// No folder to run in (the tour), or anything else.
    Other(String),
}

impl RunError {
    pub fn words(&self) -> &str {
        match self {
            RunError::Missing(w) | RunError::Refused(w) | RunError::Other(w) => w,
        }
    }
}

/// What a program that ran to the end left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finished {
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Running programs, as the editor needs it. Paths never appear: programs
/// run in the project folder, which the host resolves from the name the
/// editor holds (`picked/<token>`).
pub trait Runner {
    /// The folder programs run in, set when a project opens; `None` when the
    /// project has no folder on disk (the tour).
    fn set_folder(&mut self, folder: Option<String>);
    fn folder(&self) -> Option<&str>;
    /// Start `program` with `args` in the project folder (or `sub` inside
    /// it, when given).
    fn start(&mut self, program: &str, args: &[String], sub: Option<&str>)
        -> Result<u64, RunError>;
    /// The person's own shell in a terminal of `cols` by `rows`.
    fn terminal(&mut self, cols: u16, rows: u16) -> Result<u64, RunError>;
    /// What happened since the last call. Never waits.
    fn poll(&mut self) -> Vec<JobEvent>;
    fn write(&mut self, job: u64, bytes: &[u8]) -> Result<(), String>;
    fn close_stdin(&mut self, job: u64);
    fn resize(&mut self, job: u64, cols: u16, rows: u16);
    /// End it and everything it started.
    fn kill(&mut self, job: u64);
    /// Forget it (killing it if it still runs).
    fn release(&mut self, job: u64);
    /// Run a short program to the end with `input` on its stdin, waiting at
    /// most `timeout_ms`: the formatter, which a save waits for.
    fn run_to_end(
        &mut self,
        program: &str,
        args: &[String],
        input: &[u8],
        timeout_ms: u32,
    ) -> Result<Finished, RunError>;
}

/// The runner where nothing can run.
#[derive(Debug, Default)]
pub struct NoRunner;

impl Runner for NoRunner {
    fn set_folder(&mut self, _folder: Option<String>) {}
    fn folder(&self) -> Option<&str> {
        None
    }
    fn start(&mut self, _p: &str, _a: &[String], _s: Option<&str>) -> Result<u64, RunError> {
        Err(RunError::Other(String::from(NEEDS_FOLDER)))
    }
    fn terminal(&mut self, _cols: u16, _rows: u16) -> Result<u64, RunError> {
        Err(RunError::Other(String::from(NEEDS_FOLDER)))
    }
    fn poll(&mut self) -> Vec<JobEvent> {
        Vec::new()
    }
    fn write(&mut self, _job: u64, _bytes: &[u8]) -> Result<(), String> {
        Err(String::from(NEEDS_FOLDER))
    }
    fn close_stdin(&mut self, _job: u64) {}
    fn resize(&mut self, _job: u64, _cols: u16, _rows: u16) {}
    fn kill(&mut self, _job: u64) {}
    fn release(&mut self, _job: u64) {}
    fn run_to_end(
        &mut self,
        _p: &str,
        _a: &[String],
        _i: &[u8],
        _t: u32,
    ) -> Result<Finished, RunError> {
        Err(RunError::Other(String::from(NEEDS_FOLDER)))
    }
}

/// The plain words for a program that is not installed, with how to get it.
pub fn install_hint(program: &str) -> &'static str {
    match program {
        "cargo" | "rustc" => "Install Rust from rustup.rs, then try again",
        // From the project's own folder -- the terminal panel opens there --
        // because rustup adds a component to the Rust of the folder it runs
        // in: run elsewhere, it lands in another toolchain and this project
        // still has none (found 2026-11-04, installed into 1.94.1 while the
        // project used 1.91.1).
        "rustfmt" => "Add it in the terminal panel (Ctrl+`), which opens in this project: rustup component add rustfmt",
        "rust-analyzer" => "Add it in the terminal panel (Ctrl+`), which opens in this project: rustup component add rust-analyzer",
        "lldb-dap" | "xcrun" => {
            "Install a debug adapter: Xcode's command line tools on a Mac (xcode-select --install), the lldb package on Linux (it carries lldb-dap), or LLVM on Windows"
        }
        _ => "Install it, then try again",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Note,
}

/// Where a problem came from: the last build, or the language server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Build,
    Lsp,
}

/// One problem, pinned to a place in a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub file: String,
    /// Zero-based line and character (not byte) column.
    pub line: usize,
    pub col: usize,
    pub end_line: usize,
    pub end_col: usize,
    pub severity: Severity,
    pub message: String,
    /// `E0308` and the like.
    pub code: Option<String>,
    pub source: Source,
}

impl Diagnostic {
    pub fn at(file: &str, line: usize, col: usize, severity: Severity, message: &str) -> Self {
        Diagnostic {
            file: String::from(file),
            line,
            col,
            end_line: line,
            end_col: col + 1,
            severity,
            message: String::from(message),
            code: None,
            source: Source::Build,
        }
    }
}

fn severity_of(word: &str) -> Option<Severity> {
    match word {
        "error" | "error: internal compiler error" => Some(Severity::Error),
        "warning" => Some(Severity::Warning),
        "note" | "help" | "failure-note" => Some(Severity::Note),
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
    Some((
        String::from(file),
        line.saturating_sub(1),
        col.saturating_sub(1),
    ))
}

/// Read cargo/rustc text output -- the long form (`error: ...` then
/// `--> file:l:c`) and `--message-format=short` (`file:l:c: error: ...`).
pub fn parse_cargo(output: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let mut open: Option<(Severity, Option<String>, String)> = None;
    let plain = strip_ansi(output);
    for raw in plain.lines() {
        let line = raw.trim_end();
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("--> ") {
            if let (Some((sev, code, msg)), Some((file, l, c))) = (open.take(), location(rest)) {
                let mut d = Diagnostic::at(&file, l, c, sev, &msg);
                d.code = code;
                out.push(d);
            }
            continue;
        }
        if !line.starts_with(' ') {
            if let Some(h) = header(line) {
                // A summary like "error: could not compile" has no place.
                open = if h.2.starts_with("could not compile") || h.2.contains("generated ") {
                    None
                } else {
                    Some(h)
                };
                continue;
            }
            let mut pieces = line.splitn(4, ':');
            if let (Some(f), Some(l), Some(c), Some(rest)) =
                (pieces.next(), pieces.next(), pieces.next(), pieces.next())
            {
                if let (Ok(l), Ok(c)) = (l.trim().parse::<usize>(), c.trim().parse::<usize>()) {
                    if let Some((sev, code, msg)) = header(rest.trim_start()) {
                        let mut d =
                            Diagnostic::at(f, l.saturating_sub(1), c.saturating_sub(1), sev, &msg);
                        d.code = code;
                        out.push(d);
                    }
                }
            }
        }
    }
    out
}

/// Text without its terminal colour codes.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\u{1b}' {
            if it.peek() == Some(&'[') {
                it.next();
                for d in it.by_ref() {
                    if ('@'..='~').contains(&d) {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// One of cargo's JSON messages, as the editor uses it.
#[derive(Debug, Clone, PartialEq)]
pub enum CargoLine {
    /// A compiler message: its text as cargo would print it (colours and
    /// all), and its place when it has one.
    Message {
        rendered: String,
        diagnostic: Option<Diagnostic>,
    },
    /// A binary was built, at this path on the computer (for the debugger).
    Executable(String),
    BuildFinished {
        success: bool,
    },
    /// Not JSON: a program's own output (`cargo run`, `cargo test`).
    Text(String),
    /// JSON cargo sends that the editor has no use for.
    Other,
}

/// Read one line of `--message-format=json` output.
pub fn read_cargo_line(line: &str) -> CargoLine {
    let t = line.trim();
    if !t.starts_with('{') {
        return CargoLine::Text(String::from(line));
    }
    let Some(v) = json::parse(t) else {
        return CargoLine::Text(String::from(line));
    };
    match v.get("reason").str() {
        Some("compiler-message") => {
            let m = v.get("message");
            let rendered = String::from(m.get("rendered").str().unwrap_or(""));
            CargoLine::Message {
                rendered,
                diagnostic: diagnostic_of(m),
            }
        }
        Some("compiler-artifact") => {
            let bin = v
                .at(&["target", "kind"])
                .arr()
                .iter()
                .any(|k| k.str() == Some("bin"));
            match v.get("executable").str() {
                Some(exe) if bin => CargoLine::Executable(String::from(exe)),
                _ => CargoLine::Other,
            }
        }
        Some("build-finished") => CargoLine::BuildFinished {
            success: v.get("success").bool().unwrap_or(false),
        },
        Some(_) => CargoLine::Other,
        None => CargoLine::Text(String::from(line)),
    }
}

fn diagnostic_of(m: &Value) -> Option<Diagnostic> {
    let severity = severity_of(m.get("level").str()?)?;
    let spans = m.get("spans").arr();
    let span = spans
        .iter()
        .find(|s| s.get("is_primary").bool() == Some(true))
        .or(spans.first())?;
    let file = span.get("file_name").str()?;
    let l = span.get("line_start").uint()?.saturating_sub(1);
    let c = span.get("column_start").uint()?.saturating_sub(1);
    let el = span
        .get("line_end")
        .uint()
        .map(|n| n.saturating_sub(1))
        .unwrap_or(l);
    let ec = span
        .get("column_end")
        .uint()
        .map(|n| n.saturating_sub(1))
        .unwrap_or(c + 1);
    Some(Diagnostic {
        file: String::from(file),
        line: l,
        col: c,
        end_line: el,
        end_col: ec,
        severity,
        message: String::from(m.get("message").str().unwrap_or("")),
        code: m.at(&["code", "code"]).str().map(String::from),
        source: Source::Build,
    })
}

/// Bytes into whole lines, keeping a partial last line for the next bytes.
#[derive(Debug, Default, Clone)]
pub struct Lines {
    partial: Vec<u8>,
}

impl Lines {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.partial.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some(i) = self.partial.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=i).collect();
            let mut s = String::from_utf8_lossy(&line[..line.len() - 1]).into_owned();
            if s.ends_with('\r') {
                s.pop();
            }
            out.push(s);
        }
        out
    }

    /// Whatever is left when the stream ends.
    pub fn finish(&mut self) -> Option<String> {
        if self.partial.is_empty() {
            return None;
        }
        let s = String::from_utf8_lossy(&self.partial).into_owned();
        self.partial.clear();
        Some(s)
    }
}

/// `edition = "2021"` from a Cargo.toml, for rustfmt.
pub fn edition_of(cargo_toml: &str) -> Option<String> {
    for line in cargo_toml.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("edition") {
            let rest = rest.trim_start();
            if let Some(v) = rest.strip_prefix('=') {
                let v = v.trim().trim_matches('"').trim_matches('\'');
                if !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) {
                    return Some(String::from(v));
                }
            }
        }
    }
    None
}

/// A runner the tests script: it records what was started and hands back
/// whatever the test queued.
#[cfg(test)]
pub mod fake {
    use super::*;
    use alloc::rc::Rc;
    use core::cell::RefCell;

    #[derive(Default)]
    pub struct State {
        pub started: Vec<(u64, String, Vec<String>, Option<String>)>,
        pub events: Vec<JobEvent>,
        pub written: Vec<(u64, Vec<u8>)>,
        pub killed: Vec<u64>,
        pub resized: Vec<(u64, u16, u16)>,
        pub missing: Vec<String>,
        /// What `run_to_end` answers for a program.
        pub finished: Vec<(String, Finished)>,
        pub folder: Option<String>,
        next: u64,
    }

    /// Cloned into the workspace; the test keeps the other handle.
    #[derive(Clone, Default)]
    pub struct FakeRunner(pub Rc<RefCell<State>>);

    impl FakeRunner {
        pub fn new() -> Self {
            Self::default()
        }
        pub fn push(&self, e: JobEvent) {
            self.0.borrow_mut().events.push(e);
        }
        pub fn out(&self, job: u64, s: &str) {
            self.push(JobEvent::Output {
                job,
                bytes: s.as_bytes().to_vec(),
                stderr: false,
            });
        }
        pub fn exit(&self, job: u64, code: i32) {
            self.push(JobEvent::Exited {
                job,
                code: Some(code),
                signal: None,
            });
        }
        pub fn last(&self) -> Option<(u64, String, Vec<String>)> {
            self.0
                .borrow()
                .started
                .last()
                .map(|(j, p, a, _)| (*j, p.clone(), a.clone()))
        }
        pub fn job_of(&self, program: &str) -> Option<u64> {
            self.0
                .borrow()
                .started
                .iter()
                .rev()
                .find(|s| s.1 == program)
                .map(|s| s.0)
        }
        pub fn written_to(&self, job: u64) -> Vec<u8> {
            let mut v = Vec::new();
            for (j, b) in &self.0.borrow().written {
                if *j == job {
                    v.extend_from_slice(b);
                }
            }
            v
        }
        pub fn clear_written(&self) {
            self.0.borrow_mut().written.clear();
        }
    }

    impl Runner for FakeRunner {
        fn set_folder(&mut self, folder: Option<String>) {
            self.0.borrow_mut().folder = folder;
        }
        fn folder(&self) -> Option<&str> {
            // The fake always has a folder.
            Some("picked/test")
        }
        fn start(
            &mut self,
            program: &str,
            args: &[String],
            sub: Option<&str>,
        ) -> Result<u64, RunError> {
            let mut s = self.0.borrow_mut();
            if s.missing.iter().any(|m| m == program) {
                let mut w = String::from(program);
                w.push_str(" is not installed on this computer");
                return Err(RunError::Missing(w));
            }
            s.next += 1;
            let id = s.next;
            s.started.push((
                id,
                String::from(program),
                args.to_vec(),
                sub.map(String::from),
            ));
            Ok(id)
        }
        fn terminal(&mut self, cols: u16, rows: u16) -> Result<u64, RunError> {
            let id = self.start("shell", &[], None)?;
            self.0.borrow_mut().resized.push((id, cols, rows));
            Ok(id)
        }
        fn poll(&mut self) -> Vec<JobEvent> {
            core::mem::take(&mut self.0.borrow_mut().events)
        }
        fn write(&mut self, job: u64, bytes: &[u8]) -> Result<(), String> {
            self.0.borrow_mut().written.push((job, bytes.to_vec()));
            Ok(())
        }
        fn close_stdin(&mut self, _job: u64) {}
        fn resize(&mut self, job: u64, cols: u16, rows: u16) {
            self.0.borrow_mut().resized.push((job, cols, rows));
        }
        fn kill(&mut self, job: u64) {
            self.0.borrow_mut().killed.push(job);
        }
        fn release(&mut self, _job: u64) {}
        fn run_to_end(
            &mut self,
            program: &str,
            args: &[String],
            input: &[u8],
            _timeout_ms: u32,
        ) -> Result<Finished, RunError> {
            let id = self.start(program, args, None)?;
            let mut s = self.0.borrow_mut();
            s.written.push((id, input.to_vec()));
            let pos = s.finished.iter().position(|(p, _)| p == program);
            match pos {
                Some(i) => Ok(s.finished.remove(i).1),
                None => Ok(Finished {
                    code: Some(0),
                    stdout: input.to_vec(),
                    stderr: Vec::new(),
                }),
            }
        }
    }
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
        assert_eq!(
            (d[0].file.as_str(), d[0].line, d[0].col),
            ("src/main.rs", 2, 8)
        );
        assert_eq!(d[1].code.as_deref(), Some("E0308"));
        assert_eq!(d[1].message, "mismatched types");
    }

    #[test]
    fn short_form_and_colours() {
        let d = parse_cargo("\u{1b}[1msrc/main.rs:4:18: \u{1b}[31merror[E0425]\u{1b}[0m: cannot find value `y` in this scope\nsrc/lib.rs:1:1: warning: missing docs\n");
        assert_eq!(d.len(), 2);
        assert_eq!(
            (d[0].line, d[0].col, d[0].severity),
            (3, 17, Severity::Error)
        );
        assert_eq!(d[1].severity, Severity::Warning);
    }

    /// A real line from `cargo build --message-format=json-diagnostic-rendered-ansi`.
    const REAL: &str = r#"{"reason":"compiler-message","package_id":"path+file:///tmp/p#0.1.0","manifest_path":"/tmp/p/Cargo.toml","target":{"kind":["bin"],"crate_types":["bin"],"name":"p","src_path":"/tmp/p/src/main.rs","edition":"2021","doc":true,"doctest":false,"test":true},"message":{"rendered":"\u001b[0m\u001b[1m\u001b[38;5;9merror[E0308]\u001b[0m\u001b[0m\u001b[1m: mismatched types\u001b[0m\n","$message_type":"diagnostic","children":[],"code":{"code":"E0308","explanation":"..."},"level":"error","message":"mismatched types","spans":[{"byte_end":40,"byte_start":38,"column_end":20,"column_start":18,"expansion":null,"file_name":"src/main.rs","is_primary":true,"label":"expected `i32`, found `&str`","line_end":2,"line_start":2,"suggested_replacement":null,"suggestion_applicability":null,"text":[]}]}}"#;

    #[test]
    fn json_messages_give_exact_places() {
        match read_cargo_line(REAL) {
            CargoLine::Message {
                rendered,
                diagnostic: Some(d),
            } => {
                assert!(rendered.contains("mismatched types"));
                assert_eq!(
                    (d.file.as_str(), d.line, d.col, d.end_col),
                    ("src/main.rs", 1, 17, 19)
                );
                assert_eq!(d.code.as_deref(), Some("E0308"));
                assert_eq!(d.severity, Severity::Error);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            read_cargo_line(r#"{"reason":"build-finished","success":false}"#),
            CargoLine::BuildFinished { success: false }
        );
        assert_eq!(
            read_cargo_line(
                r#"{"reason":"compiler-artifact","target":{"kind":["bin"]},"executable":"/tmp/p/target/debug/p","fresh":false}"#
            ),
            CargoLine::Executable(String::from("/tmp/p/target/debug/p"))
        );
        assert_eq!(
            read_cargo_line("test tests::it_works ... ok"),
            CargoLine::Text(String::from("test tests::it_works ... ok"))
        );
        // A summary with no place is text for the Output panel only.
        let summary = r#"{"reason":"compiler-message","message":{"rendered":"error: aborting due to 1 previous error\n","level":"error","message":"aborting due to 1 previous error","spans":[],"code":null}}"#;
        assert!(matches!(
            read_cargo_line(summary),
            CargoLine::Message {
                diagnostic: None,
                ..
            }
        ));
    }

    #[test]
    fn lines_keep_a_partial_tail() {
        let mut l = Lines::default();
        assert_eq!(l.push(b"ab"), Vec::<String>::new());
        assert_eq!(
            l.push(b"c\r\nde\nf"),
            alloc::vec![String::from("abc"), String::from("de")]
        );
        assert_eq!(l.finish().as_deref(), Some("f"));
        assert_eq!(l.finish(), None);
    }

    #[test]
    fn edition_from_cargo_toml() {
        assert_eq!(
            edition_of("[package]\nname = \"x\"\nedition = \"2024\"\n").as_deref(),
            Some("2024")
        );
        assert_eq!(edition_of("[package]\nname = \"x\"\n"), None);
    }

    #[test]
    fn every_action_names_its_program() {
        let mut r = NoRunner;
        for t in Tool::ALL {
            assert!(!t.title().is_empty());
            assert!(!t.program().is_empty());
        }
        assert!(matches!(
            r.start("cargo", &Tool::Build.args(), None),
            Err(RunError::Other(_))
        ));
        assert!(Tool::Build
            .args()
            .iter()
            .any(|a| a.starts_with("--message-format=json")));
        assert!(r.poll().is_empty());
    }
}
