//! What turns the editor into an IDE, on top of [`Runner`]: builds and
//! runs with their problems, formatting, terminals, the language server
//! and the debugger. All of it is the workspace's state and rules, with no
//! host call of its own, so a whole session runs in a unit test against a
//! scripted runner.
//!
//! [`Runner`]: crate::tools::Runner

use crate::buffer::Pos;
use crate::dap::{Dap, DapEvent, DapState};
use crate::lsp::{Asked, CodeAction, CompletionItem, FileEdit, Location, Lsp, LspEvent, TextEdit};
use crate::project;
use crate::text;
use crate::tools::{self, CargoLine, Diagnostic, Lines, RunError, Severity, Tool};
use crate::vim::{Input, Key, Mode};
use crate::vt::{self, Vt};
use crate::workspace::{Focus, PaletteMode, PaletteState, PanelTab, Prompt, PromptKind, Workspace};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// The longest a save waits for rustfmt.
const FORMAT_WAIT_MS: u32 = 4000;
/// Two Escapes this close together leave a terminal.
const ESC_ESC_MS: u64 = 450;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobKind {
    Cargo(Tool),
}

/// A program the editor started and is reading.
#[derive(Debug)]
pub struct Job {
    pub id: u64,
    pub kind: JobKind,
    out: Lines,
    err: Lines,
    pub started: u64,
    pub errors: usize,
    pub warnings: usize,
    pub success: Option<bool>,
    pub stopped: bool,
}

/// One terminal: the shell's job and what it shows.
pub struct Term {
    pub job: u64,
    pub vt: Vt,
    /// Its number in the tabs (1, 2, ...), kept when others close.
    pub n: usize,
    pub ended: bool,
}

pub struct LspSession {
    pub job: u64,
    pub client: Lsp,
    /// The buffer version the server last saw for each file.
    pub synced: BTreeMap<String, u64>,
    /// A completion request is out: no second one until it answers.
    pub asking_completion: bool,
}

pub struct DapSession {
    pub job: u64,
    pub client: Dap,
}

/// The completion popup.
#[derive(Debug, Clone)]
pub struct Completion {
    pub items: Vec<CompletionItem>,
    /// Indices into `items`, best first, for what is typed now.
    pub shown: Vec<usize>,
    pub selected: usize,
    pub path: String,
    pub line: usize,
    /// Where the word being completed starts (bytes).
    pub start: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hover {
    pub text: String,
    pub path: String,
    pub line: usize,
    /// Byte column the box points at.
    pub col: usize,
    /// Asked by resting the mouse: moving it away closes it.
    pub from_mouse: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    pub label: String,
    pub active: Option<(usize, usize)>,
    pub path: String,
    pub line: usize,
}

/// What a row of a pick list does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    Place {
        path: String,
        line: usize,
        col: usize,
    },
    Action(CodeAction),
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn words_for(program: &str, e: &RunError) -> String {
    match e {
        RunError::Missing(w) => {
            let mut s = String::from(w);
            s.push_str(". ");
            s.push_str(tools::install_hint(program));
            s
        }
        RunError::Refused(w) | RunError::Other(w) => String::from(w.as_str()),
    }
}

impl Workspace {
    // ---- what the guest asks ------------------------------------------------

    /// Whether anything is running that may print: the guest wakes often
    /// while this is true.
    pub fn busy(&self) -> bool {
        !self.jobs.is_empty()
            || self.terminals.iter().any(|t| !t.ended)
            || self.lsp.is_some()
            || self.dap.is_some()
            || self.creating.is_some()
    }

    /// The running build or program, for the status line.
    pub fn running_label(&self) -> Option<String> {
        let j = self.jobs.first()?;
        let JobKind::Cargo(t) = j.kind;
        let mut s = String::from(t.doing());
        s.push_str("... ");
        let secs = self.now_ms.saturating_sub(j.started) / 1000;
        s.push_str(&text::num(secs as usize));
        s.push('s');
        Some(s)
    }

    /// Whether the keyboard goes to a terminal now.
    pub fn terminal_focused(&self) -> bool {
        self.prompt.is_none()
            && self.palette.is_none()
            && self.focus == Focus::Panel
            && self.panel_open
            && self.panel_tab == PanelTab::Terminal
            && self
                .terminals
                .get(self.term_active)
                .is_some_and(|t| !t.ended)
    }

    /// The terminal shown in the panel.
    pub fn active_term(&self) -> Option<&Term> {
        self.terminals.get(self.term_active)
    }

    // ---- actions --------------------------------------------------------------

    /// Build, run, format, rust-analyzer, debug, terminal.
    pub fn tool(&mut self, t: Tool) {
        match t {
            Tool::Build | Tool::Run | Tool::Test | Tool::Check | Tool::Clippy => {
                self.debug_after_build = false;
                self.start_cargo(t);
            }
            Tool::Debug => self.start_debug(),
            Tool::Format => {
                if let Some(d) = self.active_doc_index() {
                    self.format_doc(d, false);
                } else {
                    self.fail("Open a Rust file to format it");
                }
            }
            Tool::Complete => self.lsp_complete(None),
            Tool::Hover => self.lsp_hover(),
            Tool::Definition => self.lsp_definition(),
            Tool::References => self.lsp_references(),
            Tool::CodeAction => self.lsp_actions(),
            Tool::RenameSymbol => {
                if !self.lsp_ready_or_say() {
                    return;
                }
                let word = self.word_at_cursor().unwrap_or_default();
                let mut title = String::from("Rename ");
                title.push_str(if word.is_empty() { "the symbol" } else { &word });
                title.push_str(" everywhere it is used");
                self.prompt = Some(Prompt {
                    kind: PromptKind::RenameSymbol,
                    title,
                    text: word,
                    error: None,
                });
            }
            Tool::Continue | Tool::StepOver | Tool::StepIn | Tool::StepOut | Tool::StopDebug => {
                self.debug_command(t)
            }
            Tool::Terminal => self.new_terminal(),
            Tool::Stop => self.stop_jobs(),
        }
    }

    fn start_cargo(&mut self, t: Tool) {
        if self.project.is_none() {
            self.fail("Open a project folder first (Ctrl+O)");
            return;
        }
        if self.runner.folder().is_none() {
            self.fail(tools::NEEDS_FOLDER);
            return;
        }
        // One build at a time: a new one replaces the one going.
        let old: Vec<u64> = self.jobs.iter().map(|j| j.id).collect();
        for id in old {
            self.runner.kill(id);
            self.runner.release(id);
        }
        self.jobs.clear();
        let saved = self.dirty_count();
        if saved > 0 {
            self.save_all();
        }
        let args = t.args();
        // The verb is what the person asked for; the message-format flags
        // are the editor's own business.
        let mut line = String::from("\u{1b}[1m$ cargo");
        if let Some(verb) = args.first() {
            line.push(' ');
            line.push_str(verb);
        }
        line.push_str("\u{1b}[0m");
        self.log("");
        self.log(&line);
        match self.runner.start("cargo", &args, None) {
            Ok(id) => {
                self.jobs.push(Job {
                    id,
                    kind: JobKind::Cargo(t),
                    out: Lines::default(),
                    err: Lines::default(),
                    started: self.now_ms,
                    errors: 0,
                    warnings: 0,
                    success: None,
                    stopped: false,
                });
                self.build_problems.clear();
                self.refresh_problems();
                self.output.view = 0;
                self.show_panel(PanelTab::Output);
                let mut m = String::from(t.doing());
                m.push_str(" -- the Output panel shows it as it goes; :stop stops it");
                if saved > 0 {
                    m.push_str(" (saved your changes first)");
                }
                self.say(&m);
            }
            Err(e) => {
                if matches!(e, RunError::Missing(_)) {
                    self.missing.insert(String::from("cargo"));
                }
                let m = words_for("cargo", &e);
                self.log(&m);
                self.show_panel(PanelTab::Output);
                self.fail(&m);
            }
        }
    }

    fn stop_jobs(&mut self) {
        if self.jobs.is_empty() {
            self.say("Nothing is running");
            return;
        }
        for j in self.jobs.iter_mut() {
            j.stopped = true;
            self.runner.kill(j.id);
        }
        self.debug_after_build = false;
        self.say("Stopping...");
    }

    /// `cargo init` in a picked folder. Gives the folder back when cargo
    /// cannot run, for the plain skeleton.
    pub(crate) fn cargo_init(
        &mut self,
        folder: &str,
        fs: Box<dyn project::Vfs>,
        name: &str,
    ) -> Result<(), Box<dyn project::Vfs>> {
        self.runner.set_folder(fs.folder());
        let args: Vec<String> = ["init", "--name", name]
            .iter()
            .map(|s| String::from(*s))
            .collect();
        match self.runner.start("cargo", &args, None) {
            Ok(id) => {
                let mut line = String::from("$ cargo init --name ");
                line.push_str(name);
                self.log(&line);
                self.creating = Some((String::from(folder), fs, String::from(name), id));
                self.say("Making the project with cargo...");
                Ok(())
            }
            Err(e) => {
                if matches!(e, RunError::Missing(_)) {
                    self.missing.insert(String::from("cargo"));
                }
                Err(fs)
            }
        }
    }

    // ---- what comes back --------------------------------------------------

    /// What the runner hands back: output to the panels, problems parsed
    /// out, the language server's and debugger's answers handled. True when
    /// anything changed.
    pub fn poll_runner(&mut self) -> bool {
        let events = self.runner.poll();
        let mut changed = !events.is_empty();
        for e in events {
            match e {
                tools::JobEvent::Output { job, bytes, stderr } => {
                    self.job_output(job, &bytes, stderr)
                }
                tools::JobEvent::Exited { job, code, signal } => self.job_exited(job, code, signal),
            }
        }
        // Keep the server's copy of each file current, then send.
        if self.lsp.as_ref().is_some_and(|l| l.client.ready) {
            self.lsp_sync_all();
        }
        changed |= self.flush_protocols();
        changed
    }

    fn flush_protocols(&mut self) -> bool {
        let mut any = false;
        if let Some(l) = self.lsp.as_mut() {
            if !l.client.out.is_empty() {
                let bytes = core::mem::take(&mut l.client.out);
                let job = l.job;
                let _ = self.runner.write(job, &bytes);
                any = true;
            }
        }
        if let Some(d) = self.dap.as_mut() {
            if !d.client.out.is_empty() {
                let bytes = core::mem::take(&mut d.client.out);
                let job = d.job;
                let _ = self.runner.write(job, &bytes);
                any = true;
            }
        }
        any
    }

    fn job_output(&mut self, job: u64, bytes: &[u8], stderr: bool) {
        if let Some(t) = self.terminals.iter_mut().find(|t| t.job == job) {
            t.vt.feed(bytes);
            if !t.vt.reply.is_empty() {
                let r = core::mem::take(&mut t.vt.reply);
                let _ = self.runner.write(job, &r);
            }
            return;
        }
        if self.lsp.as_ref().is_some_and(|l| l.job == job) {
            if !stderr {
                let ev = self
                    .lsp
                    .as_mut()
                    .map(|l| l.client.receive(bytes))
                    .unwrap_or_default();
                for e in ev {
                    self.lsp_event(e);
                }
            }
            return;
        }
        if self.dap.as_ref().is_some_and(|d| d.job == job) {
            if stderr {
                return;
            }
            let ev = self
                .dap
                .as_mut()
                .map(|d| d.client.receive(bytes))
                .unwrap_or_default();
            for e in ev {
                self.dap_event(e);
            }
            return;
        }
        if self.creating.as_ref().is_some_and(|c| c.3 == job) {
            let s = String::from_utf8_lossy(bytes).into_owned();
            self.output.write_text(&s);
            return;
        }
        let Some(k) = self.jobs.iter().position(|j| j.id == job) else {
            return;
        };
        let lines = if stderr {
            self.jobs[k].err.push(bytes)
        } else {
            self.jobs[k].out.push(bytes)
        };
        for l in lines {
            self.cargo_line(k, &l, stderr);
        }
    }

    fn cargo_line(&mut self, k: usize, line: &str, stderr: bool) {
        if stderr {
            self.output.write_text(line);
            self.output.write_text("\n");
            return;
        }
        match tools::read_cargo_line(line) {
            CargoLine::Message {
                rendered,
                diagnostic,
            } => {
                self.output.write_text(&rendered);
                if let Some(d) = diagnostic {
                    match d.severity {
                        Severity::Error => self.jobs[k].errors += 1,
                        Severity::Warning => self.jobs[k].warnings += 1,
                        Severity::Note => {}
                    }
                    self.build_problems.push(d);
                    self.refresh_problems();
                }
            }
            CargoLine::Executable(p) => self.last_exe = Some(p),
            CargoLine::BuildFinished { success } => self.jobs[k].success = Some(success),
            CargoLine::Text(t) => {
                self.output.write_text(&t);
                self.output.write_text("\n");
            }
            CargoLine::Other => {}
        }
    }

    fn job_exited(&mut self, job: u64, code: Option<i32>, signal: Option<i32>) {
        if let Some(t) = self.terminals.iter_mut().find(|t| t.job == job) {
            t.ended = true;
            t.vt.write_text(
                "\r\n\u{1b}[2m[the shell ended -- Ctrl+` opens a new terminal]\u{1b}[0m",
            );
            self.runner.release(job);
            return;
        }
        if self.lsp.as_ref().is_some_and(|l| l.job == job) {
            self.lsp = None;
            self.runner.release(job);
            self.lsp_busy = None;
            self.completion = None;
            self.lsp_problems.clear();
            self.refresh_problems();
            self.log("rust-analyzer stopped (:lsp starts it again)");
            return;
        }
        if self.dap.as_ref().is_some_and(|d| d.job == job) {
            self.dap = None;
            self.runner.release(job);
            if self.panel_tab == PanelTab::Debug {
                self.panel_tab = PanelTab::Output;
            }
            return;
        }
        if self.creating.as_ref().is_some_and(|c| c.3 == job) {
            let Some((folder, fs, name, _)) = self.creating.take() else {
                return;
            };
            self.runner.release(job);
            if code == Some(0) {
                self.open_project(&folder, fs);
                let mut m = String::from("Made the project ");
                m.push_str(&name);
                m.push_str(" with cargo -- F7 builds it, F5 runs it");
                self.say(&m);
            } else {
                // Put the folder back and ask again; cargo said why.
                self.new_project_folder(&folder, fs);
                if let Some(p) = self.prompt.as_mut() {
                    p.text = name;
                    p.error = Some(String::from(
                        "cargo could not make it (the Output panel says why)",
                    ));
                }
                self.show_panel(PanelTab::Output);
            }
            return;
        }
        let Some(k) = self.jobs.iter().position(|j| j.id == job) else {
            return;
        };
        let tails: Vec<(String, bool)> = {
            let j = &mut self.jobs[k];
            let mut v = Vec::new();
            if let Some(l) = j.out.finish() {
                v.push((l, false));
            }
            if let Some(l) = j.err.finish() {
                v.push((l, true));
            }
            v
        };
        for (l, e) in tails {
            self.cargo_line(k, &l, e);
        }
        let j = self.jobs.remove(k);
        self.runner.release(job);
        let JobKind::Cargo(t) = j.kind;
        let secs = self.now_ms.saturating_sub(j.started);
        let took = {
            let mut s = text::num((secs / 1000) as usize);
            s.push('.');
            s.push_str(&text::num(((secs % 1000) / 100) as usize));
            s.push('s');
            s
        };
        let ok = code == Some(0);
        let mut m = String::new();
        if j.stopped || signal.is_some() && !ok {
            m.push_str("Stopped");
        } else if ok {
            m.push_str(match t {
                Tool::Run => "The program finished",
                Tool::Test => "Tests passed",
                _ => "Built",
            });
            m.push_str(" in ");
            m.push_str(&took);
            if j.warnings > 0 {
                m.push_str(", ");
                m.push_str(&text::num(j.warnings));
                m.push_str(if j.warnings == 1 {
                    " warning"
                } else {
                    " warnings"
                });
            }
        } else {
            m.push_str(match t {
                Tool::Run if j.errors == 0 => "The program ended with exit code ",
                Tool::Test if j.errors == 0 => "Tests failed, exit code ",
                _ => "",
            });
            if m.is_empty() {
                m.push_str("cargo ");
                m.push_str(match t {
                    Tool::Run => "run",
                    Tool::Test => "test",
                    Tool::Check => "check",
                    Tool::Clippy => "clippy",
                    _ => "build",
                });
                m.push_str(" failed: ");
                m.push_str(&text::num(j.errors));
                m.push_str(if j.errors == 1 { " error" } else { " errors" });
                if j.warnings > 0 {
                    m.push_str(", ");
                    m.push_str(&text::num(j.warnings));
                    m.push_str(if j.warnings == 1 {
                        " warning"
                    } else {
                        " warnings"
                    });
                }
                m.push_str(" -- Enter on one in Problems jumps to it");
            } else {
                let c = code.unwrap_or(-1);
                if c < 0 {
                    m.push('-');
                }
                m.push_str(&text::num(c.unsigned_abs() as usize));
            }
        }
        let mut logged = String::from("\u{1b}[2m");
        logged.push_str(&m);
        logged.push_str("\u{1b}[0m");
        self.log(&logged);
        if j.errors > 0 {
            self.show_panel(PanelTab::Problems);
            self.fail(&m);
        } else if ok {
            self.say(&m);
        } else {
            self.fail(&m);
        }
        if self.debug_after_build {
            self.debug_after_build = false;
            if ok && j.errors == 0 {
                self.launch_debugger();
            }
        }
    }

    pub fn set_problems(&mut self, p: Vec<Diagnostic>) {
        self.build_problems = p;
        self.refresh_problems();
        if !self.problems.is_empty() {
            self.show_panel(PanelTab::Problems);
        }
    }

    /// The Problems list: the build's, then the language server's, without
    /// the same problem twice. Errors first.
    pub fn refresh_problems(&mut self) {
        let mut all: Vec<Diagnostic> = self.build_problems.clone();
        for ds in self.lsp_problems.values() {
            for d in ds {
                let dup = all
                    .iter()
                    .any(|b| b.file == d.file && b.line == d.line && b.message == d.message);
                if !dup {
                    all.push(d.clone());
                }
            }
        }
        let rank = |s: Severity| match s {
            Severity::Error => 0,
            Severity::Warning => 1,
            Severity::Note => 2,
        };
        all.sort_by(|a, b| {
            rank(a.severity)
                .cmp(&rank(b.severity))
                .then(a.file.cmp(&b.file))
                .then(a.line.cmp(&b.line))
        });
        self.problems = all;
        if self.problem_selected >= self.problems.len() {
            self.problem_selected = 0;
        }
    }

    /// Problems in `path` on line `line`, for squiggles and the gutter.
    pub fn problems_on(&self, path: &str, line: usize) -> impl Iterator<Item = &Diagnostic> {
        let path = String::from(path);
        self.problems
            .iter()
            .filter(move |d| d.file == path && d.line <= line && line <= d.end_line.max(d.line))
    }

    /// Stop everything the project started: when another one opens.
    pub(crate) fn close_ide(&mut self) {
        for j in core::mem::take(&mut self.jobs) {
            self.runner.kill(j.id);
            self.runner.release(j.id);
        }
        if let Some(mut l) = self.lsp.take() {
            l.client.shutdown();
            let b = core::mem::take(&mut l.client.out);
            let _ = self.runner.write(l.job, &b);
            self.runner.release(l.job);
        }
        if let Some(d) = self.dap.take() {
            self.runner.release(d.job);
        }
        self.root_abs = None;
        self.completion = None;
        self.hover = None;
        self.signature = None;
        self.lsp_busy = None;
        self.last_exe = None;
    }

    // ---- formatting ---------------------------------------------------------

    /// Run rustfmt over a Rust file, through its stdin, in the project
    /// folder so the project's rustfmt.toml applies. The new text replaces
    /// the old as one change: one `u` puts it back. `quiet` (a save) says
    /// nothing when there is nothing to say.
    pub fn format_doc(&mut self, d: usize, quiet: bool) -> bool {
        let Some(doc) = self.docs.get(d) else {
            return false;
        };
        if !doc.is_rust() {
            if !quiet {
                self.fail("rustfmt formats Rust files (.rs)");
            }
            return false;
        }
        if self.runner.folder().is_none() || self.missing.contains("rustfmt") {
            if !quiet {
                if self.missing.contains("rustfmt") {
                    let mut m = String::from("rustfmt is not installed on this computer. ");
                    m.push_str(tools::install_hint("rustfmt"));
                    self.fail(&m);
                } else {
                    self.fail(tools::NEEDS_FOLDER);
                }
            }
            return false;
        }
        let edition = self
            .project
            .as_mut()
            .and_then(|p| p.fs.read("Cargo.toml").ok())
            .and_then(|t| tools::edition_of(&t))
            .unwrap_or_else(|| String::from("2021"));
        let args: Vec<String> = ["--emit=stdout", "--edition", edition.as_str()]
            .iter()
            .map(|s| String::from(*s))
            .collect();
        let before = doc.buf.text();
        let input = if doc.buf.crlf {
            before.replace("\r\n", "\n")
        } else {
            before.clone()
        };
        match self
            .runner
            .run_to_end("rustfmt", &args, input.as_bytes(), FORMAT_WAIT_MS)
        {
            Ok(f) if f.code == Some(0) => {
                let after = String::from_utf8_lossy(&f.stdout).into_owned();
                if after.is_empty() && !input.is_empty() {
                    if !quiet {
                        self.fail("rustfmt gave back nothing, so the file was left as it was");
                    }
                    return false;
                }
                if after == input {
                    if !quiet {
                        self.say("Already formatted");
                    }
                    return true;
                }
                self.replace_doc_text(d, &after);
                if !quiet {
                    self.say("Formatted with rustfmt (u undoes it)");
                }
                true
            }
            Ok(f) => {
                let err = String::from_utf8_lossy(&f.stderr).into_owned();
                let first = tools::strip_ansi(
                    err.lines()
                        .find(|l| l.trim_start().starts_with("error"))
                        .or(err.lines().next())
                        .unwrap_or("it did not say why"),
                );
                let mut m = String::from(if quiet {
                    "Saved without formatting: rustfmt says "
                } else {
                    "rustfmt could not format it: "
                });
                m.push_str(first.trim());
                if f.code.is_none() {
                    m = String::from("rustfmt took too long, so the file was left as it was");
                }
                self.fail(&m);
                false
            }
            Err(e) => {
                if matches!(e, RunError::Missing(_)) {
                    self.missing.insert(String::from("rustfmt"));
                }
                if !quiet || matches!(e, RunError::Missing(_)) {
                    let m = words_for("rustfmt", &e);
                    self.fail(&m);
                }
                false
            }
        }
    }

    /// Replace a document's whole text as one undoable change, touching
    /// only the lines that differ so the cursor and breakpoints stay put.
    pub fn replace_doc_text(&mut self, d: usize, new_text: &str) {
        let Some(doc) = self.docs.get_mut(d) else {
            return;
        };
        let new_lines: Vec<&str> = new_text.split('\n').collect();
        let old_n = doc.buf.line_count();
        let mut pre = 0;
        while pre < old_n && pre < new_lines.len() && doc.buf.line(pre) == new_lines[pre] {
            pre += 1;
        }
        let mut suf = 0;
        while suf < old_n - pre
            && suf < new_lines.len() - pre
            && doc.buf.line(old_n - 1 - suf) == new_lines[new_lines.len() - 1 - suf]
        {
            suf += 1;
        }
        let cursor = self
            .panes
            .iter()
            .find(|p| p.doc == Some(d))
            .map(|p| p.cur.pos)
            .unwrap_or_default();
        // A typing run still open is its own step; the format is the next.
        let was_open = doc.buf.group_open();
        if was_open {
            doc.buf.end_group(cursor);
        }
        doc.buf.begin_group(cursor);
        let a_line = pre;
        let b_line = old_n - suf; // exclusive
        let middle: Vec<&str> = new_lines[pre..new_lines.len() - suf].to_vec();
        if a_line < b_line {
            // Replace whole old lines a..b with the middle.
            let a = Pos::new(a_line, 0);
            let b = if b_line < old_n {
                Pos::new(b_line, 0)
            } else {
                Pos::new(old_n - 1, doc.buf.line_len(old_n - 1))
            };
            let mut t = middle.join("\n");
            if b_line < old_n && !middle.is_empty() {
                t.push('\n');
            }
            if b_line >= old_n && a_line > 0 && middle.is_empty() {
                // Deleting the last lines: take the newline before them.
                let prev = a_line - 1;
                let a2 = Pos::new(prev, doc.buf.line_len(prev));
                doc.buf.replace(a2, b, "");
            } else {
                doc.buf.replace(a, b, &t);
            }
        } else if !middle.is_empty() {
            // Only additions, at line a.
            if a_line < old_n {
                let mut t = middle.join("\n");
                t.push('\n');
                doc.buf.insert(Pos::new(a_line, 0), &t);
            } else {
                let last = old_n - 1;
                let mut t = String::from("\n");
                t.push_str(&middle.join("\n"));
                doc.buf.insert(Pos::new(last, doc.buf.line_len(last)), &t);
            }
        }
        doc.buf.end_group(cursor);
        if was_open {
            doc.buf.begin_group(cursor);
        }
        doc.sync();
        for p in self.panes.iter_mut() {
            if p.doc == Some(d) {
                p.cur.pos = doc.buf.clamp(p.cur.pos);
                p.cur.anchor = doc.buf.clamp(p.cur.anchor);
            }
        }
    }

    // ---- terminals ------------------------------------------------------------

    /// Open a new terminal in the project folder, in the panel.
    pub fn new_terminal(&mut self) {
        if self.runner.folder().is_none() {
            self.show_panel(PanelTab::Terminal);
            self.fail(tools::NEEDS_FOLDER);
            return;
        }
        let (cols, rows) = self.term_size;
        match self.runner.terminal(cols.max(10), rows.max(3)) {
            Ok(job) => {
                let n = self.terminals.iter().map(|t| t.n).max().unwrap_or(0) + 1;
                let mut v = Vt::new(cols as usize, rows as usize);
                v.dark = true;
                self.terminals.push(Term {
                    job,
                    vt: v,
                    n,
                    ended: false,
                });
                self.term_active = self.terminals.len() - 1;
                self.term_sel = None;
                self.leave_editor_mode();
                self.show_panel(PanelTab::Terminal);
                self.focus = Focus::Panel;
                self.say(
                    "Typing goes to the terminal -- Esc twice (or Ctrl+`) goes back to the editor",
                );
            }
            Err(e) => {
                self.show_panel(PanelTab::Terminal);
                let m = words_for("shell", &e);
                self.fail(&m);
            }
        }
    }

    fn leave_editor_mode(&mut self) {
        if self.vim.mode != Mode::Normal {
            self.leave_mode();
        }
    }

    /// Show the terminal (opening one when there is none), or go back to
    /// the editor from it.
    pub fn toggle_terminal(&mut self) {
        if self.terminal_focused() {
            self.focus = Focus::Editor;
            return;
        }
        if self.terminals.iter().all(|t| t.ended) {
            self.new_terminal();
            return;
        }
        if self.terminals.get(self.term_active).is_none_or(|t| t.ended) {
            self.term_active = self.terminals.iter().position(|t| !t.ended).unwrap_or(0);
        }
        self.leave_editor_mode();
        self.show_panel(PanelTab::Terminal);
        self.focus = Focus::Panel;
    }

    pub fn close_terminal(&mut self, i: usize) {
        if i >= self.terminals.len() {
            return;
        }
        let t = self.terminals.remove(i);
        self.runner.kill(t.job);
        self.runner.release(t.job);
        if self.term_active >= self.terminals.len() {
            self.term_active = self.terminals.len().saturating_sub(1);
        }
        if self.terminals.is_empty() && self.focus == Focus::Panel {
            self.focus = Focus::Editor;
        }
    }

    pub fn select_terminal(&mut self, i: usize) {
        if i < self.terminals.len() {
            self.term_active = i;
            self.term_sel = None;
            self.show_panel(PanelTab::Terminal);
            self.focus = Focus::Panel;
        }
    }

    fn term_write(&mut self, bytes: &[u8]) {
        let Some(t) = self.terminals.get_mut(self.term_active) else {
            return;
        };
        if t.ended {
            return;
        }
        t.vt.view = 0;
        let job = t.job;
        self.term_sel = None;
        let _ = self.runner.write(job, bytes);
    }

    /// A key pressed while the terminal has the keyboard, by the runtime's
    /// name for it. True when it went to the terminal.
    pub fn term_key(&mut self, name: &str, shift: bool, ctrl: bool, alt: bool) -> bool {
        if name == "Escape" {
            if self.now_ms.saturating_sub(self.last_esc_ms) < ESC_ESC_MS {
                self.last_esc_ms = 0;
                self.focus = Focus::Editor;
                self.say("Back in the editor (Ctrl+` returns to the terminal)");
                return true;
            }
            self.last_esc_ms = self.now_ms;
        }
        let app = self.active_term().is_some_and(|t| t.vt.app_cursor);
        match vt::key_bytes(name, shift, ctrl, alt, app) {
            Some(b) => {
                self.term_write(&b);
                true
            }
            None => false,
        }
    }

    /// Typed text for the terminal.
    pub fn term_text(&mut self, s: &str) {
        self.term_write(s.as_bytes());
    }

    pub fn term_paste(&mut self, s: &str) {
        let b = self
            .active_term()
            .map(|t| t.vt.paste_bytes(s))
            .unwrap_or_default();
        self.term_write(&b);
    }

    /// Text input from the editor's own key path (a test, the guest's
    /// fallback): characters and Enter go to the terminal.
    pub(crate) fn term_text_input(&mut self, input: &Input) -> bool {
        if !self.terminal_focused() {
            return false;
        }
        match input {
            Input::Char(c) => {
                let mut b = [0u8; 4];
                self.term_write(c.encode_utf8(&mut b).as_bytes());
            }
            Input::Key(Key::Enter) => self.term_write(b"\r"),
            Input::Key(Key::Backspace) => self.term_write(&[0x7f]),
            Input::Key(Key::Tab) => self.term_write(b"\t"),
            Input::Key(Key::Esc) => {
                self.term_key("Escape", false, false, false);
            }
            Input::Ctrl(c) => {
                let mut s = String::new();
                s.push(*c);
                self.term_key(&s, false, true, false);
            }
            Input::Paste(t) => {
                let t = t.clone();
                self.term_paste(&t);
            }
            _ => return false,
        }
        true
    }

    /// The panel fits `cols` by `rows` now: every terminal follows.
    pub fn term_resize(&mut self, cols: u16, rows: u16) {
        let (cols, rows) = (cols.max(10), rows.max(3));
        self.term_size = (cols, rows);
        for t in self.terminals.iter_mut() {
            if t.vt.cols != cols as usize || t.vt.rows != rows as usize {
                t.vt.resize(cols as usize, rows as usize);
                if !t.ended {
                    self.runner.resize(t.job, cols, rows);
                }
            }
        }
    }

    /// The selected text in the terminal, for the clipboard.
    pub fn term_copy(&mut self) -> bool {
        let (Some(t), Some((a, b))) = (self.active_term(), self.term_sel) else {
            return false;
        };
        let s = t.vt.text_between(a, b);
        if s.is_empty() {
            return false;
        }
        self.clipboard_out = Some(s);
        self.say("Copied");
        true
    }

    // ---- the language server -----------------------------------------------

    /// The project's place on the computer, asked of cargo once.
    pub fn project_root(&mut self) -> Option<String> {
        if let Some(r) = &self.root_abs {
            return Some(r.clone());
        }
        if self.runner.folder().is_none() || self.missing.contains("cargo") {
            return None;
        }
        let args: Vec<String> = ["locate-project", "--workspace", "--message-format", "plain"]
            .iter()
            .map(|s| String::from(*s))
            .collect();
        match self.runner.run_to_end("cargo", &args, b"", 15_000) {
            Ok(f) if f.code == Some(0) => {
                let out = String::from_utf8_lossy(&f.stdout).into_owned();
                let path = out.trim();
                let root = path
                    .strip_suffix("Cargo.toml")
                    .unwrap_or(path)
                    .trim_end_matches(['/', '\\']);
                if root.is_empty() {
                    return None;
                }
                self.root_abs = Some(String::from(root));
                Some(String::from(root))
            }
            Ok(_) => None,
            Err(e) => {
                if matches!(e, RunError::Missing(_)) {
                    self.missing.insert(String::from("cargo"));
                }
                None
            }
        }
    }

    /// Start rust-analyzer for the project. `asked`: the person asked, so
    /// say plainly why it cannot start.
    pub fn start_lsp(&mut self, asked: bool) {
        if asked {
            if let Some(l) = self.lsp.take() {
                self.runner.kill(l.job);
                self.runner.release(l.job);
            }
            self.missing.remove("rust-analyzer");
        }
        if self.lsp.is_some() {
            return;
        }
        if self.missing.contains("rust-analyzer") {
            if asked {
                self.lsp_missing_say();
            }
            return;
        }
        let Some(root) = self.project_root() else {
            if asked {
                self.fail(
                    "Suggestions need a Cargo project on disk: open a folder with a Cargo.toml",
                );
            }
            return;
        };
        match self.runner.start("rust-analyzer", &[], None) {
            Ok(job) => {
                self.lsp = Some(LspSession {
                    job,
                    client: Lsp::new(&root),
                    synced: BTreeMap::new(),
                    asking_completion: false,
                });
                self.lsp_busy = Some(String::from("Starting rust-analyzer"));
                self.flush_protocols();
            }
            Err(e) => {
                if matches!(e, RunError::Missing(_)) {
                    self.missing.insert(String::from("rust-analyzer"));
                    let mut m = String::from("Suggestions, hover and go to definition need rust-analyzer, which is not installed. ");
                    m.push_str(tools::install_hint("rust-analyzer"));
                    self.log(&m);
                    if asked {
                        self.fail(&m);
                    }
                } else if asked {
                    let m = words_for("rust-analyzer", &e);
                    self.fail(&m);
                }
            }
        }
    }

    fn lsp_missing_say(&mut self) {
        let mut m = String::from("rust-analyzer is not installed on this computer. ");
        m.push_str(tools::install_hint("rust-analyzer"));
        self.fail(&m);
    }

    /// Whether requests can go to the server now; says why not otherwise.
    fn lsp_ready_or_say(&mut self) -> bool {
        if self.lsp.is_none() {
            self.start_lsp(false);
        }
        match &self.lsp {
            Some(l) if l.client.ready => true,
            Some(_) => {
                self.say("rust-analyzer is still starting -- try again in a moment");
                false
            }
            None => {
                if self.missing.contains("rust-analyzer") {
                    self.lsp_missing_say();
                } else if self.runner.folder().is_none() {
                    self.fail("Suggestions need a project on disk (the tour lives in memory)");
                } else {
                    self.fail("rust-analyzer is not running (:lsp starts it)");
                }
                false
            }
        }
    }

    fn lsp_sync_all(&mut self) {
        let Some(l) = self.lsp.as_mut() else {
            return;
        };
        for d in self.docs.iter() {
            if !d.is_rust() {
                continue;
            }
            let v = d.buf.version;
            if l.synced.get(&d.path) != Some(&v) || !l.client.is_open(&d.path) {
                l.client.sync(&d.path, &d.buf.text());
                l.synced.insert(d.path.clone(), v);
            }
        }
    }

    pub(crate) fn lsp_saved(&mut self, path: &str) {
        if self.lsp.as_ref().is_some_and(|l| l.client.ready) {
            self.lsp_sync_all();
            if let Some(l) = self.lsp.as_mut() {
                l.client.saved(path);
            }
            self.flush_protocols();
        }
    }

    /// Where the cursor is, as the server names it.
    fn asked_here(&self) -> Option<Asked> {
        let doc = self.active_doc()?;
        if !doc.is_rust() {
            return None;
        }
        let l = self.lsp.as_ref()?;
        let p = self.panes.get(self.active)?.cur.pos;
        let line = doc.buf.line(p.line);
        Some(Asked {
            path: doc.path.clone(),
            line: p.line,
            col: l.client.to_lsp(line, p.col),
        })
    }

    fn lsp_send<F: FnOnce(&mut Lsp, Asked)>(&mut self, f: F) -> bool {
        if !self.lsp_ready_or_say() {
            return false;
        }
        let Some(at) = self.asked_here() else {
            self.fail("Put the cursor in a Rust file first");
            return false;
        };
        self.lsp_sync_all();
        if let Some(l) = self.lsp.as_mut() {
            f(&mut l.client, at);
        }
        self.flush_protocols();
        true
    }

    pub fn lsp_complete(&mut self, trigger: Option<char>) {
        if self.lsp.as_ref().is_some_and(|l| l.asking_completion) {
            return;
        }
        if self.lsp_send(|c, at| {
            c.completion(at, trigger);
        }) {
            if let Some(l) = self.lsp.as_mut() {
                l.asking_completion = true;
            }
        }
    }

    pub fn lsp_hover(&mut self) {
        self.lsp_send(|c, at| {
            c.hover(at);
        });
    }

    /// The mouse rested over the code at `pos` in the active file.
    pub fn hover_at(&mut self, pane: usize, pos: Pos) {
        if !self.lsp.as_ref().is_some_and(|l| l.client.ready) {
            return;
        }
        let Some(doc) = self
            .panes
            .get(pane)
            .and_then(|p| p.doc)
            .and_then(|d| self.docs.get(d))
        else {
            return;
        };
        if !doc.is_rust() {
            return;
        }
        let line = doc.buf.line(pos.line);
        // Only over a word: resting on blank space asks nothing.
        if !line
            .get(pos.col..)
            .and_then(|r| r.chars().next())
            .is_some_and(is_ident)
        {
            return;
        }
        let path = doc.path.clone();
        let Some(l) = self.lsp.as_mut() else { return };
        let col = l.client.to_lsp(line, pos.col);
        self.lsp_sync_all();
        if let Some(l) = self.lsp.as_mut() {
            l.client.hover(Asked {
                path,
                line: pos.line,
                col,
            });
        }
        self.hover_from_mouse = true;
        self.flush_protocols();
    }

    pub fn lsp_definition(&mut self) {
        self.lsp_send(|c, at| {
            c.definition(at);
        });
    }

    pub fn lsp_references(&mut self) {
        if self.lsp_send(|c, at| {
            c.references(at);
        }) {
            self.say("Finding references...");
        }
    }

    pub fn lsp_actions(&mut self) {
        self.lsp_send(|c, at| {
            c.code_actions(at);
        });
    }

    pub fn lsp_rename(&mut self, name: &str) {
        let name = String::from(name);
        self.lsp_send(move |c, at| {
            c.rename(at, &name);
        });
    }

    fn word_at_cursor(&self) -> Option<String> {
        let doc = self.active_doc()?;
        let p = self.panes.get(self.active)?.cur.pos;
        let line = doc.buf.line(p.line);
        let mut a = p.col.min(line.len());
        while a > 0 {
            let prev = line[..a].chars().next_back()?;
            if !is_ident(prev) {
                break;
            }
            a -= prev.len_utf8();
        }
        let mut b = p.col.min(line.len());
        while let Some(c) = line[b..].chars().next() {
            if !is_ident(c) {
                break;
            }
            b += c.len_utf8();
        }
        if a == b {
            None
        } else {
            Some(String::from(&line[a..b]))
        }
    }

    fn lsp_event(&mut self, e: LspEvent) {
        match e {
            LspEvent::Ready => {
                self.lsp_busy = None;
                self.lsp_sync_all();
                self.log(
                    "rust-analyzer is ready: suggestions, K for docs, gd to jump, F2 to rename",
                );
            }
            LspEvent::Diagnostics { path, items } => {
                if let Some(p) = path {
                    // Character columns: the server's were converted with
                    // the text it has, which is the editor's.
                    if items.is_empty() {
                        self.lsp_problems.remove(&p);
                    } else {
                        self.lsp_problems.insert(p, items);
                    }
                    self.refresh_problems();
                }
            }
            LspEvent::Progress(p) => self.lsp_busy = p,
            LspEvent::Say(s) => {
                self.log(&s);
                self.fail(&s);
            }
            LspEvent::Completion { at, items } => {
                if let Some(l) = self.lsp.as_mut() {
                    l.asking_completion = false;
                }
                self.completion_arrived(at, items);
            }
            LspEvent::Hover { at, text } => {
                let from_mouse = core::mem::take(&mut self.hover_from_mouse);
                if text.trim().is_empty() {
                    if !from_mouse {
                        self.say("Nothing to show here");
                    }
                    self.hover = None;
                    return;
                }
                let col = self.col_from_server(&at.path, at.line, at.col);
                self.hover = Some(Hover {
                    text,
                    path: at.path,
                    line: at.line,
                    col,
                    from_mouse,
                });
            }
            LspEvent::Definition { found, .. } => match found.first() {
                None => self.say("No definition found here"),
                Some(loc) => {
                    let loc = loc.clone();
                    self.go_to_location(&loc);
                }
            },
            LspEvent::References { found, .. } => {
                if found.is_empty() {
                    self.say("No references found");
                    return;
                }
                let mut picks = Vec::new();
                let mut labels = Vec::new();
                for loc in &found {
                    let Some(path) = &loc.path else { continue };
                    let mut label = path.clone();
                    label.push(':');
                    label.push_str(&text::num(loc.line + 1));
                    if let Some(t) = self.line_of(path, loc.line) {
                        label.push_str("  ");
                        label.push_str(t.trim());
                    }
                    labels.push(label);
                    picks.push(Pick::Place {
                        path: path.clone(),
                        line: loc.line,
                        col: loc.col,
                    });
                }
                let mut title = text::num(picks.len());
                title.push_str(if picks.len() == 1 {
                    " reference"
                } else {
                    " references"
                });
                self.open_pick(title, labels, picks);
            }
            LspEvent::Rename { edits, .. } => {
                if edits.is_empty() {
                    self.say("Nothing to rename here");
                    return;
                }
                let (places, files) = self.apply_file_edits(&edits);
                let mut m = String::from("Renamed in ");
                m.push_str(&text::num(places));
                m.push_str(if places == 1 { " place" } else { " places" });
                m.push_str(" across ");
                m.push_str(&text::num(files));
                m.push_str(if files == 1 { " file" } else { " files" });
                m.push_str(" -- :wa saves them, u undoes each file");
                self.say(&m);
            }
            LspEvent::Signature { at, label, active } => {
                if label.is_empty() {
                    self.signature = None;
                } else if self.vim.mode == Mode::Insert {
                    self.signature = Some(Signature {
                        label,
                        active,
                        path: at.path,
                        line: at.line,
                    });
                }
            }
            LspEvent::Actions { actions, .. } => {
                if actions.is_empty() {
                    self.say("No quick fixes here");
                    return;
                }
                let labels = actions.iter().map(|a| a.title.clone()).collect();
                let picks = actions.into_iter().map(Pick::Action).collect();
                self.open_pick(String::from("Quick fixes and code actions"), labels, picks);
            }
        }
    }

    fn line_of(&mut self, path: &str, line: usize) -> Option<String> {
        if let Some(d) = self.docs.iter().find(|d| d.path == path) {
            return Some(String::from(d.buf.line(line.min(d.buf.last_line()))));
        }
        let t = self.project.as_mut()?.fs.read(path).ok()?;
        t.split('\n').nth(line).map(String::from)
    }

    /// A server column on a line of `path` as a byte column.
    fn col_from_server(&mut self, path: &str, line: usize, col: usize) -> usize {
        let text = self.line_of(path, line).unwrap_or_default();
        match &self.lsp {
            Some(l) => l.client.from_lsp(&text, col),
            None => col,
        }
    }

    fn open_pick(&mut self, title: String, labels: Vec<String>, picks: Vec<Pick>) {
        let mut st = PaletteState {
            mode: PaletteMode::Pick,
            query: String::new(),
            items: Vec::new(),
            files: labels,
            selected: 0,
            picks,
            title,
        };
        Self::refilter(&mut st);
        self.palette = Some(st);
    }

    pub(crate) fn pick_chosen(&mut self, p: Pick) {
        match p {
            Pick::Place { path, line, col } => {
                let loc = Location {
                    path: Some(path),
                    outside: String::new(),
                    line,
                    col,
                    end_line: line,
                    end_col: col,
                };
                self.go_to_location(&loc);
            }
            Pick::Action(a) => {
                let (places, _) = self.apply_file_edits(&a.edits);
                let mut m = String::from(a.title.as_str());
                m.push_str(if places > 0 {
                    " -- done (u undoes it)"
                } else {
                    ""
                });
                self.say(&m);
            }
        }
    }

    fn go_to_location(&mut self, loc: &Location) {
        let Some(path) = &loc.path else {
            let mut m = String::from("Defined in ");
            m.push_str(&loc.outside);
            m.push(':');
            m.push_str(&text::num(loc.line + 1));
            m.push_str(" -- outside the project folder, so it does not open here");
            self.say(&m);
            return;
        };
        if !self.open_file(path) {
            return;
        }
        let col = self.col_from_server(path, loc.line, loc.col);
        let a = self.active;
        if let (Some(di), Some(p)) = (self.active_doc_index(), self.panes.get_mut(a)) {
            let pos = self.docs[di].buf.clamp(Pos::new(loc.line, col));
            p.cur.pos = pos;
            p.cur.anchor = pos;
            self.vim.sync_want(&self.docs[di].buf, &mut p.cur);
        }
        self.focus = Focus::Editor;
        self.ensure_visible(self.active);
        let rows = self.panes.get(a).map(|p| p.rows).unwrap_or(20);
        if let Some(p) = self.panes.get_mut(a) {
            // Land in the middle, like `zz`.
            p.top = (loc.line as f32 - (rows / 2) as f32).max(0.0);
        }
    }

    /// Apply edits from the server to files, opening the ones not open.
    /// Each file's edits are one undoable change. Returns (edits, files).
    fn apply_file_edits(&mut self, edits: &[FileEdit]) -> (usize, usize) {
        let mut places = 0;
        let mut files = 0;
        let back = self.active_doc_index();
        let pane = self.panes.get(self.active).copied();
        for fe in edits {
            let Some(path) = &fe.path else { continue };
            if fe.edits.is_empty() {
                continue;
            }
            let d = match self.docs.iter().position(|d| &d.path == path) {
                Some(d) => d,
                None => {
                    if !self.open_file(path) {
                        continue;
                    }
                    match self.docs.iter().position(|d| &d.path == path) {
                        Some(d) => d,
                        None => continue,
                    }
                }
            };
            places += self.apply_edits_to(d, &fe.edits);
            files += 1;
        }
        // Back to where the person was.
        if let (Some(b), Some(p)) = (back, pane) {
            if self.active_doc_index() != Some(b) {
                if let Some(pm) = self.panes.get_mut(self.active) {
                    pm.doc = Some(b);
                    pm.cur = p.cur;
                    pm.top = p.top;
                    pm.left = p.left;
                }
            }
        }
        self.after_edit();
        (places, files)
    }

    /// Server edits on one document, last first so earlier places hold.
    fn apply_edits_to(&mut self, d: usize, edits: &[TextEdit]) -> usize {
        let utf8 = self.lsp.as_ref().is_none_or(|l| l.client.utf8);
        let mut sorted: Vec<&TextEdit> = edits.iter().collect();
        sorted.sort_by_key(|e| core::cmp::Reverse((e.line, e.col)));
        let Some(doc) = self.docs.get_mut(d) else {
            return 0;
        };
        let cursor = self
            .panes
            .iter()
            .find(|p| p.doc == Some(d))
            .map(|p| p.cur.pos)
            .unwrap_or_default();
        let was_open = doc.buf.group_open();
        if was_open {
            doc.buf.end_group(cursor);
        }
        doc.buf.begin_group(cursor);
        let conv = |line: &str, col: usize| -> usize {
            if utf8 {
                let mut c = col.min(line.len());
                while c > 0 && !line.is_char_boundary(c) {
                    c -= 1;
                }
                c
            } else {
                let mut units = 0;
                for (i, ch) in line.char_indices() {
                    if units >= col {
                        return i;
                    }
                    units += ch.len_utf16();
                }
                line.len()
            }
        };
        let mut n = 0;
        for e in sorted {
            let last = doc.buf.last_line();
            let (l0, l1) = (e.line.min(last), e.end_line.min(last));
            let a = Pos::new(l0, conv(doc.buf.line(l0), e.col));
            let b = if e.end_line > last {
                doc.buf.end()
            } else {
                Pos::new(l1, conv(doc.buf.line(l1), e.end_col))
            };
            doc.buf.replace(a, b, &e.text);
            n += 1;
        }
        doc.buf.end_group(cursor);
        if was_open {
            doc.buf.begin_group(cursor);
        }
        doc.sync();
        n
    }

    // ---- completion --------------------------------------------------------

    fn completion_arrived(&mut self, at: Asked, items: Vec<CompletionItem>) {
        let Some(doc) = self.active_doc() else { return };
        let p = self.panes[self.active].cur.pos;
        if doc.path != at.path || p.line != at.line || self.vim.mode != Mode::Insert {
            return;
        }
        if items.is_empty() {
            self.completion = None;
            return;
        }
        let line = doc.buf.line(p.line);
        let mut start = p.col.min(line.len());
        while start > 0 {
            let Some(c) = line[..start].chars().next_back() else {
                break;
            };
            if !is_ident(c) {
                break;
            }
            start -= c.len_utf8();
        }
        let mut c = Completion {
            items,
            shown: Vec::new(),
            selected: 0,
            path: at.path,
            line: at.line,
            start,
        };
        self.filter_completion(&mut c);
        self.completion = if c.shown.is_empty() { None } else { Some(c) };
    }

    fn filter_completion(&self, c: &mut Completion) {
        let Some(doc) = self.active_doc() else { return };
        let p = self.panes[self.active].cur.pos;
        let line = doc.buf.line(p.line);
        let typed = line.get(c.start..p.col.min(line.len())).unwrap_or("");
        let lower = typed.to_lowercase();
        let mut scored: Vec<(u8, &str, usize)> = c
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, it)| {
                let f = it.filter.to_lowercase();
                let rank = if lower.is_empty() || f.starts_with(&lower) {
                    0
                } else if text::fuzzy_score(&lower, &f).is_some() {
                    1
                } else {
                    return None;
                };
                Some((rank, it.sort.as_str(), i))
            })
            .collect();
        scored.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(b.1)).then(a.2.cmp(&b.2)));
        c.shown = scored.into_iter().map(|s| s.2).take(200).collect();
        if c.selected >= c.shown.len() {
            c.selected = 0;
        }
    }

    fn accept_completion(&mut self) {
        let Some(c) = self.completion.take() else {
            return;
        };
        let Some(&i) = c.shown.get(c.selected) else {
            return;
        };
        let item = c.items[i].clone();
        let utf8 = self.lsp.as_ref().is_none_or(|l| l.client.utf8);
        let a = self.active;
        let Some(d) = self.active_doc_index() else {
            return;
        };
        let (docs, panes) = (&mut self.docs, &mut self.panes);
        let (Some(doc), Some(pane)) = (docs.get_mut(d), panes.get_mut(a)) else {
            return;
        };
        let cur = pane.cur.pos;
        let line = String::from(doc.buf.line(cur.line));
        let conv = |col: usize| -> usize {
            if utf8 {
                col.min(line.len())
            } else {
                let mut units = 0;
                for (i, ch) in line.char_indices() {
                    if units >= col {
                        return i;
                    }
                    units += ch.len_utf16();
                }
                line.len()
            }
        };
        let (from, text_in) = match &item.edit {
            Some(e) if e.line == cur.line => (conv(e.col).min(cur.col), e.text.clone()),
            _ => (c.start.min(cur.col), item.insert.clone()),
        };
        // Everything typed since the word began is replaced.
        let end = doc.buf.replace(Pos::new(cur.line, from), cur, &text_in);
        pane.cur.pos = end;
        // Extra edits (an import at the top) keep the cursor on its text.
        let mut extra = item.extra.clone();
        extra.sort_by_key(|e| core::cmp::Reverse((e.line, e.col)));
        for e in extra {
            let last = doc.buf.last_line();
            let l0 = e.line.min(last);
            let l1 = e.end_line.min(last);
            let a0 = Pos::new(l0, e.col.min(doc.buf.line_len(l0)));
            let b0 = Pos::new(l1, e.end_col.min(doc.buf.line_len(l1)));
            let before = doc.buf.line_count();
            doc.buf.replace(a0, b0, &e.text);
            if l0 < pane.cur.pos.line {
                let delta = doc.buf.line_count() as isize - before as isize;
                pane.cur.pos.line = (pane.cur.pos.line as isize + delta).max(0) as usize;
            }
        }
        pane.cur.pos = doc.buf.clamp(pane.cur.pos);
        doc.sync();
        self.after_edit();
        // A function call: ask what goes in the brackets.
        if text_in.ends_with('(') || text_in.ends_with("()") {
            self.lsp_send(|c, at| {
                c.signature(at);
            });
        }
    }

    // ---- keys the language server and debugger take ------------------------

    /// Keys taken before the modal engine sees them. True when taken.
    pub(crate) fn ide_intercept(&mut self, input: &Input) -> bool {
        // Any key closes a hover box (and is still a key).
        if self.hover.is_some() {
            self.hover = None;
            if *input == Input::Key(Key::Esc) {
                return true;
            }
        }
        let lsp_on = self.lsp.as_ref().is_some_and(|l| l.client.ready);
        match self.vim.mode {
            Mode::Insert if self.completion.is_some() => match input {
                Input::Key(Key::Tab) | Input::Key(Key::Enter) => {
                    self.accept_completion();
                    return true;
                }
                Input::Key(Key::Down) | Input::Ctrl('n') => {
                    if let Some(c) = self.completion.as_mut() {
                        c.selected = (c.selected + 1) % c.shown.len().max(1);
                    }
                    return true;
                }
                Input::Key(Key::Up) | Input::Ctrl('p') => {
                    if let Some(c) = self.completion.as_mut() {
                        let n = c.shown.len().max(1);
                        c.selected = (c.selected + n - 1) % n;
                    }
                    return true;
                }
                Input::Key(Key::Esc) => {
                    self.completion = None;
                    self.signature = None;
                }
                _ => {}
            },
            Mode::Normal if lsp_on => {
                let g = [Input::Char('g')];
                if *input == Input::Char('K') && self.vim.pending_is(&[]) {
                    self.lsp_hover();
                    return true;
                }
                if self.vim.pending_is(&g) {
                    match input {
                        Input::Char('d') => {
                            self.vim.clear_pending();
                            self.lsp_definition();
                            return true;
                        }
                        Input::Char('r') => {
                            self.vim.clear_pending();
                            self.lsp_references();
                            return true;
                        }
                        Input::Char('a') => {
                            self.vim.clear_pending();
                            self.lsp_actions();
                            return true;
                        }
                        _ => {}
                    }
                }
            }
            Mode::Normal if *input == Input::Char('K') && self.vim.pending_is(&[]) => {
                self.lsp_ready_or_say();
                return true;
            }
            _ => {}
        }
        if self.vim.mode != Mode::Insert {
            self.signature = None;
        }
        false
    }

    /// After the engine handled a key: completion and signature help.
    pub(crate) fn ide_after_input(&mut self, input: &Input, was_insert: bool) {
        if self.vim.mode != Mode::Insert {
            self.completion = None;
            self.signature = None;
            return;
        }
        if !self.lsp.as_ref().is_some_and(|l| l.client.ready) {
            return;
        }
        let triggers: Vec<char> = self
            .lsp
            .as_ref()
            .map(|l| l.client.triggers.clone())
            .unwrap_or_default();
        let before = self.char_before_cursor(1);
        match input {
            Input::Char(c) if is_ident(*c) && was_insert => {
                match self.completion.take() {
                    Some(mut comp) => {
                        self.filter_completion(&mut comp);
                        if comp.shown.is_empty() {
                            // Nothing left that fits: ask again from here.
                            self.lsp_complete(None);
                        } else {
                            self.completion = Some(comp);
                        }
                    }
                    None => {
                        // The first letter of a word asks.
                        let prev = self.char_before_cursor(2);
                        if !prev.is_some_and(is_ident) {
                            self.lsp_complete(None);
                        }
                    }
                }
            }
            Input::Char(c) if triggers.contains(c) => {
                self.completion = None;
                // `:` asks only as `::`.
                if *c != ':' || self.char_before_cursor(2) == Some(':') {
                    self.lsp_complete(Some(*c));
                }
                let _ = before;
            }
            Input::Char('(') | Input::Char(',') => {
                self.completion = None;
                self.lsp_send(|c, at| {
                    c.signature(at);
                });
            }
            Input::Char(')') => {
                self.completion = None;
                self.signature = None;
            }
            Input::Key(Key::Backspace) => {
                if let Some(mut comp) = self.completion.take() {
                    let p = self.panes[self.active].cur.pos;
                    if p.col > comp.start && p.line == comp.line {
                        self.filter_completion(&mut comp);
                        if !comp.shown.is_empty() {
                            self.completion = Some(comp);
                        }
                    }
                }
            }
            Input::Char(_) | Input::Key(_) | Input::Paste(_) => {
                self.completion = None;
            }
            _ => {}
        }
    }

    /// The `n`th character before the cursor (1 is the one just typed).
    fn char_before_cursor(&self, n: usize) -> Option<char> {
        let doc = self.active_doc()?;
        let p = self.panes.get(self.active)?.cur.pos;
        let line = doc.buf.line(p.line);
        line.get(..p.col.min(line.len()))?.chars().rev().nth(n - 1)
    }

    // ---- the debugger --------------------------------------------------------

    fn start_debug(&mut self) {
        if let Some(d) = self.dap.as_mut() {
            if d.client.state != DapState::Ended {
                self.say("Already debugging (Shift+F5 stops)");
                return;
            }
        }
        if self.missing.contains("lldb-dap") && self.missing.contains("xcrun") {
            self.debugger_missing();
            return;
        }
        self.start_cargo(Tool::Debug);
        if !self.jobs.is_empty() {
            self.debug_after_build = true;
            self.say("Building for the debugger...");
        }
    }

    fn debugger_missing(&mut self) {
        let mut m = String::from("No debug adapter (lldb-dap) is installed on this computer. ");
        m.push_str(tools::install_hint("lldb-dap"));
        self.log(&m);
        self.show_panel(PanelTab::Output);
        self.fail(&m);
    }

    fn breakpoint_map(&self) -> BTreeMap<String, Vec<usize>> {
        let mut m = BTreeMap::new();
        for d in &self.docs {
            if !d.breakpoints.is_empty() {
                m.insert(d.path.clone(), d.breakpoints.iter().copied().collect());
            }
        }
        m
    }

    fn launch_debugger(&mut self) {
        let Some(exe) = self.last_exe.clone() else {
            self.fail("The build made no program to debug (is this a library?)");
            return;
        };
        let Some(root) = self.project_root() else {
            self.fail("cargo could not say where the project is, so the debugger cannot start");
            return;
        };
        // lldb-dap on the PATH, else Apple's through xcrun.
        let mut job = None;
        for (program, args) in [
            ("lldb-dap", alloc::vec![]),
            ("xcrun", alloc::vec![String::from("lldb-dap")]),
        ] {
            if self.missing.contains(program) {
                continue;
            }
            match self.runner.start(program, &args, None) {
                Ok(j) => {
                    job = Some(j);
                    break;
                }
                Err(RunError::Missing(_)) => {
                    self.missing.insert(String::from(program));
                }
                Err(e) => {
                    let m = words_for(program, &e);
                    self.fail(&m);
                    return;
                }
            }
        }
        let Some(job) = job else {
            self.debugger_missing();
            return;
        };
        let bps = self.breakpoint_map();
        let n: usize = bps.values().map(Vec::len).sum();
        self.dap = Some(DapSession {
            job,
            client: Dap::new(&root, &exe, bps),
        });
        self.flush_protocols();
        self.show_panel(PanelTab::Debug);
        self.log("Debugging -- F5 continue, F10 step over, F11 step into, Shift+F11 step out, Shift+F5 stop");
        let mut m = String::from("Debugging");
        if n == 0 {
            m.push_str(" -- no breakpoints yet (F9 on a line sets one)");
        }
        self.say(&m);
    }

    fn debug_command(&mut self, t: Tool) {
        let Some(d) = self.dap.as_mut() else {
            if t == Tool::StopDebug {
                self.say("Not debugging");
            } else {
                self.say("Not debugging -- Shift+F5 builds and starts the debugger");
            }
            return;
        };
        let stopped = d.client.state == DapState::Stopped;
        let job = d.job;
        match t {
            Tool::Continue => d.client.resume(),
            Tool::StepOver => d.client.step_over(),
            Tool::StepIn => d.client.step_in(),
            Tool::StepOut => d.client.step_out(),
            Tool::StopDebug => d.client.stop(),
            _ => {}
        }
        self.flush_protocols();
        if t == Tool::StopDebug {
            // The program goes with the session either way.
            self.runner.kill(job);
        } else if !stopped {
            self.say("The program is running -- it stops at a breakpoint");
        }
    }

    pub(crate) fn dap_breakpoints_changed(&mut self, doc: usize) {
        let Some(d) = self.docs.get(doc) else { return };
        let path = d.path.clone();
        let lines: Vec<usize> = d.breakpoints.iter().copied().collect();
        if let Some(s) = self.dap.as_mut() {
            s.client.set_breakpoints(&path, lines);
        }
        self.flush_protocols();
    }

    fn dap_event(&mut self, e: DapEvent) {
        match e {
            DapEvent::Stopped { reason } => {
                let frame = self.dap.as_ref().and_then(|d| d.client.current().cloned());
                let mut m = String::from("Stopped");
                if !reason.is_empty() {
                    m.push_str(" (");
                    m.push_str(&reason);
                    m.push(')');
                }
                if let Some(f) = frame {
                    m.push_str(" in ");
                    m.push_str(&f.name);
                    if let Some(p) = &f.path {
                        let loc = Location {
                            path: Some(p.clone()),
                            outside: String::new(),
                            line: f.line,
                            col: 0,
                            end_line: f.line,
                            end_col: 0,
                        };
                        self.go_to_location(&loc);
                    }
                }
                m.push_str(" -- F10 step over, F11 into, F5 continue");
                self.show_panel(PanelTab::Debug);
                self.say(&m);
            }
            DapEvent::Continued | DapEvent::Refreshed => {}
            DapEvent::Output(t) => self.output.write_text(&t),
            DapEvent::Ended(code) => {
                let mut m = String::from("The program ended");
                if let Some(c) = code {
                    m.push_str(" with exit code ");
                    if c < 0 {
                        m.push('-');
                    }
                    m.push_str(&text::num(c.unsigned_abs() as usize));
                }
                self.log(&m);
                self.say(&m);
                if let Some(d) = self.dap.take() {
                    self.runner.kill(d.job);
                    self.runner.release(d.job);
                }
                if self.panel_tab == PanelTab::Debug {
                    self.panel_tab = PanelTab::Output;
                }
            }
            DapEvent::Say(s) => {
                self.log(&s);
                self.fail(&s);
            }
        }
    }

    /// The line the debugger is stopped on in `path`, if it is.
    pub fn debug_line(&self, path: &str) -> Option<usize> {
        let d = self.dap.as_ref()?;
        if d.client.state != DapState::Stopped {
            return None;
        }
        let f = d.client.current()?;
        (f.path.as_deref() == Some(path)).then_some(f.line)
    }

    /// Choose a frame of the call stack (the panel's left column).
    pub fn debug_select_frame(&mut self, i: usize) {
        let frame = self.dap.as_mut().and_then(|d| {
            d.client.select_frame(i);
            d.client.current().cloned()
        });
        if let Some(f) = frame {
            if let Some(p) = f.path {
                let loc = Location {
                    path: Some(p),
                    outside: String::new(),
                    line: f.line,
                    col: 0,
                    end_line: f.line,
                    end_col: 0,
                };
                self.go_to_location(&loc);
                self.focus = Focus::Panel;
            }
        }
        self.flush_protocols();
    }

    pub fn debug_toggle_var(&mut self, i: usize) {
        if let Some(d) = self.dap.as_mut() {
            d.client.toggle_var(i);
            self.var_selected = i;
        }
        self.flush_protocols();
    }

    pub(crate) fn debug_panel_key(&mut self, input: Input) {
        let n = self.dap.as_ref().map(|d| d.client.vars.len()).unwrap_or(0);
        match input {
            Input::Char('j') | Input::Key(Key::Down) => {
                if n > 0 {
                    self.var_selected = (self.var_selected + 1).min(n - 1);
                }
            }
            Input::Char('k') | Input::Key(Key::Up) => {
                self.var_selected = self.var_selected.saturating_sub(1)
            }
            Input::Key(Key::Enter) | Input::Char('l') | Input::Char('h') => {
                let i = self.var_selected;
                self.debug_toggle_var(i);
            }
            Input::Char('c') => self.tool(Tool::Continue),
            Input::Char('n') => self.tool(Tool::StepOver),
            Input::Char('s') => self.tool(Tool::StepIn),
            _ => self.focus = Focus::Editor,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json;
    use crate::project::MemFs;
    use crate::rpc;
    use crate::tools::fake::FakeRunner;
    use crate::tools::{Finished, JobEvent, Source};
    use crate::vim::parse_keys;

    /// A project "on disk" (the fake runner says it has a folder).
    fn ws() -> (Workspace, FakeRunner) {
        let mut w = Workspace::new();
        let r = FakeRunner::new();
        w.runner = Box::new(r.clone());
        let mut m = MemFs::new();
        for (p, t) in project::tour_files() {
            let _ = project::Vfs::write(&mut m, p, t);
        }
        w.open_project("tour", Box::new(m));
        (w, r)
    }

    fn keys(w: &mut Workspace, k: &str) {
        for i in parse_keys(k) {
            w.input(i);
        }
    }

    const ERR: &str = r#"{"reason":"compiler-message","message":{"rendered":"\u001b[1m\u001b[91merror[E0308]\u001b[0m: mismatched types\n --> src/main.rs:2:18\n","level":"error","message":"mismatched types","code":{"code":"E0308"},"spans":[{"file_name":"src/main.rs","line_start":2,"column_start":18,"line_end":2,"column_end":20,"is_primary":true}]}}"#;

    #[test]
    fn build_streams_output_and_fills_problems() {
        let (mut w, r) = ws();
        w.now_ms = 1000;
        keys(&mut w, "ggOx<Esc>");
        w.run(crate::palette::Action::Tool(Tool::Build));
        let (job, program, args) = r.last().unwrap();
        assert_eq!(program, "cargo");
        assert_eq!(args[0], "build");
        assert!(w.dirty_count() == 0, "changes are saved before a build");
        assert_eq!(w.panel_tab, PanelTab::Output);
        assert!(w.running_label().unwrap().starts_with("Building"));
        // stderr and JSON arrive in pieces.
        r.push(JobEvent::Output {
            job,
            bytes: b"   \x1b[32mCompiling\x1b[0m tour v0.1.0\n".to_vec(),
            stderr: true,
        });
        let (a, b) = ERR.split_at(40);
        r.out(job, a);
        w.poll_runner();
        assert!(w.problems.is_empty(), "half a message is not a problem yet");
        r.out(job, b);
        r.out(job, "\n{\"reason\":\"build-finished\",\"success\":false}\n");
        w.now_ms = 3400;
        r.exit(job, 101);
        w.poll_runner();
        assert_eq!(w.problems.len(), 1);
        assert_eq!(w.problems[0].code.as_deref(), Some("E0308"));
        assert_eq!(w.panel_tab, PanelTab::Problems);
        assert!(w.jobs.is_empty());
        let out = w.output_lines().join("\n");
        assert!(out.contains("Compiling tour"), "{out}");
        assert!(out.contains("error[E0308]: mismatched types"), "{out}");
        assert!(
            w.message().unwrap().0.contains("1 error"),
            "{:?}",
            w.message()
        );
        // Enter on the problem opens its place.
        w.focus = Focus::Panel;
        w.input(Input::Key(Key::Enter));
        let p = w.panes[w.active].cur.pos;
        assert_eq!((p.line, p.col), (1, 17));
        assert_eq!(w.focus, Focus::Editor);
    }

    #[test]
    fn stop_and_missing_cargo_say_so() {
        let (mut w, r) = ws();
        w.run(crate::palette::Action::Tool(Tool::Run));
        let (job, ..) = r.last().unwrap();
        w.run(crate::palette::Action::Tool(Tool::Stop));
        assert_eq!(r.0.borrow().killed, alloc::vec![job]);
        r.push(JobEvent::Exited {
            job,
            code: None,
            signal: Some(9),
        });
        w.poll_runner();
        assert_eq!(w.message().unwrap().0, "Stopped");
        r.0.borrow_mut().missing.push(String::from("cargo"));
        w.run(crate::palette::Action::Tool(Tool::Build));
        let (m, err) = w.message().unwrap();
        assert!(err);
        assert!(
            m.contains("cargo is not installed") && m.contains("rustup.rs"),
            "{m}"
        );
    }

    #[test]
    fn run_output_and_test_results_reach_the_output_panel() {
        let (mut w, r) = ws();
        w.run(crate::palette::Action::Tool(Tool::Test));
        let (job, ..) = r.last().unwrap();
        r.out(job, "running 1 test\ntest it_works ... ok\n");
        r.exit(job, 0);
        w.poll_runner();
        assert!(w.output_lines().iter().any(|l| l == "test it_works ... ok"));
        assert!(w.message().unwrap().0.starts_with("Tests passed"));
    }

    #[test]
    fn format_replaces_the_text_as_one_undo_step() {
        let (mut w, r) = ws();
        let before = w.active_doc().unwrap().buf.text();
        let formatted = before.replacen("fn main()", "fn main( )", 1);
        r.0.borrow_mut().finished.push((
            String::from("rustfmt"),
            Finished {
                code: Some(0),
                stdout: formatted.clone().into_bytes(),
                stderr: Vec::new(),
            },
        ));
        w.run(crate::palette::Action::Tool(Tool::Format));
        assert_eq!(w.active_doc().unwrap().buf.text(), formatted);
        let (_, p, a) = r.last().unwrap();
        assert_eq!(p, "rustfmt");
        assert!(a.contains(&String::from("--emit=stdout")));
        keys(&mut w, "u");
        assert_eq!(w.active_doc().unwrap().buf.text(), before);
        // A syntax error leaves the file as it was and says why.
        r.0.borrow_mut().finished.push((
            String::from("rustfmt"),
            Finished {
                code: Some(1),
                stdout: Vec::new(),
                stderr: b"error: expected one of `;` or `}`, found `x`\n".to_vec(),
            },
        ));
        w.run(crate::palette::Action::Tool(Tool::Format));
        assert_eq!(w.active_doc().unwrap().buf.text(), before);
        assert!(w.message().unwrap().0.contains("expected one of"));
    }

    #[test]
    fn save_formats_when_the_setting_is_on() {
        let (mut w, r) = ws();
        keys(&mut w, "ggOfn   x(){}<Esc>");
        let tidy = w
            .active_doc()
            .unwrap()
            .buf
            .text()
            .replace("fn   x(){}", "fn x() {}");
        r.0.borrow_mut().finished.push((
            String::from("rustfmt"),
            Finished {
                code: Some(0),
                stdout: tidy.clone().into_bytes(),
                stderr: Vec::new(),
            },
        ));
        keys(&mut w, ":w<CR>");
        assert_eq!(w.active_doc().unwrap().buf.text(), tidy);
        assert!(!w.active_doc().unwrap().buf.modified());
        let saved =
            project::Vfs::read(&mut *w.project.as_mut().unwrap().fs, "src/main.rs").unwrap();
        assert_eq!(saved, tidy);
        // Off: saved as typed.
        w.run(crate::palette::Action::FormatOnSave);
        assert!(!w.settings.format_on_save);
        keys(&mut w, "ggOlet  y=1;<Esc>:w<CR>");
        assert!(w.active_doc().unwrap().buf.line(0).contains("let  y=1;"));
    }

    #[test]
    fn replacing_text_keeps_unchanged_lines() {
        let (mut w, _r) = ws();
        let d = w.active_doc_index().unwrap();
        for (old, new) in [
            ("a\nb\nc", "a\nB\nc"),
            ("a\nb\nc", "a\nc"),
            ("a\nb\nc", "a\nb\nc\nd"),
            ("a\nb\nc", "z\na\nb\nc"),
            ("a\nb\nc", "a"),
            ("a\nb", "x\ny\nz"),
            ("", "fn x() {}\n"),
        ] {
            w.docs[d] = crate::workspace::Doc::new("src/main.rs", old);
            w.replace_doc_text(d, new);
            assert_eq!(w.docs[d].buf.text(), new, "{old:?} -> {new:?}");
            assert!(w.docs[d].buf.undo().is_some());
            assert_eq!(w.docs[d].buf.text(), old, "undo {old:?}");
        }
    }

    #[test]
    fn terminals_take_keys_and_follow_the_panel() {
        let (mut w, r) = ws();
        w.term_resize(80, 20);
        w.run(crate::palette::Action::Tool(Tool::Terminal));
        assert_eq!(w.terminals.len(), 1);
        assert!(w.terminal_focused());
        let job = w.terminals[0].job;
        assert!(r.0.borrow().resized.contains(&(job, 80, 20)));
        r.out(job, "\x1b[01;34msrc\x1b[0m  Cargo.toml\r\n$ ");
        w.poll_runner();
        assert_eq!(w.terminals[0].vt.screen_text()[0], "src  Cargo.toml");
        w.term_text("ls");
        assert!(w.term_key("Enter", false, false, false));
        assert!(w.term_key("c", false, true, false));
        assert_eq!(r.written_to(job), b"ls\r\x03");
        // A program asking where the cursor is gets an answer.
        r.clear_written();
        r.out(job, "\x1b[6n");
        w.poll_runner();
        assert_eq!(r.written_to(job), b"\x1b[2;3R");
        w.term_resize(60, 10);
        assert!(r.0.borrow().resized.contains(&(job, 60, 10)));
        assert_eq!(w.terminals[0].vt.cols, 60);
        // A second terminal is a second tab.
        w.run(crate::palette::Action::Tool(Tool::Terminal));
        assert_eq!(w.terminals.len(), 2);
        assert_eq!(w.term_active, 1);
        assert_eq!(w.terminals[1].n, 2);
        // Escape twice quickly goes back to the editor.
        w.now_ms = 10_000;
        w.term_key("Escape", false, false, false);
        assert!(w.terminal_focused());
        w.now_ms = 10_200;
        w.term_key("Escape", false, false, false);
        assert!(!w.terminal_focused());
        assert_eq!(w.focus, Focus::Editor);
        // The shell ending says so; the tab stays to read.
        let j2 = w.terminals[1].job;
        r.exit(j2, 0);
        w.poll_runner();
        assert!(w.terminals[1].ended);
        w.close_terminal(1);
        assert_eq!(w.terminals.len(), 1);
    }

    fn lsp_ready(w: &mut Workspace, r: &FakeRunner) -> u64 {
        r.0.borrow_mut().finished.push((
            String::from("cargo"),
            Finished {
                code: Some(0),
                stdout: b"/home/me/tour/Cargo.toml\n".to_vec(),
                stderr: Vec::new(),
            },
        ));
        w.start_lsp(true);
        let job = r.job_of("rust-analyzer").expect("started");
        let init = json::parse(
            r#"{"jsonrpc":"2.0","id":1,"result":{"capabilities":{"positionEncoding":"utf-8"}}}"#,
        )
        .unwrap();
        r.push(JobEvent::Output {
            job,
            bytes: rpc::frame(&init),
            stderr: false,
        });
        w.poll_runner();
        assert!(w.lsp.as_ref().unwrap().client.ready);
        job
    }

    /// The messages the editor sent the server.
    fn to_server(r: &FakeRunner, job: u64) -> Vec<json::Value> {
        let mut f = crate::rpc::Framer::default();
        f.push(&r.written_to(job));
        r.clear_written();
        f.drain()
    }

    fn answer(r: &FakeRunner, job: u64, id: i64, result: &str) {
        let v = json::parse(&alloc::format!(
            r#"{{"jsonrpc":"2.0","id":{id},"result":{result}}}"#
        ))
        .unwrap();
        r.push(JobEvent::Output {
            job,
            bytes: rpc::frame(&v),
            stderr: false,
        });
    }

    #[test]
    fn completion_pops_up_and_tab_takes_it() {
        let (mut w, r) = ws();
        let job = lsp_ready(&mut w, &r);
        let sent = to_server(&r, job);
        assert!(sent
            .iter()
            .any(|m| m.get("method").str() == Some("textDocument/didOpen")));
        keys(&mut w, "ggO    let n = sh");
        let sent = to_server(&r, job);
        let req = sent
            .iter()
            .find(|m| m.get("method").str() == Some("textDocument/completion"))
            .expect("typing a word asks");
        assert!(sent
            .iter()
            .any(|m| m.get("method").str() == Some("textDocument/didChange")));
        let id = req.get("id").int().unwrap();
        answer(
            &r,
            job,
            id,
            r#"{"items":[{"label":"shapes","kind":6,"sortText":"1"},{"label":"show","kind":3,"sortText":"2","textEdit":{"range":{"start":{"line":0,"character":12},"end":{"line":0,"character":14}},"newText":"show()"}},{"label":"other","sortText":"0"}]}"#,
        );
        w.poll_runner();
        let c = w.completion.as_ref().expect("the popup");
        assert_eq!(c.shown.len(), 2, "only what fits `sh`");
        assert_eq!(c.items[c.shown[0]].label, "shapes");
        w.input(Input::Key(Key::Down));
        w.input(Input::Key(Key::Tab));
        assert!(w.completion.is_none());
        assert_eq!(w.active_doc().unwrap().buf.line(0), "    let n = show()");
        assert_eq!(w.vim.mode, Mode::Insert);
        // Typing more filters without asking again.
        keys(&mut w, "<Esc>");
        assert!(w.completion.is_none());
    }

    #[test]
    fn hover_definition_references_rename() {
        let (mut w, r) = ws();
        let job = lsp_ready(&mut w, &r);
        to_server(&r, job);
        keys(&mut w, "K");
        let m = to_server(&r, job);
        let hid = m
            .iter()
            .find(|m| m.get("method").str() == Some("textDocument/hover"))
            .unwrap()
            .get("id")
            .int()
            .unwrap();
        answer(
            &r,
            job,
            hid,
            r#"{"contents":{"kind":"markdown","value":"```rust\nfn main()\n```"}}"#,
        );
        w.poll_runner();
        assert_eq!(w.hover.as_ref().unwrap().text, "fn main()");
        keys(&mut w, "j");
        assert!(w.hover.is_none(), "a key closes it");

        keys(&mut w, "gd");
        let m = to_server(&r, job);
        let did = m
            .iter()
            .find(|m| m.get("method").str() == Some("textDocument/definition"))
            .unwrap()
            .get("id")
            .int()
            .unwrap();
        answer(
            &r,
            job,
            did,
            r#"[{"uri":"file:///home/me/tour/src/shapes.rs","range":{"start":{"line":2,"character":4},"end":{"line":2,"character":9}}}]"#,
        );
        w.poll_runner();
        assert_eq!(w.active_doc().unwrap().path, "src/shapes.rs");
        assert_eq!(w.panes[w.active].cur.pos, Pos::new(2, 4));

        keys(&mut w, "gr");
        let m = to_server(&r, job);
        let rid = m
            .iter()
            .find(|m| m.get("method").str() == Some("textDocument/references"))
            .unwrap()
            .get("id")
            .int()
            .unwrap();
        answer(
            &r,
            job,
            rid,
            r#"[{"uri":"file:///home/me/tour/src/main.rs","range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}}},{"uri":"file:///home/me/tour/src/shapes.rs","range":{"start":{"line":2,"character":4},"end":{"line":2,"character":5}}}]"#,
        );
        w.poll_runner();
        let p = w.palette.as_ref().expect("a list of places");
        assert_eq!(p.title, "2 references");
        w.input(Input::Key(Key::Enter));
        assert_eq!(w.active_doc().unwrap().path, "src/main.rs");

        w.run(crate::palette::Action::Tool(Tool::RenameSymbol));
        assert_eq!(w.prompt.as_ref().unwrap().kind, PromptKind::RenameSymbol);
        w.prompt.as_mut().unwrap().text = String::from("Thing");
        w.prompt_confirm();
        let m = to_server(&r, job);
        let nid = m
            .iter()
            .find(|m| m.get("method").str() == Some("textDocument/rename"))
            .unwrap()
            .get("id")
            .int()
            .unwrap();
        answer(
            &r,
            job,
            nid,
            r#"{"changes":{"file:///home/me/tour/src/main.rs":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":2}},"newText":"Thing"}]}}"#,
        );
        w.poll_runner();
        assert!(
            w.active_doc().unwrap().buf.line(0).starts_with("Thing"),
            "{}",
            w.active_doc().unwrap().buf.line(0)
        );
        assert!(w.message().unwrap().0.starts_with("Renamed in 1 place"));
        keys(&mut w, "u");
        assert!(!w.active_doc().unwrap().buf.line(0).starts_with("Thing"));
    }

    #[test]
    fn diagnostics_from_the_server_join_problems() {
        let (mut w, r) = ws();
        let job = lsp_ready(&mut w, &r);
        let n = json::parse(r#"{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":"file:///home/me/tour/src/main.rs","diagnostics":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":2}},"severity":2,"message":"unused"}]}}"#).unwrap();
        r.push(JobEvent::Output {
            job,
            bytes: rpc::frame(&n),
            stderr: false,
        });
        w.poll_runner();
        assert_eq!(w.problems.len(), 1);
        assert_eq!(w.problems[0].source, Source::Lsp);
        assert_eq!(w.problems_on("src/main.rs", 0).count(), 1);
    }

    #[test]
    fn missing_tools_are_said_plainly() {
        let (mut w, r) = ws();
        r.0.borrow_mut().missing.push(String::from("rust-analyzer"));
        r.0.borrow_mut().finished.push((
            String::from("cargo"),
            Finished {
                code: Some(0),
                stdout: b"/x/Cargo.toml".to_vec(),
                stderr: Vec::new(),
            },
        ));
        w.start_lsp(true);
        assert!(w
            .message()
            .unwrap()
            .0
            .contains("rustup component add rust-analyzer"));
        keys(&mut w, "K");
        assert!(w
            .message()
            .unwrap()
            .0
            .contains("rust-analyzer is not installed"));
        r.0.borrow_mut().missing.push(String::from("lldb-dap"));
        r.0.borrow_mut().missing.push(String::from("xcrun"));
        w.run(crate::palette::Action::Tool(Tool::Debug));
        let (job, ..) = r.last().unwrap();
        r.out(job, r#"{"reason":"compiler-artifact","target":{"kind":["bin"]},"executable":"/x/target/debug/tour"}"#);
        r.out(job, "\n");
        r.exit(job, 0);
        w.poll_runner();
        assert!(
            w.message().unwrap().0.contains("No debug adapter"),
            "{:?}",
            w.message()
        );
    }

    #[test]
    fn debugging_stops_at_a_breakpoint_and_steps() {
        let (mut w, r) = ws();
        r.0.borrow_mut().finished.push((
            String::from("cargo"),
            Finished {
                code: Some(0),
                stdout: b"/home/me/tour/Cargo.toml\n".to_vec(),
                stderr: Vec::new(),
            },
        ));
        keys(&mut w, "3G");
        w.function_key(9, false);
        assert!(w.active_doc().unwrap().breakpoints.contains(&2));
        w.function_key(5, true);
        let (build, p, a) = r.last().unwrap();
        assert_eq!((p.as_str(), a[0].as_str()), ("cargo", "build"));
        r.out(build, "{\"reason\":\"compiler-artifact\",\"target\":{\"kind\":[\"bin\"]},\"executable\":\"/home/me/tour/target/debug/tour\"}\n");
        r.exit(build, 0);
        w.poll_runner();
        let dap = r.job_of("lldb-dap").expect("the adapter started");
        let frames = |r: &FakeRunner| {
            let mut f = crate::rpc::Framer::default();
            f.push(&r.written_to(dap));
            r.clear_written();
            f.drain()
        };
        let say = |r: &FakeRunner, s: &str| {
            r.push(JobEvent::Output {
                job: dap,
                bytes: rpc::frame(&json::parse(s).unwrap()),
                stderr: false,
            });
        };
        assert_eq!(frames(&r)[0].get("command").str(), Some("initialize"));
        say(
            &r,
            r#"{"seq":1,"type":"response","request_seq":1,"success":true,"command":"initialize","body":{}}"#,
        );
        w.poll_runner();
        let m = frames(&r);
        assert_eq!(
            m[0].at(&["arguments", "program"]).str(),
            Some("/home/me/tour/target/debug/tour")
        );
        say(
            &r,
            r#"{"seq":2,"type":"event","event":"initialized","body":{}}"#,
        );
        w.poll_runner();
        let m = frames(&r);
        assert_eq!(
            m[0].at(&["arguments", "breakpoints"])
                .idx(0)
                .get("line")
                .int(),
            Some(3)
        );
        say(
            &r,
            r#"{"seq":3,"type":"event","event":"stopped","body":{"reason":"breakpoint","threadId":1}}"#,
        );
        w.poll_runner();
        let st = frames(&r)
            .iter()
            .find(|m| m.get("command").str() == Some("stackTrace"))
            .unwrap()
            .get("seq")
            .int()
            .unwrap();
        say(
            &r,
            &alloc::format!(
                r#"{{"seq":4,"type":"response","request_seq":{st},"success":true,"command":"stackTrace","body":{{"stackFrames":[{{"id":5,"name":"tour::main","line":3,"source":{{"path":"/home/me/tour/src/main.rs"}}}}]}}}}"#
            ),
        );
        w.poll_runner();
        assert_eq!(w.debug_line("src/main.rs"), Some(2));
        assert_eq!(w.panel_tab, PanelTab::Debug);
        assert!(w
            .message()
            .unwrap()
            .0
            .starts_with("Stopped (breakpoint) in tour::main"));
        w.function_key(10, false);
        assert!(frames(&r)
            .iter()
            .any(|m| m.get("command").str() == Some("next")));
        w.function_key(5, true);
        assert!(frames(&r)
            .iter()
            .any(|m| m.get("command").str() == Some("disconnect")));
    }

    #[test]
    fn new_project_runs_cargo_init() {
        let mut w = Workspace::new();
        let r = FakeRunner::new();
        w.runner = Box::new(r.clone());
        struct Disk(MemFs);
        impl project::Vfs for Disk {
            fn is_real(&self) -> bool {
                true
            }
            fn list(&mut self, d: &str) -> Result<Vec<project::Entry>, String> {
                self.0.list(d)
            }
            fn read(&mut self, p: &str) -> Result<String, String> {
                self.0.read(p)
            }
            fn write(&mut self, p: &str, t: &str) -> Result<(), String> {
                self.0.write(p, t)
            }
            fn mkdir(&mut self, p: &str) -> Result<(), String> {
                self.0.mkdir(p)
            }
            fn rename(&mut self, a: &str, b: &str) -> Result<(), String> {
                self.0.rename(a, b)
            }
            fn remove_file(&mut self, p: &str) -> Result<(), String> {
                self.0.remove_file(p)
            }
            fn remove_dir(&mut self, p: &str) -> Result<(), String> {
                self.0.remove_dir(p)
            }
            fn exists(&mut self, p: &str) -> bool {
                self.0.exists(p)
            }
            fn folder(&self) -> Option<String> {
                Some(String::from("picked/abc"))
            }
        }
        w.new_project_folder("hello", Box::new(Disk(MemFs::new())));
        w.prompt_confirm();
        let (job, p, a) = r.last().unwrap();
        assert_eq!(
            (p.as_str(), a.join(" ")),
            ("cargo", String::from("init --name hello"))
        );
        assert!(w.creating.is_some());
        r.exit(job, 0);
        w.poll_runner();
        assert_eq!(w.project.as_ref().unwrap().name, "hello");
        assert!(w.message().unwrap().0.contains("with cargo"));
    }
}
