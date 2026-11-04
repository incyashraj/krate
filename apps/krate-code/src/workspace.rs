//! The whole editor's state: open files, panes, the tree, the palette, the
//! bottom panel, prompts, settings -- and the rules that tie them together.
//!
//! The guest draws this and feeds it input; nothing here calls the host, so
//! a whole session (open the tour, edit, split, save, rename) runs in a unit
//! test. Anything that needs the host -- the folder picker, closing the
//! window -- is left in [`Workspace::host`] for the guest to carry out.

use crate::buffer::{Buffer, Pos};
use crate::ide::{Completion, DapSession, Hover, Job, LspSession, Pick, Signature, Term};
use crate::palette::{self, Action, COMMANDS};
use crate::project::{self, MemFs, Project, Vfs};
use crate::syntax::Highlighter;
use crate::text;
use crate::tools::{Diagnostic, NoRunner, Runner, Tool};
use crate::vim::{Ctx, Cursor, Effect, Input, Key, Mode, ScrollTo, Vim};
use crate::vt::Vt;
use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;

/// The most splits side by side.
pub const MAX_PANES: usize = 4;
/// Lines kept between the cursor and the top or bottom edge.
const SCROLL_OFF: usize = 3;
/// A file bigger than this is not opened into the editor.
const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;

pub struct Doc {
    pub path: String,
    pub buf: Buffer,
    pub hl: Highlighter,
    /// Zero-based lines with a breakpoint, for the debugger (IC-977).
    pub breakpoints: BTreeSet<usize>,
}

impl Doc {
    pub fn new(path: &str, text: &str) -> Self {
        Doc {
            path: String::from(path),
            buf: Buffer::from_text(text),
            hl: Highlighter::new(),
            breakpoints: BTreeSet::new(),
        }
    }

    pub fn name(&self) -> &str {
        project::file_name(&self.path)
    }

    pub fn is_rust(&self) -> bool {
        self.path.ends_with(".rs")
    }

    /// Bring the highlighter and the breakpoints up to date with edits.
    pub fn sync(&mut self) {
        if let Some(line) = self.buf.take_dirty_from() {
            self.hl.invalidate_from(line);
        }
        for s in self.buf.take_shifts() {
            if self.breakpoints.is_empty() {
                continue;
            }
            let gone_to = s.from as isize - s.delta.min(0);
            let moved: BTreeSet<usize> = self
                .breakpoints
                .iter()
                .filter_map(|&l| {
                    let li = l as isize;
                    if l < s.from {
                        Some(l)
                    } else if s.delta < 0 && li < gone_to {
                        // A breakpoint on a deleted line goes with it.
                        None
                    } else {
                        Some((li + s.delta).max(0) as usize)
                    }
                })
                .collect();
            self.breakpoints = moved;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pane {
    pub doc: Option<usize>,
    pub cur: Cursor,
    /// The first line shown, fractional so a trackpad glides.
    pub top: f32,
    /// Columns scrolled off the left.
    pub left: f32,
    /// How many whole lines and columns the pane shows (set by the guest).
    pub rows: usize,
    pub cols: usize,
}

impl Pane {
    fn new(doc: Option<usize>) -> Self {
        Pane {
            doc,
            cur: Cursor::default(),
            top: 0.0,
            left: 0.0,
            rows: 30,
            cols: 100,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Editor,
    Tree,
    Panel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelTab {
    Problems,
    Output,
    Terminal,
    Debug,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptKind {
    NewFile {
        dir: String,
    },
    NewFolder {
        dir: String,
    },
    Rename {
        path: String,
    },
    Delete {
        path: String,
        is_dir: bool,
    },
    /// Name the crate a new project makes.
    ProjectName,
    /// The window is closing with unsaved files.
    CloseUnsaved,
    /// A new name for the symbol under the cursor (rust-analyzer).
    RenameSymbol,
}

#[derive(Debug, Clone)]
pub struct Prompt {
    pub kind: PromptKind,
    pub title: String,
    pub text: String,
    pub error: Option<String>,
}

impl Prompt {
    /// Whether it asks a yes/no question rather than for text.
    pub fn is_question(&self) -> bool {
        matches!(
            self.kind,
            PromptKind::Delete { .. } | PromptKind::CloseUnsaved
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteMode {
    Commands,
    Files,
    /// A list to choose from: references, code actions.
    Pick,
}

#[derive(Debug, Clone)]
pub struct PaletteState {
    pub mode: PaletteMode,
    pub query: String,
    /// Indices into [`COMMANDS`] or `files`.
    pub items: Vec<usize>,
    pub files: Vec<String>,
    pub selected: usize,
    /// What each row of a `Pick` list does.
    pub picks: Vec<Pick>,
    /// The title of a `Pick` list.
    pub title: String,
}

impl PaletteState {
    pub fn label(&self, item: usize) -> (&str, &str) {
        match self.mode {
            PaletteMode::Commands => COMMANDS
                .get(item)
                .map(|c| (c.title, c.keys))
                .unwrap_or(("", "")),
            PaletteMode::Files | PaletteMode::Pick => {
                (self.files.get(item).map(String::as_str).unwrap_or(""), "")
            }
        }
    }
}

/// Something only the guest can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostRequest {
    PickFolder,
    PickFolderForNewProject,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeChoice {
    Auto,
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    pub theme: ThemeChoice,
    /// A click into the text starts typing (insert mode) rather than only
    /// moving the cursor.
    pub click_inserts: bool,
    pub relative_numbers: bool,
    pub tree: bool,
    /// Code text size in points, before the person's own text scale.
    pub font: f32,
    /// Run rustfmt on a Rust file each time it is saved.
    pub format_on_save: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: ThemeChoice::Auto,
            click_inserts: true,
            relative_numbers: false,
            tree: true,
            font: 13.5,
            format_on_save: true,
        }
    }
}

impl Settings {
    pub fn to_text(&self) -> String {
        let mut s = String::from("theme=");
        s.push_str(match self.theme {
            ThemeChoice::Auto => "auto",
            ThemeChoice::Dark => "dark",
            ThemeChoice::Light => "light",
        });
        s.push_str(if self.click_inserts {
            "\nclick=insert"
        } else {
            "\nclick=normal"
        });
        s.push_str(if self.relative_numbers {
            "\nrnu=1"
        } else {
            "\nrnu=0"
        });
        s.push_str(if self.tree { "\ntree=1" } else { "\ntree=0" });
        s.push_str(if self.format_on_save {
            "\nfmt=1"
        } else {
            "\nfmt=0"
        });
        s.push_str("\nfont=");
        s.push_str(&text::num((self.font * 2.0) as usize));
        s.push('\n');
        s
    }

    pub fn from_text(t: &str) -> Self {
        let mut s = Settings::default();
        for line in t.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            match (k.trim(), v.trim()) {
                ("theme", "dark") => s.theme = ThemeChoice::Dark,
                ("theme", "light") => s.theme = ThemeChoice::Light,
                ("theme", _) => s.theme = ThemeChoice::Auto,
                ("click", v) => s.click_inserts = v != "normal",
                ("rnu", v) => s.relative_numbers = v == "1",
                ("tree", v) => s.tree = v != "0",
                ("fmt", v) => s.format_on_save = v != "0",
                ("font", v) => {
                    if let Ok(n) = v.parse::<usize>() {
                        let f = n as f32 / 2.0;
                        if (9.0..=28.0).contains(&f) {
                            s.font = f;
                        }
                    }
                }
                _ => {}
            }
        }
        s
    }
}

pub struct Workspace {
    pub vim: Vim,
    pub docs: Vec<Doc>,
    pub panes: Vec<Pane>,
    pub active: usize,
    pub focus: Focus,
    pub project: Option<Project>,
    pub settings: Settings,
    /// Set when settings changed and should be stored.
    pub settings_dirty: bool,
    pub panel_open: bool,
    pub panel_tab: PanelTab,
    pub problem_selected: usize,
    pub palette: Option<PaletteState>,
    pub prompt: Option<Prompt>,
    /// The Output panel: a log with colours, drawn like a terminal.
    pub output: Vt,
    /// Every problem shown: the last build's and the language server's.
    pub problems: Vec<Diagnostic>,
    pub build_problems: Vec<Diagnostic>,
    pub lsp_problems: BTreeMap<String, Vec<Diagnostic>>,
    /// Programs running for the editor: builds, `cargo new`.
    pub jobs: Vec<Job>,
    /// The time now, in milliseconds, set by the guest each turn.
    pub now_ms: u64,
    pub terminals: Vec<Term>,
    pub term_active: usize,
    /// The columns and rows the terminal panel fits (set by the guest).
    pub term_size: (u16, u16),
    pub lsp: Option<LspSession>,
    pub dap: Option<DapSession>,
    pub completion: Option<Completion>,
    pub hover: Option<Hover>,
    pub signature: Option<Signature>,
    /// The project folder's place on the computer, as cargo reports it --
    /// what the language server and the debugger need.
    pub root_abs: Option<String>,
    /// Programs found missing this run, so they are not tried every key.
    pub missing: BTreeSet<String>,
    /// What rust-analyzer is busy with ("Indexing").
    pub lsp_busy: Option<String>,
    /// The binary the last build made, for the debugger.
    pub last_exe: Option<String>,
    /// Start the debugger when the build that is going succeeds.
    pub debug_after_build: bool,
    /// The last Escape typed into a terminal: twice quickly leaves it.
    pub last_esc_ms: u64,
    /// A selection in the terminal or Output panel, in rows from the first
    /// line of history and columns.
    pub term_sel: Option<((usize, usize), (usize, usize))>,
    /// Text for the guest to put on the clipboard.
    pub clipboard_out: Option<String>,
    /// The next hover answer was asked by resting the mouse.
    pub hover_from_mouse: bool,
    /// The variable row chosen in the Debug panel.
    pub var_selected: usize,
    /// A project being made by `cargo init`: its folder and name.
    pub(crate) creating: Option<(String, Box<dyn Vfs>, String, u64)>,
    pub host: Option<HostRequest>,
    /// A line for the status area that is not the engine's.
    pub status: Option<(String, bool)>,
    pub runner: Box<dyn Runner>,
    /// The folder picked for a new project, waiting for its name.
    new_project: Option<(String, Box<dyn Vfs>)>,
    /// A text drag started here.
    drag_from: Option<Pos>,
    /// The close prompt was shown once: a second close goes through.
    close_warned: bool,
    /// Shown when no pane has a file: lets `:` work with nothing open.
    empty: Buffer,
    empty_cur: Cursor,
    /// Shown on the start screen and in the Keys guide.
    pub show_keys: bool,
}

impl Default for Workspace {
    fn default() -> Self {
        Workspace::new()
    }
}

impl Workspace {
    pub fn new() -> Self {
        Workspace {
            vim: Vim::new(),
            docs: Vec::new(),
            panes: alloc::vec![Pane::new(None)],
            active: 0,
            focus: Focus::Editor,
            project: None,
            settings: Settings::default(),
            settings_dirty: false,
            panel_open: false,
            panel_tab: PanelTab::Problems,
            problem_selected: 0,
            palette: None,
            prompt: None,
            output: {
                let mut v = Vt::new(120, 12);
                v.max_scrollback = 20_000;
                v
            },
            problems: Vec::new(),
            build_problems: Vec::new(),
            lsp_problems: BTreeMap::new(),
            jobs: Vec::new(),
            now_ms: 0,
            terminals: Vec::new(),
            term_active: 0,
            term_size: (100, 16),
            lsp: None,
            dap: None,
            completion: None,
            hover: None,
            signature: None,
            root_abs: None,
            missing: BTreeSet::new(),
            lsp_busy: None,
            last_exe: None,
            debug_after_build: false,
            last_esc_ms: 0,
            term_sel: None,
            clipboard_out: None,
            creating: None,
            hover_from_mouse: false,
            var_selected: 0,
            host: None,
            status: None,
            runner: Box::new(NoRunner),
            new_project: None,
            drag_from: None,
            close_warned: false,
            empty: Buffer::new(),
            empty_cur: Cursor::default(),
            show_keys: false,
        }
    }

    pub fn apply_settings(&mut self, s: Settings) {
        self.settings = s;
        self.vim.relative_numbers = s.relative_numbers;
    }

    fn settings_changed(&mut self) {
        self.settings.relative_numbers = self.vim.relative_numbers;
        self.settings_dirty = true;
    }

    // ---- what the guest reads --------------------------------------------------

    pub fn pane_doc(&self, pane: usize) -> Option<&Doc> {
        self.panes
            .get(pane)
            .and_then(|p| p.doc)
            .and_then(|d| self.docs.get(d))
    }

    pub fn active_doc_index(&self) -> Option<usize> {
        self.panes.get(self.active).and_then(|p| p.doc)
    }

    pub fn active_doc(&self) -> Option<&Doc> {
        self.pane_doc(self.active)
    }

    pub fn dirty_count(&self) -> usize {
        self.docs.iter().filter(|d| d.buf.modified()).count()
    }

    /// The status-area message: the engine's newest, else the workspace's.
    pub fn message(&self) -> Option<(&str, bool)> {
        self.vim
            .message
            .as_ref()
            .map(|(m, e)| (m.as_str(), *e))
            .or(self.status.as_ref().map(|(m, e)| (m.as_str(), *e)))
    }

    pub(crate) fn say(&mut self, msg: &str) {
        self.vim.message = None;
        self.status = Some((String::from(msg), false));
    }

    pub(crate) fn fail(&mut self, msg: &str) {
        self.vim.message = None;
        self.status = Some((String::from(msg), true));
    }

    /// A line for the Output panel.
    pub(crate) fn log(&mut self, line: &str) {
        self.output.write_text(line);
        self.output.write_text("\n");
    }

    /// Every line of the Output panel, for tests and automated runs.
    pub fn output_lines(&self) -> Vec<String> {
        let mut v = self.output.all_text();
        while v.last().is_some_and(|l| l.is_empty()) {
            v.pop();
        }
        v
    }

    // ---- projects ----------------------------------------------------------------

    /// Open a project folder (the guest made the backend from a pick).
    pub fn open_project(&mut self, name: &str, fs: Box<dyn Vfs>) {
        if self.dirty_count() > 0 {
            // Keep unsaved files of the old project from silently vanishing.
            self.fail("Save or close your changed files before opening another folder (:wa)");
            return;
        }
        self.close_ide();
        self.docs.clear();
        self.panes = alloc::vec![Pane::new(None)];
        self.active = 0;
        self.problems.clear();
        self.build_problems.clear();
        self.lsp_problems.clear();
        let real = fs.is_real();
        self.runner.set_folder(fs.folder());
        let mut p = Project::new(name, fs);
        let first = ["src/main.rs", "src/lib.rs", "README.md", "Cargo.toml"]
            .iter()
            .find(|f| p.fs.exists(f))
            .map(|f| String::from(*f));
        p.tree.selected = 0;
        self.project = Some(p);
        self.focus = Focus::Editor;
        self.show_keys = false;
        let mut line = String::from(if real { "Opened " } else { "Opened the tour: " });
        line.push_str(name);
        self.log(&line);
        if let Some(f) = first {
            self.open_file(&f);
        }
        if real
            && self
                .project
                .as_mut()
                .is_some_and(|p| p.fs.exists("Cargo.toml"))
        {
            self.start_lsp(false);
        }
        if !real {
            self.say("The tour keeps its files in memory. Press i to type, Esc for commands, Ctrl+Shift+P for everything.");
        } else {
            let mut m = String::from("Opened ");
            m.push_str(name);
            self.say(&m);
        }
    }

    pub fn open_tour(&mut self) {
        let mut m = MemFs::new();
        for (path, text) in project::tour_files() {
            let _ = m.write(path, text);
        }
        m.writes = 0;
        self.open_project("tour", Box::new(m));
    }

    /// The guest picked a folder for a new project: ask for the crate name.
    pub fn new_project_folder(&mut self, folder_name: &str, fs: Box<dyn Vfs>) {
        let mut fs = fs;
        if fs.exists("Cargo.toml") {
            self.open_project(folder_name, fs);
            self.say("That folder already has a Cargo.toml, so it opened as it is");
            return;
        }
        let suggested = project::crate_name_from(folder_name);
        self.new_project = Some((String::from(folder_name), fs));
        self.prompt = Some(Prompt {
            kind: PromptKind::ProjectName,
            title: String::from("Name the new Rust project (Cargo.toml, src/main.rs)"),
            text: suggested,
            error: None,
        });
    }

    fn finish_new_project(&mut self, name: &str) -> Result<(), String> {
        if !project::valid_crate_name(name) {
            return Err(String::from(
                "Use lowercase letters, digits, - or _, starting with a letter",
            ));
        }
        let Some((folder, mut fs)) = self.new_project.take() else {
            return Err(String::from("Pick a folder first"));
        };
        // cargo makes it the standard way when it is installed; the files
        // below are the same skeleton for when it is not.
        if fs.is_real() && !self.missing.contains("cargo") {
            match self.cargo_init(&folder, fs, name) {
                Ok(()) => return Ok(()),
                Err(back) => fs = back,
            }
        }
        for (path, body) in project::new_project_files(name) {
            if fs.exists(path) {
                continue;
            }
            if path.contains('/') {
                let _ = fs.mkdir(project::parent(path));
            }
            if let Err(e) = fs.write(path, &body) {
                let mut m = String::from("Could not write ");
                m.push_str(path);
                m.push_str(": ");
                m.push_str(&e);
                self.new_project = Some((folder, fs));
                return Err(m);
            }
        }
        self.open_project(&folder, fs);
        let mut m = String::from("Made the project ");
        m.push_str(name);
        if self.project.as_ref().is_some_and(|p| p.fs.is_real()) {
            m.push_str(" -- F7 builds it, F5 runs it");
        }
        self.say(&m);
        Ok(())
    }

    // ---- files -------------------------------------------------------------------

    /// Show `path` in the active pane, opening it if needed.
    pub fn open_file(&mut self, path: &str) -> bool {
        let path = match project::clean_relative(path) {
            Ok(p) => p,
            Err(e) => {
                self.fail(&e);
                return false;
            }
        };
        if let Some(i) = self.docs.iter().position(|d| d.path == path) {
            self.show_doc(i);
            return true;
        }
        let Some(project) = self.project.as_mut() else {
            self.fail("Open a folder first (Ctrl+O), or try the tour");
            return false;
        };
        let text = if project.fs.exists(&path) {
            match project.fs.read(&path) {
                Ok(t) => t,
                Err(e) => {
                    let mut m = String::from("Could not open ");
                    m.push_str(&path);
                    m.push_str(": ");
                    m.push_str(&e);
                    self.fail(&m);
                    return false;
                }
            }
        } else {
            // `:e new.rs` makes a new file, saved on the first :w.
            String::new()
        };
        if text.len() > MAX_FILE_BYTES {
            self.fail("That file is over 8 MB; Krate Code opens source files");
            return false;
        }
        if text.contains('\0') {
            self.fail("That looks like a binary file, so it stays closed");
            return false;
        }
        self.docs.push(Doc::new(&path, &text));
        let i = self.docs.len() - 1;
        self.show_doc(i);
        if let Some(p) = self.project.as_mut() {
            p.tree.reveal(&mut *p.fs, &path);
        }
        true
    }

    fn show_doc(&mut self, i: usize) {
        if self.vim.mode != Mode::Normal {
            self.leave_mode();
        }
        let Some(pane) = self.panes.get_mut(self.active) else {
            return;
        };
        if pane.doc != Some(i) {
            // Another pane on the same file lends its place.
            let other = self.panes.iter().find(|p| p.doc == Some(i)).copied();
            let pane = &mut self.panes[self.active];
            pane.doc = Some(i);
            match other {
                Some(o) => {
                    pane.cur = o.cur;
                    pane.top = o.top;
                    pane.left = o.left;
                }
                None => {
                    pane.cur = Cursor::default();
                    pane.top = 0.0;
                    pane.left = 0.0;
                }
            }
        }
        self.focus = Focus::Editor;
    }

    pub(crate) fn leave_mode(&mut self) {
        let a = self.active;
        match self.panes.get(a).and_then(|p| p.doc) {
            Some(d) => {
                if let (Some(doc), Some(pane)) = (self.docs.get_mut(d), self.panes.get_mut(a)) {
                    self.vim.to_normal(&mut doc.buf, &mut pane.cur);
                }
            }
            None => {
                self.vim.to_normal(&mut self.empty, &mut self.empty_cur);
            }
        }
    }

    /// Write a file. `as_path` saves it under a new name (relative).
    pub fn save(&mut self, doc: usize, as_path: Option<&str>) -> bool {
        if as_path.is_none() && self.settings.format_on_save {
            self.format_doc(doc, true);
        }
        let Some(project) = self.project.as_mut() else {
            self.fail("Open a folder first: files are saved into the project you open");
            return false;
        };
        let Some(d) = self.docs.get_mut(doc) else {
            return false;
        };
        if let Some(p) = as_path {
            match project::clean_relative(p) {
                Ok(p) => d.path = p,
                Err(e) => {
                    self.status = Some((e, true));
                    return false;
                }
            }
        }
        let parent = String::from(project::parent(&d.path));
        if !parent.is_empty() && !project.fs.exists(&parent) {
            let _ = project.fs.mkdir(&parent);
        }
        let text = d.buf.text();
        match project.fs.write(&d.path, &text) {
            Ok(()) => {
                d.buf.mark_saved();
                let lines = d.buf.line_count();
                let mut m = String::from("\"");
                m.push_str(&d.path);
                m.push_str("\" written, ");
                m.push_str(&text::num(lines));
                m.push_str(if lines == 1 { " line" } else { " lines" });
                if !project.fs.is_real() {
                    m.push_str(" (the tour keeps it in memory)");
                }
                let path = d.path.clone();
                project.tree.invalidate(&parent);
                project.refresh();
                self.log(&m);
                self.say(&m);
                self.lsp_saved(&path);
                true
            }
            Err(e) => {
                let mut m = String::from("Could not save ");
                m.push_str(&d.path);
                m.push_str(": ");
                m.push_str(&e);
                self.fail(&m);
                false
            }
        }
    }

    pub fn save_all(&mut self) -> usize {
        let mut n = 0;
        for i in 0..self.docs.len() {
            if self.docs[i].buf.modified() && self.save(i, None) {
                n += 1;
            }
        }
        if n > 1 {
            let mut m = text::num(n);
            m.push_str(" files written");
            self.say(&m);
        }
        n
    }

    /// Close a file; `force` drops unsaved changes.
    pub fn close_doc(&mut self, i: usize, force: bool) -> bool {
        let Some(d) = self.docs.get(i) else {
            return false;
        };
        if d.buf.modified() && !force {
            let mut m = String::from("No write since last change in ");
            m.push_str(d.name());
            m.push_str(" (:w to save, :q! to drop the changes)");
            self.fail(&m);
            return false;
        }
        if self.vim.mode != Mode::Normal {
            self.leave_mode();
        }
        let gone = self.docs[i].path.clone();
        if let Some(l) = self.lsp.as_mut() {
            l.client.closed(&gone);
            l.synced.remove(&gone);
        }
        self.completion = None;
        self.hover = None;
        self.docs.remove(i);
        let count = self.docs.len();
        for p in self.panes.iter_mut() {
            p.doc = match p.doc {
                Some(d) if d == i => {
                    p.cur = Cursor::default();
                    p.top = 0.0;
                    if count == 0 {
                        None
                    } else {
                        Some(i.min(count - 1))
                    }
                }
                Some(d) if d > i => Some(d - 1),
                other => other,
            };
        }
        true
    }

    // ---- panes -------------------------------------------------------------------

    pub fn split(&mut self, path: Option<&str>) {
        if self.panes.len() >= MAX_PANES {
            self.fail("Four splits is the most that fit side by side");
            return;
        }
        if self.vim.mode != Mode::Normal {
            self.leave_mode();
        }
        let copy = self
            .panes
            .get(self.active)
            .copied()
            .unwrap_or(Pane::new(None));
        self.panes.insert(self.active + 1, copy);
        self.active += 1;
        if let Some(p) = path {
            self.open_file(p);
        }
    }

    pub fn close_pane(&mut self) -> bool {
        if self.panes.len() <= 1 {
            return false;
        }
        if self.vim.mode != Mode::Normal {
            self.leave_mode();
        }
        self.panes.remove(self.active);
        self.active = self.active.min(self.panes.len() - 1);
        true
    }

    pub fn focus_pane(&mut self, i: usize) {
        if i < self.panes.len() && i != self.active {
            if self.vim.mode != Mode::Normal {
                self.leave_mode();
            }
            self.active = i;
        }
        self.focus = Focus::Editor;
    }

    /// Keep the cursor of `pane` on screen, `SCROLL_OFF` lines from the edges.
    pub fn ensure_visible(&mut self, pane: usize) {
        let tab = self.vim.tab;
        let Some(p) = self.panes.get(pane).copied() else {
            return;
        };
        let Some(doc) = p.doc.and_then(|d| self.docs.get(d)) else {
            return;
        };
        let line = p.cur.pos.line;
        let rows = p.rows.max(1);
        let off = SCROLL_OFF.min(rows.saturating_sub(1) / 2);
        let mut top = p.top;
        if (line as f32) < top + off as f32 {
            top = line.saturating_sub(off) as f32;
        }
        let bottom_ok = top + (rows - 1 - off) as f32;
        if line as f32 > bottom_ok {
            top = (line + off + 1) as f32 - rows as f32;
        }
        let max_top = doc.buf.line_count().saturating_sub(1) as f32;
        top = top.clamp(0.0, max_top);
        let col = text::display_col(doc.buf.line(line), p.cur.pos.col, tab) as f32;
        let cols = p.cols.max(8) as f32;
        let mut left = p.left;
        if col < left {
            left = (col - 4.0).max(0.0);
        } else if col > left + cols - 3.0 {
            left = col - cols + 6.0;
        }
        if let Some(pm) = self.panes.get_mut(pane) {
            pm.top = top;
            pm.left = left.max(0.0);
        }
    }

    /// Scroll a pane by whole or partial lines and columns (a wheel).
    pub fn scroll(&mut self, pane: usize, lines: f32, cols: f32) {
        let Some(p) = self.panes.get(pane).copied() else {
            return;
        };
        let Some(doc) = p.doc.and_then(|d| self.docs.get(d)) else {
            return;
        };
        let count = doc.buf.line_count();
        let max_top = count.saturating_sub(p.rows.saturating_sub(2).max(1)) as f32;
        let widest = 400.0f32;
        if let Some(pm) = self.panes.get_mut(pane) {
            pm.top = (pm.top + lines).clamp(0.0, max_top.max(0.0));
            pm.left = (pm.left + cols).clamp(0.0, widest);
        }
    }

    fn scroll_to(&mut self, to: ScrollTo) {
        let a = self.active;
        let Some(p) = self.panes.get_mut(a) else {
            return;
        };
        let line = p.cur.pos.line as f32;
        let rows = p.rows.max(1) as f32;
        p.top = match to {
            ScrollTo::Top => line,
            ScrollTo::Center => line - ((rows as usize) / 2) as f32,
            ScrollTo::Bottom => line - rows + 1.0,
        }
        .max(0.0);
    }

    // ---- input -------------------------------------------------------------------

    /// One input from the keyboard, routed to whatever has focus.
    pub fn input(&mut self, input: Input) {
        self.status = None;
        if self.prompt.is_some() {
            self.prompt_key(input);
            return;
        }
        if self.palette.is_some() {
            self.palette_key(input);
            return;
        }
        if self.show_keys && input == Input::Key(Key::Esc) {
            self.show_keys = false;
            return;
        }
        match self.focus {
            Focus::Tree => self.tree_key(input),
            Focus::Panel => self.panel_key(input),
            Focus::Editor => self.editor_input(input),
        }
    }

    fn editor_input(&mut self, input: Input) {
        let a = self.active;
        let Some(d) = self.panes.get(a).and_then(|p| p.doc) else {
            // Nothing open: `:` still works (`:e`, `:theme`), and typing says
            // how to start.
            let cmd_like = matches!(self.vim.mode, Mode::Command | Mode::Search { .. })
                || matches!(input, Input::Char(':'));
            if cmd_like {
                let mut ctx = Ctx {
                    buf: &mut self.empty,
                    cur: &mut self.empty_cur,
                    page: 10,
                    top: 0,
                };
                let fx = self.vim.feed(&mut ctx, input);
                self.empty = Buffer::new();
                self.empty_cur = Cursor::default();
                self.effects(fx);
            } else if matches!(input, Input::Char(_)) {
                if self.project.is_some() {
                    self.say("Open a file from the tree on the left, or press Ctrl+P to find one");
                } else {
                    self.say("Open a folder, make a new project, or take the tour to start");
                }
            }
            return;
        };
        self.vim.message = None;
        if self.ide_intercept(&input) {
            return;
        }
        let was_insert = self.vim.mode == Mode::Insert;
        let fx = {
            let (docs, panes) = (&mut self.docs, &mut self.panes);
            let (Some(doc), Some(pane)) = (docs.get_mut(d), panes.get_mut(a)) else {
                return;
            };
            let mut ctx = Ctx {
                buf: &mut doc.buf,
                cur: &mut pane.cur,
                page: pane.rows.max(1),
                top: pane.top as usize,
            };
            let fx = self.vim.feed(&mut ctx, input.clone());
            doc.sync();
            fx
        };
        self.effects(fx);
        self.after_edit();
        self.ide_after_input(&input, was_insert);
    }

    /// Keep every pane's cursor valid and the active one on screen.
    pub(crate) fn after_edit(&mut self) {
        for i in 0..self.panes.len() {
            let Some(d) = self.panes[i].doc else { continue };
            if let Some(doc) = self.docs.get(d) {
                let p = &mut self.panes[i];
                p.cur.pos = doc.buf.clamp(p.cur.pos);
                p.cur.anchor = doc.buf.clamp(p.cur.anchor);
            }
        }
        self.ensure_visible(self.active);
    }

    fn effects(&mut self, fx: Vec<Effect>) {
        for e in fx {
            match e {
                Effect::Write { path } => {
                    if let Some(d) = self.active_doc_index() {
                        self.save(d, path.as_deref());
                    } else {
                        self.fail("No file to write");
                    }
                }
                Effect::Quit { force } => self.quit(force),
                Effect::WriteQuit => {
                    let saved = match self.active_doc_index() {
                        Some(d) if self.docs[d].buf.modified() => self.save(d, None),
                        _ => true,
                    };
                    if saved {
                        self.quit(false);
                    }
                }
                Effect::WriteAll => {
                    self.save_all();
                }
                Effect::QuitAll { force } => {
                    let dirty = self.dirty_count();
                    if dirty > 0 && !force {
                        let mut m = text::num(dirty);
                        m.push_str(if dirty == 1 {
                            " file has"
                        } else {
                            " files have"
                        });
                        m.push_str(" unsaved changes (:wa to save, :qa! to drop them)");
                        self.fail(&m);
                    } else {
                        self.host = Some(HostRequest::Close);
                    }
                }
                Effect::Edit { path, force } => match path {
                    Some(p) => {
                        self.open_file(&p);
                    }
                    None => self.reload_active(force),
                },
                Effect::Split { path } => self.split(path.as_deref()),
                Effect::FocusPane { forward } => {
                    let n = self.panes.len();
                    if self.focus == Focus::Tree {
                        self.focus = Focus::Editor;
                    } else if n > 1 {
                        let next = if forward {
                            (self.active + 1) % n
                        } else {
                            (self.active + n - 1) % n
                        };
                        self.focus_pane(next);
                    } else if !forward && self.settings.tree && self.project.is_some() {
                        self.focus = Focus::Tree;
                    }
                }
                Effect::ClosePane => {
                    if !self.close_pane() {
                        self.fail("This is the only split (:q closes the file)");
                    }
                }
                Effect::OnlyPane => {
                    let keep = self.panes.get(self.active).copied();
                    if let Some(k) = keep {
                        self.panes = alloc::vec![k];
                        self.active = 0;
                    }
                }
                Effect::NextBuffer { forward } => self.cycle_doc(forward),
                Effect::CloseBuffer { force } => {
                    if let Some(d) = self.active_doc_index() {
                        self.close_doc(d, force);
                    }
                }
                Effect::Scroll(to) => self.scroll_to(to),
                Effect::ScrollLines(n) => {
                    let a = self.active;
                    self.scroll(a, n as f32, 0.0);
                }
                Effect::Command(c) => self.command(&c),
            }
        }
    }

    fn quit(&mut self, force: bool) {
        if self.panes.len() > 1 {
            self.close_pane();
            return;
        }
        match self.active_doc_index() {
            Some(d) => {
                self.close_doc(d, force);
            }
            None => self.say("Nothing open to close (:qa closes the window)"),
        }
    }

    fn cycle_doc(&mut self, forward: bool) {
        let n = self.docs.len();
        if n == 0 {
            return;
        }
        let cur = self.active_doc_index().unwrap_or(0);
        let next = if forward {
            (cur + 1) % n
        } else {
            (cur + n - 1) % n
        };
        self.show_doc(next);
    }

    fn reload_active(&mut self, force: bool) {
        let Some(d) = self.active_doc_index() else {
            return;
        };
        if self.docs[d].buf.modified() && !force {
            self.fail("No write since last change (:e! drops the changes and reloads)");
            return;
        }
        let path = self.docs[d].path.clone();
        let Some(p) = self.project.as_mut() else {
            return;
        };
        match p.fs.read(&path) {
            Ok(t) => {
                self.docs[d] = Doc::new(&path, &t);
                self.after_edit();
                self.say("Reloaded from disk");
            }
            Err(e) => self.fail(&e),
        }
    }

    /// `:` commands the engine handed over.
    pub fn command(&mut self, c: &str) {
        let (name, arg) = match c.split_once(' ') {
            Some((n, a)) => (n, a.trim()),
            None => (c, ""),
        };
        match name {
            "theme" | "colorscheme" | "colo" => match arg {
                "dark" | "dusk" => self.run(Action::ThemeDark),
                "light" | "paper" => self.run(Action::ThemeLight),
                "auto" | "" => self.run(Action::ThemeAuto),
                _ => self.fail("Themes: dark, light or auto"),
            },
            "h" | "help" | "keys" => self.run(Action::Keys),
            "tree" | "Explore" | "Ex" | "NvimTreeToggle" => self.run(Action::ToggleTree),
            "palette" => self.run(Action::QuickOpen),
            "cargo" => {
                let tool = match arg.split_whitespace().next().unwrap_or("build") {
                    "run" | "r" => Tool::Run,
                    "test" | "t" => Tool::Test,
                    "check" | "c" => Tool::Check,
                    "clippy" => Tool::Clippy,
                    "fmt" => Tool::Format,
                    _ => Tool::Build,
                };
                self.run(Action::Tool(tool));
            }
            "make" | "build" => self.run(Action::Tool(Tool::Build)),
            "run" => self.run(Action::Tool(Tool::Run)),
            "test" => self.run(Action::Tool(Tool::Test)),
            "check" => self.run(Action::Tool(Tool::Check)),
            "fmt" | "format" => self.run(Action::Tool(Tool::Format)),
            "term" | "terminal" => self.run(Action::Tool(Tool::Terminal)),
            "new" | "cargo-new" => self.run(Action::NewProject),
            "debug" => self.run(Action::Tool(Tool::Debug)),
            "stop" => self.run(Action::Tool(Tool::Stop)),
            "rename" => {
                if arg.is_empty() {
                    self.run(Action::Tool(Tool::RenameSymbol));
                } else {
                    self.lsp_rename(arg);
                }
            }
            "refs" | "references" => self.run(Action::Tool(Tool::References)),
            "actions" | "fix" => self.run(Action::Tool(Tool::CodeAction)),
            "hover" => self.run(Action::Tool(Tool::Hover)),
            "def" | "definition" => self.run(Action::Tool(Tool::Definition)),
            "continue" | "cont" => self.run(Action::Tool(Tool::Continue)),
            "step" => self.run(Action::Tool(Tool::StepIn)),
            "over" => self.run(Action::Tool(Tool::StepOver)),
            "finish" => self.run(Action::Tool(Tool::StepOut)),
            "fos" | "formatonsave" => self.run(Action::FormatOnSave),
            "lsp" => self.start_lsp(true),
            "clippy" => self.run(Action::Tool(Tool::Clippy)),
            "bp" | "breakpoint" => self.run(Action::ToggleBreakpoint),
            "copen" | "problems" => self.run(Action::ShowProblems),
            "messages" | "output" => self.run(Action::ShowOutput),
            "tour" => self.run(Action::Tour),
            "open" => self.run(Action::OpenFolder),
            _ => {
                let mut m = String::from("Not an editor command: ");
                m.push_str(c);
                m.push_str(" (Ctrl+Shift+P lists everything)");
                self.fail(&m);
            }
        }
    }

    /// Cmd or Ctrl with a key. True when it was a shortcut here; false
    /// means "pass it to the modal engine as Ctrl+key".
    pub fn shortcut(&mut self, key: char, shift: bool) -> bool {
        let action = match (key.to_ascii_lowercase(), shift) {
            ('p', true) => {
                self.open_palette(PaletteMode::Commands);
                return true;
            }
            ('p', false) => Action::QuickOpen,
            ('s', false) => Action::Save,
            ('s', true) => Action::SaveAll,
            ('o', _) => Action::OpenFolder,
            ('b', _) => Action::ToggleTree,
            ('j', _) => Action::TogglePanel,
            ('e', true) => Action::FocusTree,
            ('z', false) => Action::Undo,
            ('z', true) | ('y', _) => Action::Redo,
            ('`', _) => Action::ShowTerminal,
            ('.', _) => Action::Tool(Tool::CodeAction),
            ('=', _) | ('+', _) => Action::BiggerText,
            ('-', _) => Action::SmallerText,
            ('n', _) => Action::NewFile,
            ('f', _) => Action::Find,
            ('a', false) => {
                self.select_all();
                return true;
            }
            ('c', false) | ('x', false) => {
                let cut = key == 'x';
                return self.copy_or_cut(cut);
            }
            ('v', false) => return true, // the paste itself arrives as text
            _ => return false,
        };
        self.run(action);
        true
    }

    /// F5, F7, F9, F12.
    pub fn function_key(&mut self, n: u8, shift: bool) -> bool {
        let debugging = self.dap.is_some();
        let a = match (n, shift) {
            (2, _) => Action::Tool(Tool::RenameSymbol),
            (5, false) if debugging => Action::Tool(Tool::Continue),
            (5, false) => Action::Tool(Tool::Run),
            (5, true) if debugging => Action::Tool(Tool::StopDebug),
            (5, true) => Action::Tool(Tool::Debug),
            (7, _) => Action::Tool(Tool::Build),
            (9, _) => Action::ToggleBreakpoint,
            (10, _) => Action::Tool(Tool::StepOver),
            (11, false) => Action::Tool(Tool::StepIn),
            (11, true) => Action::Tool(Tool::StepOut),
            (12, false) => Action::Tool(Tool::Definition),
            (12, true) => Action::Tool(Tool::References),
            _ => return false,
        };
        self.run(a);
        true
    }

    fn select_all(&mut self) {
        let a = self.active;
        let Some(d) = self.active_doc_index() else {
            return;
        };
        let (docs, panes) = (&mut self.docs, &mut self.panes);
        let (Some(doc), Some(pane)) = (docs.get_mut(d), panes.get_mut(a)) else {
            return;
        };
        let end = doc.buf.end();
        pane.cur.pos = end;
        self.vim
            .begin_select(&mut doc.buf, &mut pane.cur, Pos::new(0, 0));
    }

    fn copy_or_cut(&mut self, cut: bool) -> bool {
        let a = self.active;
        let Some(d) = self.active_doc_index() else {
            return false;
        };
        if !self.vim.mode.is_visual() {
            // Ctrl+C with nothing selected is Vim's: back to normal.
            if !cut {
                self.leave_mode();
                return true;
            }
            return false;
        }
        let (docs, panes) = (&mut self.docs, &mut self.panes);
        let (Some(doc), Some(pane)) = (docs.get_mut(d), panes.get_mut(a)) else {
            return false;
        };
        let mut ctx = Ctx {
            buf: &mut doc.buf,
            cur: &mut pane.cur,
            page: pane.rows,
            top: pane.top as usize,
        };
        let done = self.vim.copy_selection(&mut ctx, cut);
        doc.sync();
        if done {
            // The system clipboard too, so it pastes into other apps.
            self.clipboard_out = self.vim.registers.get(&'"').map(|r| r.text.clone());
            self.say(if cut {
                "Cut (paste it with p)"
            } else {
                "Copied (paste it with p)"
            });
        }
        self.after_edit();
        done
    }

    // ---- actions -----------------------------------------------------------------

    pub fn run(&mut self, a: Action) {
        match a {
            Action::OpenFolder => self.host = Some(HostRequest::PickFolder),
            Action::NewProject => self.host = Some(HostRequest::PickFolderForNewProject),
            Action::Tour => self.open_tour(),
            Action::QuickOpen => {
                if self.project.is_none() {
                    self.fail("Open a folder first (Ctrl+O), or try the tour");
                } else {
                    self.open_palette(PaletteMode::Files);
                }
            }
            Action::Save => {
                if let Some(d) = self.active_doc_index() {
                    self.leave_insert_for_save();
                    self.save(d, None);
                }
            }
            Action::SaveAll => {
                self.save_all();
            }
            Action::CloseFile => {
                if let Some(d) = self.active_doc_index() {
                    self.close_doc(d, false);
                }
            }
            Action::NextFile => self.cycle_doc(true),
            Action::PrevFile => self.cycle_doc(false),
            Action::NewFile | Action::NewFolder => {
                let Some(p) = self.project.as_ref() else {
                    self.fail("Open a folder first (Ctrl+O), or try the tour");
                    return;
                };
                let dir = if self.focus == Focus::Tree {
                    p.tree.target_dir()
                } else {
                    self.active_doc()
                        .map(|d| String::from(project::parent(&d.path)))
                        .unwrap_or_else(|| p.tree.target_dir())
                };
                let file = a == Action::NewFile;
                let mut title = String::from(if file {
                    "New file in "
                } else {
                    "New folder in "
                });
                title.push_str(if dir.is_empty() { "the project" } else { &dir });
                self.prompt = Some(Prompt {
                    kind: if file {
                        PromptKind::NewFile { dir }
                    } else {
                        PromptKind::NewFolder { dir }
                    },
                    title,
                    text: String::new(),
                    error: None,
                });
            }
            Action::Rename | Action::Delete => {
                let target = match (self.focus, self.project.as_ref()) {
                    (Focus::Tree, Some(p)) => {
                        p.tree.selected_row().map(|r| (r.path.clone(), r.is_dir))
                    }
                    (_, Some(_)) => self.active_doc().map(|d| (d.path.clone(), false)),
                    _ => None,
                };
                let Some((path, is_dir)) = target else {
                    self.fail("Pick a file in the tree first");
                    return;
                };
                if a == Action::Rename {
                    let mut title = String::from("Rename ");
                    title.push_str(&path);
                    self.prompt = Some(Prompt {
                        kind: PromptKind::Rename { path: path.clone() },
                        title,
                        text: path,
                        error: None,
                    });
                } else {
                    let mut title = String::from("Delete ");
                    title.push_str(&path);
                    title.push_str(if is_dir {
                        " and everything in it?"
                    } else {
                        "?"
                    });
                    if self.project.as_ref().is_some_and(|p| p.fs.is_real()) {
                        title.push_str(" It is removed from your disk, not moved to the bin.");
                    }
                    self.prompt = Some(Prompt {
                        kind: PromptKind::Delete { path, is_dir },
                        title,
                        text: String::new(),
                        error: None,
                    });
                }
            }
            Action::RevealInTree => {
                if let (Some(d), Some(p)) = (
                    self.active_doc().map(|d| d.path.clone()),
                    self.project.as_mut(),
                ) {
                    p.tree.reveal(&mut *p.fs, &d);
                    self.settings.tree = true;
                    self.focus = Focus::Tree;
                }
            }
            Action::FocusTree => {
                if self.project.is_some() {
                    self.settings.tree = true;
                    self.leave_mode();
                    self.focus = Focus::Tree;
                }
            }
            Action::SplitRight => self.split(None),
            Action::ClosePane => {
                if !self.close_pane() {
                    self.fail("This is the only split");
                }
            }
            Action::NextPane => {
                let n = self.panes.len();
                self.focus_pane((self.active + 1) % n.max(1));
            }
            Action::ToggleTree => {
                self.settings.tree = !self.settings.tree;
                if !self.settings.tree && self.focus == Focus::Tree {
                    self.focus = Focus::Editor;
                }
                self.settings_changed();
            }
            Action::TogglePanel => {
                self.panel_open = !self.panel_open;
                if !self.panel_open && self.focus == Focus::Panel {
                    self.focus = Focus::Editor;
                }
            }
            Action::ShowProblems => self.show_panel(PanelTab::Problems),
            Action::ShowOutput => self.show_panel(PanelTab::Output),
            Action::ShowTerminal => self.toggle_terminal(),
            Action::Find => self.editor_keys_from_normal("/"),
            Action::Replace => self.editor_keys_from_normal(":%s/"),
            Action::GotoLine => self.editor_keys_from_normal(":"),
            Action::Undo => self.editor_keys_from_normal("u"),
            Action::Redo => {
                self.leave_mode();
                self.editor_input(Input::Ctrl('r'));
            }
            Action::ToggleBreakpoint => {
                let a = self.active;
                if let (Some(d), Some(p)) = (self.active_doc_index(), self.panes.get(a)) {
                    let line = p.cur.pos.line;
                    let doc = &mut self.docs[d];
                    let on = if doc.breakpoints.remove(&line) {
                        false
                    } else {
                        doc.breakpoints.insert(line);
                        true
                    };
                    let mut m = String::from(if on {
                        "Breakpoint on line "
                    } else {
                        "Breakpoint removed from line "
                    });
                    m.push_str(&text::num(line + 1));
                    if self.dap.is_none() {
                        m.push_str(" -- Shift+F5 builds and debugs, stopping there");
                    }
                    self.say(&m);
                    self.dap_breakpoints_changed(d);
                }
            }
            Action::ThemeAuto | Action::ThemeDark | Action::ThemeLight => {
                self.settings.theme = match a {
                    Action::ThemeDark => ThemeChoice::Dark,
                    Action::ThemeLight => ThemeChoice::Light,
                    _ => ThemeChoice::Auto,
                };
                self.settings_changed();
            }
            Action::ClickMode => {
                self.settings.click_inserts = !self.settings.click_inserts;
                self.settings_changed();
                self.say(if self.settings.click_inserts {
                    "Clicking in the text now starts typing (insert mode)"
                } else {
                    "Clicking in the text now only moves the cursor (normal mode)"
                });
            }
            Action::RelativeNumbers => {
                self.vim.relative_numbers = !self.vim.relative_numbers;
                self.settings_changed();
            }
            Action::BiggerText | Action::SmallerText => {
                let step = if a == Action::BiggerText { 1.0 } else { -1.0 };
                self.settings.font = (self.settings.font + step).clamp(9.0, 28.0);
                self.settings_changed();
            }
            Action::Keys => self.show_keys = !self.show_keys,
            Action::FormatOnSave => {
                self.settings.format_on_save = !self.settings.format_on_save;
                self.settings_changed();
                self.say(if self.settings.format_on_save {
                    "Rust files are formatted with rustfmt each time you save"
                } else {
                    "Saving no longer formats (Format file still does)"
                });
            }
            Action::ShowDebug => self.show_panel(PanelTab::Debug),
            Action::Tool(t) => self.tool(t),
        }
    }

    fn leave_insert_for_save(&mut self) {
        // Cmd+S in insert mode keeps typing afterwards, like any editor, but
        // the typed run becomes its own undo step.
        let a = self.active;
        if let (Some(d), Some(p)) = (self.active_doc_index(), self.panes.get(a)) {
            let at = p.cur.pos;
            let buf = &mut self.docs[d].buf;
            if buf.group_open() {
                buf.end_group(at);
                if self.vim.mode == Mode::Insert {
                    buf.begin_group(at);
                }
            }
        }
    }

    /// Type keys into the editor from normal mode (palette actions).
    fn editor_keys_from_normal(&mut self, keys: &str) {
        self.focus = Focus::Editor;
        self.leave_mode();
        for i in crate::vim::parse_keys(keys) {
            self.editor_input(i);
        }
    }

    pub(crate) fn show_panel(&mut self, tab: PanelTab) {
        self.panel_open = true;
        self.panel_tab = tab;
    }

    /// Open the file of problem `i` at its place.
    pub fn jump_to_problem(&mut self, i: usize) {
        let Some(d) = self.problems.get(i).cloned() else {
            return;
        };
        self.problem_selected = i;
        if self.open_file(&d.file) {
            let a = self.active;
            if let (Some(di), Some(p)) = (self.active_doc_index(), self.panes.get_mut(a)) {
                let buf = &self.docs[di].buf;
                let line = d.line.min(buf.last_line());
                let col = text::byte_of_char(buf.line(line), d.col);
                let pos = buf.clamp(Pos::new(line, col));
                p.cur.pos = pos;
                self.vim.sync_want(&self.docs[di].buf, &mut p.cur);
            }
            self.focus = Focus::Editor;
            self.ensure_visible(self.active);
        }
    }

    // ---- palette -----------------------------------------------------------------

    pub fn open_palette(&mut self, mode: PaletteMode) {
        self.leave_mode();
        let files = match (mode, self.project.as_mut()) {
            (PaletteMode::Files, Some(p)) => p.all_files(5000),
            _ => Vec::new(),
        };
        let mut st = PaletteState {
            mode,
            query: String::new(),
            items: Vec::new(),
            files,
            selected: 0,
            picks: Vec::new(),
            title: String::new(),
        };
        Self::refilter(&mut st);
        self.palette = Some(st);
    }

    pub(crate) fn refilter(st: &mut PaletteState) {
        st.items = match st.mode {
            PaletteMode::Commands => palette::filter(&st.query),
            PaletteMode::Files | PaletteMode::Pick => {
                let mut scored: Vec<(i32, usize)> = st
                    .files
                    .iter()
                    .enumerate()
                    .filter_map(|(i, f)| text::fuzzy_score(&st.query, f).map(|s| (s, i)))
                    .collect();
                if !st.query.is_empty() {
                    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
                }
                scored.into_iter().map(|(_, i)| i).collect()
            }
        };
        st.selected = 0;
    }

    pub fn palette_choose(&mut self, row: usize) {
        let Some(st) = self.palette.take() else {
            return;
        };
        let Some(&item) = st.items.get(row) else {
            return;
        };
        match st.mode {
            PaletteMode::Commands => {
                if let Some(c) = COMMANDS.get(item) {
                    self.run(c.action);
                }
            }
            PaletteMode::Files => {
                if let Some(f) = st.files.get(item).cloned() {
                    self.open_file(&f);
                }
            }
            PaletteMode::Pick => {
                if let Some(p) = st.picks.get(item).cloned() {
                    self.pick_chosen(p);
                }
            }
        }
    }

    fn palette_key(&mut self, input: Input) {
        let Some(st) = self.palette.as_mut() else {
            return;
        };
        match input {
            Input::Key(Key::Esc) => self.palette = None,
            Input::Key(Key::Enter) => {
                let row = st.selected;
                self.palette_choose(row);
            }
            Input::Key(Key::Down) | Input::Ctrl('n') | Input::Key(Key::Tab) => {
                if !st.items.is_empty() {
                    st.selected = (st.selected + 1) % st.items.len();
                }
            }
            Input::Key(Key::Up) | Input::Ctrl('p') => {
                if !st.items.is_empty() {
                    st.selected = (st.selected + st.items.len() - 1) % st.items.len();
                }
            }
            Input::Key(Key::Backspace) => {
                st.query.pop();
                Self::refilter(st);
            }
            Input::Char(c) => {
                st.query.push(c);
                Self::refilter(st);
            }
            Input::Paste(t) => {
                st.query.push_str(t.lines().next().unwrap_or(""));
                Self::refilter(st);
            }
            _ => {}
        }
    }

    // ---- prompts -----------------------------------------------------------------

    fn prompt_key(&mut self, input: Input) {
        let Some(pr) = self.prompt.as_mut() else {
            return;
        };
        if pr.is_question() {
            let yes = matches!(
                input,
                Input::Char('y') | Input::Char('Y') | Input::Key(Key::Enter)
            );
            match (&pr.kind, input) {
                (_, Input::Key(Key::Esc)) | (_, Input::Char('n')) | (_, Input::Char('N')) => {
                    if pr.kind == PromptKind::CloseUnsaved {
                        self.close_warned = false;
                    }
                    self.prompt = None;
                }
                (PromptKind::CloseUnsaved, Input::Char('s'))
                | (PromptKind::CloseUnsaved, Input::Key(Key::Enter)) => {
                    self.prompt = None;
                    self.save_all();
                    if self.dirty_count() == 0 {
                        self.host = Some(HostRequest::Close);
                    }
                }
                (PromptKind::CloseUnsaved, Input::Char('d')) => {
                    self.prompt = None;
                    self.host = Some(HostRequest::Close);
                }
                (PromptKind::Delete { .. }, _) if yes => self.prompt_confirm(),
                _ => {}
            }
            return;
        }
        match input {
            Input::Key(Key::Esc) => {
                if pr.kind == PromptKind::ProjectName {
                    self.new_project = None;
                }
                self.prompt = None;
            }
            Input::Key(Key::Enter) => self.prompt_confirm(),
            Input::Key(Key::Backspace) => {
                pr.text.pop();
                pr.error = None;
            }
            Input::Ctrl('u') => pr.text.clear(),
            Input::Char(c) => {
                pr.text.push(c);
                pr.error = None;
            }
            Input::Paste(t) => pr.text.push_str(t.lines().next().unwrap_or("")),
            _ => {}
        }
    }

    /// The prompt's button (or Enter).
    pub fn prompt_confirm(&mut self) {
        let Some(pr) = self.prompt.clone() else {
            return;
        };
        let result: Result<(), String> = match &pr.kind {
            PromptKind::NewFile { dir } | PromptKind::NewFolder { dir } => {
                let is_file = matches!(pr.kind, PromptKind::NewFile { .. });
                match project::clean_relative(&project::join(dir, &pr.text)) {
                    Err(e) => Err(e),
                    Ok(path) => {
                        let taken = self.project.as_mut().is_some_and(|p| p.fs.exists(&path));
                        match self.project.as_mut() {
                            None => Err(String::from("Open a folder first")),
                            Some(_) if taken => {
                                Err(String::from("Something with that name is already there"))
                            }
                            Some(p) => {
                                let parent = String::from(project::parent(&path));
                                if !parent.is_empty() && !p.fs.exists(&parent) {
                                    let _ = p.fs.mkdir(&parent);
                                }
                                let r = if is_file {
                                    p.fs.write(&path, "")
                                } else {
                                    p.fs.mkdir(&path)
                                };
                                match r {
                                    Ok(()) => {
                                        p.tree.invalidate(&parent);
                                        p.tree.reveal(&mut *p.fs, &path);
                                        self.prompt = None;
                                        if is_file {
                                            self.open_file(&path);
                                            let a = self.active;
                                            if let (Some(d), Some(pane)) =
                                                (self.active_doc_index(), self.panes.get_mut(a))
                                            {
                                                self.vim.begin_insert(
                                                    &mut self.docs[d].buf,
                                                    &mut pane.cur,
                                                );
                                            }
                                        }
                                        return;
                                    }
                                    Err(e) => Err(e),
                                }
                            }
                        }
                    }
                }
            }
            PromptKind::Rename { path } => match project::clean_relative(&pr.text) {
                Err(e) => Err(e),
                Ok(to) if &to == path => {
                    self.prompt = None;
                    return;
                }
                Ok(to) => match self.project.as_mut() {
                    None => Err(String::from("Open a folder first")),
                    Some(p) => {
                        let parent = String::from(project::parent(&to));
                        if !parent.is_empty() && !p.fs.exists(&parent) {
                            let _ = p.fs.mkdir(&parent);
                        }
                        match p.fs.rename(path, &to) {
                            Ok(()) => {
                                p.tree.invalidate(project::parent(path));
                                p.tree.invalidate(&parent);
                                p.tree.reveal(&mut *p.fs, &to);
                                let prefix = String::from(path.as_str()) + "/";
                                for d in self.docs.iter_mut() {
                                    if &d.path == path {
                                        d.path = to.clone();
                                    } else if let Some(rest) = d.path.strip_prefix(&prefix) {
                                        d.path = project::join(&to, rest);
                                    }
                                }
                                let mut m = String::from("Renamed to ");
                                m.push_str(&to);
                                self.prompt = None;
                                self.say(&m);
                                return;
                            }
                            Err(e) => Err(e),
                        }
                    }
                },
            },
            PromptKind::Delete { path, is_dir } => match self.project.as_mut() {
                None => Err(String::from("Open a folder first")),
                Some(p) => match p.remove(path, *is_dir) {
                    Ok(()) => {
                        p.refresh();
                        let prefix = String::from(path.as_str()) + "/";
                        let gone: Vec<usize> = self
                            .docs
                            .iter()
                            .enumerate()
                            .filter(|(_, d)| &d.path == path || d.path.starts_with(&prefix))
                            .map(|(i, _)| i)
                            .collect();
                        for i in gone.into_iter().rev() {
                            self.close_doc(i, true);
                        }
                        let mut m = String::from("Deleted ");
                        m.push_str(path);
                        self.prompt = None;
                        self.say(&m);
                        return;
                    }
                    Err(e) => Err(e),
                },
            },
            PromptKind::ProjectName => {
                let name = String::from(pr.text.trim());
                match self.finish_new_project(&name) {
                    Ok(()) => {
                        self.prompt = None;
                        return;
                    }
                    Err(e) => Err(e),
                }
            }
            PromptKind::CloseUnsaved => {
                self.prompt = None;
                return;
            }
            PromptKind::RenameSymbol => {
                let name = String::from(pr.text.trim());
                if name.is_empty() {
                    Err(String::from("Type the new name"))
                } else {
                    self.prompt = None;
                    self.lsp_rename(&name);
                    return;
                }
            }
        };
        if let (Err(e), Some(p)) = (result, self.prompt.as_mut()) {
            p.error = Some(e);
        }
    }

    pub fn prompt_cancel(&mut self) {
        self.input(Input::Key(Key::Esc));
    }

    /// The window was asked to close. True when it may close now.
    pub fn close_requested(&mut self) -> bool {
        let dirty = self.dirty_count();
        if dirty == 0 || self.close_warned {
            return true;
        }
        self.close_warned = true;
        let mut title = text::num(dirty);
        title.push_str(if dirty == 1 {
            " file has"
        } else {
            " files have"
        });
        title.push_str(" unsaved changes. Save them before closing?");
        self.prompt = Some(Prompt {
            kind: PromptKind::CloseUnsaved,
            title,
            text: String::new(),
            error: None,
        });
        false
    }

    // ---- tree --------------------------------------------------------------------

    fn tree_key(&mut self, input: Input) {
        let Some(p) = self.project.as_mut() else {
            self.focus = Focus::Editor;
            return;
        };
        let n = p.tree.rows.len();
        match input {
            Input::Char('j') | Input::Key(Key::Down) => {
                if n > 0 {
                    p.tree.selected = (p.tree.selected + 1).min(n - 1);
                }
            }
            Input::Char('k') | Input::Key(Key::Up) => {
                p.tree.selected = p.tree.selected.saturating_sub(1)
            }
            Input::Char('g') | Input::Key(Key::Home) => p.tree.selected = 0,
            Input::Char('G') | Input::Key(Key::End) => p.tree.selected = n.saturating_sub(1),
            Input::Key(Key::Enter)
            | Input::Char('l')
            | Input::Char('o')
            | Input::Key(Key::Right) => {
                let i = p.tree.selected;
                self.tree_open(i);
            }
            Input::Char('h') | Input::Key(Key::Left) => {
                let Some(row) = p.tree.selected_row().cloned() else {
                    return;
                };
                if row.is_dir && row.expanded {
                    let i = p.tree.selected;
                    p.tree.toggle(&mut *p.fs, i);
                } else if let Some(i) = p
                    .tree
                    .rows
                    .iter()
                    .position(|r| r.path == project::parent(&row.path))
                {
                    p.tree.selected = i;
                }
            }
            Input::Char('a') | Input::Char('%') => self.run(Action::NewFile),
            Input::Char('A') => self.run(Action::NewFolder),
            Input::Char('r') => self.run(Action::Rename),
            Input::Char('d') | Input::Key(Key::Delete) | Input::Char('D') => {
                self.run(Action::Delete)
            }
            Input::Char('R') => {
                p.reload();
                self.say("Reloaded the tree");
            }
            Input::Key(Key::Esc) | Input::Key(Key::Tab) | Input::Ctrl('l') => {
                self.focus = Focus::Editor
            }
            Input::Ctrl('w') => {}
            Input::Char(':') => {
                self.focus = Focus::Editor;
                self.editor_input(Input::Char(':'));
            }
            _ => {}
        }
    }

    /// A tree row was chosen: open the file, or fold the folder.
    pub fn tree_open(&mut self, i: usize) {
        let Some(p) = self.project.as_mut() else {
            return;
        };
        let Some(row) = p.tree.rows.get(i).cloned() else {
            return;
        };
        p.tree.selected = i;
        if row.is_dir {
            p.tree.toggle(&mut *p.fs, i);
        } else {
            self.open_file(&row.path);
        }
    }

    pub fn tree_click(&mut self, i: usize) {
        self.leave_mode();
        self.focus = Focus::Tree;
        self.tree_open(i);
    }

    fn panel_key(&mut self, input: Input) {
        if self.panel_tab == PanelTab::Debug {
            self.debug_panel_key(input);
            return;
        }
        if self.panel_tab == PanelTab::Terminal && self.term_text_input(&input) {
            return;
        }
        match input {
            Input::Char('j') | Input::Key(Key::Down) => {
                if !self.problems.is_empty() {
                    self.problem_selected =
                        (self.problem_selected + 1).min(self.problems.len() - 1);
                }
            }
            Input::Char('k') | Input::Key(Key::Up) => {
                self.problem_selected = self.problem_selected.saturating_sub(1)
            }
            Input::Key(Key::Enter) => {
                let i = self.problem_selected;
                self.jump_to_problem(i);
            }
            _ => self.focus = Focus::Editor,
        }
    }

    // ---- the mouse ---------------------------------------------------------------

    /// A press in pane `pane`'s text at `pos`. `clicks` is 1, 2 (a word) or
    /// 3 (the line); `shift` extends the selection.
    pub fn press_text(&mut self, pane: usize, pos: Pos, clicks: u8, shift: bool) {
        self.palette = None;
        if pane != self.active {
            self.focus_pane(pane);
        }
        self.focus = Focus::Editor;
        if matches!(self.vim.mode, Mode::Command | Mode::Search { .. }) {
            self.leave_mode();
        }
        let Some(d) = self.panes.get(pane).and_then(|p| p.doc) else {
            return;
        };
        let (docs, panes) = (&mut self.docs, &mut self.panes);
        let (Some(doc), Some(p)) = (docs.get_mut(d), panes.get_mut(pane)) else {
            return;
        };
        let pos = doc.buf.clamp(pos);
        if shift {
            let anchor = if self.vim.mode.is_visual() {
                p.cur.anchor
            } else {
                p.cur.pos
            };
            p.cur.pos = pos;
            self.vim.begin_select(&mut doc.buf, &mut p.cur, anchor);
            self.drag_from = Some(anchor);
            return;
        }
        if clicks >= 2 {
            let s = doc.buf.line(pos.line);
            let (a, b) = if clicks == 2 {
                let mut a = pos.col;
                let mut b = pos.col;
                let is_w = |col: usize| {
                    s.get(col..)
                        .and_then(|r| r.chars().next())
                        .is_some_and(text::is_word_char)
                };
                while a > 0 && is_w(doc.buf.prev_col(pos.line, a)) {
                    a = doc.buf.prev_col(pos.line, a);
                }
                while b < s.len() && is_w(b) {
                    b = doc.buf.next_col(pos.line, b);
                }
                (a, b)
            } else {
                (0, s.len())
            };
            if a < b {
                p.cur.pos = Pos::new(pos.line, b);
                self.vim
                    .begin_select(&mut doc.buf, &mut p.cur, Pos::new(pos.line, a));
                self.drag_from = None;
                return;
            }
        }
        if self.vim.mode.is_visual() {
            self.vim.to_normal(&mut doc.buf, &mut p.cur);
            if self.vim.mode == Mode::Normal && self.settings.click_inserts {
                // fall through to insert below
            }
        }
        p.cur.pos = pos;
        p.cur.anchor = pos;
        if self.settings.click_inserts {
            self.vim.begin_insert(&mut doc.buf, &mut p.cur);
        } else {
            self.vim.to_normal(&mut doc.buf, &mut p.cur);
        }
        self.vim.sync_want(&doc.buf, &mut p.cur);
        self.drag_from = Some(pos);
    }

    /// The pointer moved with the button down.
    pub fn drag_text(&mut self, pane: usize, pos: Pos) {
        let Some(from) = self.drag_from else { return };
        if pane != self.active {
            return;
        }
        let Some(d) = self.panes.get(pane).and_then(|p| p.doc) else {
            return;
        };
        let (docs, panes) = (&mut self.docs, &mut self.panes);
        let (Some(doc), Some(p)) = (docs.get_mut(d), panes.get_mut(pane)) else {
            return;
        };
        let pos = doc.buf.clamp(pos);
        if pos == p.cur.pos {
            return;
        }
        p.cur.pos = pos;
        if pos == from {
            return;
        }
        if self.vim.mode != Mode::Select {
            self.vim.begin_select(&mut doc.buf, &mut p.cur, from);
        }
        self.ensure_visible(pane);
    }

    pub fn release(&mut self) {
        self.drag_from = None;
    }

    pub fn dragging(&self) -> bool {
        self.drag_from.is_some()
    }

    /// The mode chip on the status line was clicked: switch between typing
    /// and commands.
    pub fn toggle_mode(&mut self) {
        let a = self.active;
        let Some(d) = self.active_doc_index() else {
            return;
        };
        let (docs, panes) = (&mut self.docs, &mut self.panes);
        let (Some(doc), Some(p)) = (docs.get_mut(d), panes.get_mut(a)) else {
            return;
        };
        if self.vim.mode == Mode::Normal {
            self.vim.begin_insert(&mut doc.buf, &mut p.cur);
        } else {
            self.vim.to_normal(&mut doc.buf, &mut p.cur);
        }
        self.focus = Focus::Editor;
    }

    pub fn click_tab(&mut self, doc: usize) {
        self.palette = None;
        self.show_doc(doc);
    }

    pub fn close_tab(&mut self, doc: usize) {
        self.close_doc(doc, false);
    }

    /// One `key:value` per line about the state, for automated runs.
    pub fn report(&self) -> String {
        let mut s = String::new();
        let mut kv = |k: &str, v: &str| {
            s.push_str(k);
            s.push(':');
            s.push_str(v);
            s.push('\n');
        };
        kv(
            "project",
            self.project
                .as_ref()
                .map(|p| p.name.as_str())
                .unwrap_or("none"),
        );
        kv("files_open", &text::num(self.docs.len()));
        kv("panes", &text::num(self.panes.len()));
        kv("mode", self.vim.mode.label());
        if let Some(d) = self.active_doc() {
            kv("file", &d.path);
            kv("lines", &text::num(d.buf.line_count()));
            kv("first_line", d.buf.line(0));
            kv("modified", if d.buf.modified() { "yes" } else { "no" });
            let p = self.panes[self.active].cur.pos;
            kv("line", &text::num(p.line + 1));
            kv("col", &text::num(p.col + 1));
        }
        kv(
            "tree_rows",
            &text::num(
                self.project
                    .as_ref()
                    .map(|p| p.tree.rows.len())
                    .unwrap_or(0),
            ),
        );
        kv("problems", &text::num(self.problems.len()));
        kv("jobs", &text::num(self.jobs.len()));
        kv("terminals", &text::num(self.terminals.len()));
        kv(
            "lsp",
            match &self.lsp {
                Some(l) if l.client.ready => "ready",
                Some(_) => "starting",
                None => "off",
            },
        );
        kv(
            "debug",
            match self.dap.as_ref().map(|d| d.client.state) {
                Some(crate::dap::DapState::Stopped) => "stopped",
                Some(crate::dap::DapState::Running) => "running",
                Some(crate::dap::DapState::Starting) => "starting",
                Some(crate::dap::DapState::Ended) => "ended",
                None => "off",
            },
        );
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools;
    use crate::vim::parse_keys;

    fn ws() -> Workspace {
        let mut w = Workspace::new();
        w.open_tour();
        w
    }

    fn keys(w: &mut Workspace, k: &str) {
        for i in parse_keys(k) {
            w.input(i);
        }
    }

    fn read(w: &mut Workspace, path: &str) -> String {
        w.project.as_mut().unwrap().fs.read(path).unwrap()
    }

    #[test]
    fn the_tour_opens_main_rs_in_normal_mode() {
        let w = ws();
        assert_eq!(w.active_doc().unwrap().path, "src/main.rs");
        assert_eq!(w.vim.mode, Mode::Normal);
        assert!(w.report().contains("project:tour"));
    }

    #[test]
    fn edit_and_write_through_the_project() {
        let mut w = ws();
        keys(&mut w, "ggO// edited<Esc>:w<CR>");
        assert!(read(&mut w, "src/main.rs").starts_with("// edited\n//! The tour"));
        assert!(!w.active_doc().unwrap().buf.modified());
        assert!(w.message().unwrap().0.contains("written"));
        keys(&mut w, ":w src/copy.rs<CR>");
        assert_eq!(w.active_doc().unwrap().path, "src/copy.rs");
        assert!(read(&mut w, "src/copy.rs").starts_with("// edited"));
        assert!(w
            .project
            .as_ref()
            .unwrap()
            .tree
            .rows
            .iter()
            .any(|r| r.path == "src/copy.rs"));
    }

    #[test]
    fn q_refuses_unsaved_changes_and_q_bang_drops_them() {
        let mut w = ws();
        keys(&mut w, "dd:q<CR>");
        assert_eq!(w.docs.len(), 1);
        assert!(w.message().unwrap().1);
        keys(&mut w, ":q!<CR>");
        assert_eq!(w.docs.len(), 0);
        assert!(w.active_doc().is_none());
    }

    #[test]
    fn e_opens_files_and_bn_cycles_tabs() {
        let mut w = ws();
        keys(&mut w, ":e src/shapes.rs<CR>");
        assert_eq!(w.active_doc().unwrap().path, "src/shapes.rs");
        assert_eq!(w.docs.len(), 2);
        keys(&mut w, ":bn<CR>");
        assert_eq!(w.active_doc().unwrap().path, "src/main.rs");
        keys(&mut w, ":e ../../etc/passwd<CR>");
        assert!(
            w.message().unwrap().1,
            "a path outside the project is refused"
        );
        keys(&mut w, ":e notes.txt<CR>itext<Esc>:w<CR>");
        assert_eq!(read(&mut w, "notes.txt"), "text");
    }

    #[test]
    fn splits_share_a_file_and_move_focus() {
        let mut w = ws();
        keys(&mut w, "<C-w>v");
        assert_eq!(w.panes.len(), 2);
        assert_eq!(w.active, 1);
        keys(&mut w, ":e src/shapes.rs<CR>");
        assert_eq!(w.pane_doc(0).unwrap().path, "src/main.rs");
        assert_eq!(w.pane_doc(1).unwrap().path, "src/shapes.rs");
        keys(&mut w, "<C-w>w");
        assert_eq!(w.active, 0);
        // An edit in one pane keeps the other's cursor valid.
        keys(&mut w, ":vsp<CR>Gdgg");
        assert_eq!(w.panes.len(), 3);
        for p in &w.panes {
            let d = &w.docs[p.doc.unwrap()];
            assert!(p.cur.pos.line <= d.buf.last_line());
        }
        keys(&mut w, ":close<CR>:only<CR>");
        assert_eq!(w.panes.len(), 1);
        keys(&mut w, "<C-w>v<C-w>v<C-w>v<C-w>v");
        assert_eq!(w.panes.len(), MAX_PANES);
    }

    #[test]
    fn tree_new_rename_delete_with_a_confirm() {
        let mut w = ws();
        w.run(Action::FocusTree);
        assert_eq!(w.focus, Focus::Tree);
        // Select src/main.rs, then make a file beside it.
        let i = w
            .project
            .as_ref()
            .unwrap()
            .tree
            .rows
            .iter()
            .position(|r| r.path == "src/main.rs")
            .unwrap();
        w.project.as_mut().unwrap().tree.selected = i;
        keys(&mut w, "a");
        assert!(matches!(
            w.prompt.as_ref().unwrap().kind,
            PromptKind::NewFile { .. }
        ));
        keys(&mut w, "util.rs<CR>");
        assert!(w.prompt.is_none());
        assert_eq!(w.active_doc().unwrap().path, "src/util.rs");
        assert_eq!(w.vim.mode, Mode::Insert, "a new file is ready to type in");
        keys(&mut w, "pub fn x() {}<Esc>:w<CR>");
        assert_eq!(read(&mut w, "src/util.rs"), "pub fn x() {}");
        // Rename it from the tree.
        w.run(Action::FocusTree);
        let i = w
            .project
            .as_ref()
            .unwrap()
            .tree
            .rows
            .iter()
            .position(|r| r.path == "src/util.rs")
            .unwrap();
        w.project.as_mut().unwrap().tree.selected = i;
        keys(&mut w, "r<C-u>src/helpers.rs<CR>");
        assert_eq!(
            w.active_doc().unwrap().path,
            "src/helpers.rs",
            "the open file follows the rename"
        );
        assert!(w.project.as_mut().unwrap().fs.exists("src/helpers.rs"));
        // Delete asks first; n keeps it, y removes it and closes its tab.
        w.run(Action::FocusTree);
        keys(&mut w, "d");
        assert!(w
            .prompt
            .as_ref()
            .unwrap()
            .title
            .starts_with("Delete src/helpers.rs"));
        keys(&mut w, "n");
        assert!(w.project.as_mut().unwrap().fs.exists("src/helpers.rs"));
        keys(&mut w, "dy");
        assert!(!w.project.as_mut().unwrap().fs.exists("src/helpers.rs"));
        assert!(!w.docs.iter().any(|d| d.path == "src/helpers.rs"));
    }

    #[test]
    fn a_bad_new_name_says_why_and_keeps_the_prompt() {
        let mut w = ws();
        w.run(Action::NewFile);
        keys(&mut w, "../escape.rs<CR>");
        let p = w.prompt.as_ref().unwrap();
        assert!(p.error.as_ref().unwrap().contains(".."));
        keys(&mut w, "<Esc>");
        assert!(w.prompt.is_none());
    }

    #[test]
    fn new_project_writes_a_cargo_skeleton() {
        let mut w = Workspace::new();
        w.new_project_folder("Hello World", Box::new(MemFs::new()));
        assert_eq!(w.prompt.as_ref().unwrap().text, "hello-world");
        keys(&mut w, "<C-u>Bad Name<CR>");
        assert!(w.prompt.as_ref().unwrap().error.is_some());
        keys(&mut w, "<C-u>hello<CR>");
        assert!(w.prompt.is_none());
        assert!(read(&mut w, "Cargo.toml").contains("name = \"hello\""));
        assert_eq!(w.active_doc().unwrap().path, "src/main.rs");
        assert!(read(&mut w, ".gitignore").contains("target"));
        // A folder that is already a crate just opens.
        let mut w = Workspace::new();
        w.new_project_folder(
            "x",
            Box::new(MemFs::with(&[("Cargo.toml", "[package]\nname=\"x\"")])),
        );
        assert!(w.prompt.is_none());
        assert!(w.project.is_some());
    }

    #[test]
    fn palette_filters_and_runs_actions() {
        let mut w = ws();
        assert!(w.shortcut('p', true));
        keys(&mut w, "split");
        keys(&mut w, "<CR>");
        assert_eq!(w.panes.len(), 2);
        assert!(w.palette.is_none());
        // Ctrl+P finds files.
        assert!(w.shortcut('p', false));
        keys(&mut w, "shap<CR>");
        assert_eq!(w.active_doc().unwrap().path, "src/shapes.rs");
        // Esc closes it without doing anything.
        w.shortcut('p', true);
        keys(&mut w, "<Esc>");
        assert!(w.palette.is_none());
    }

    #[test]
    fn the_tour_says_it_has_nothing_on_disk_to_build() {
        let mut w = ws();
        w.run(Action::Tool(Tool::Build));
        assert!(w.message().unwrap().0.contains(tools::NEEDS_FOLDER));
        keys(&mut w, ":cargo test<CR>");
        assert!(w.message().unwrap().0.contains(tools::NEEDS_FOLDER));
        w.run(Action::Tool(Tool::Terminal));
        assert!(w.panel_open);
        assert_eq!(w.panel_tab, PanelTab::Terminal);
        assert!(w.message().unwrap().0.contains(tools::NEEDS_FOLDER));
    }

    #[test]
    fn problems_jump_to_their_place() {
        let mut w = ws();
        w.set_problems(tools::parse_cargo(
            "src/shapes.rs:18:9: error[E0308]: mismatched types\n",
        ));
        assert!(w.panel_open);
        w.jump_to_problem(0);
        assert_eq!(w.active_doc().unwrap().path, "src/shapes.rs");
        assert_eq!(w.panes[w.active].cur.pos, Pos::new(17, 8));
    }

    #[test]
    fn breakpoints_toggle_and_follow_edits() {
        let mut w = ws();
        keys(&mut w, "10G");
        w.run(Action::ToggleBreakpoint);
        assert!(w.active_doc().unwrap().breakpoints.contains(&9));
        keys(&mut w, "ggOx<Esc>");
        assert!(
            w.active_doc().unwrap().breakpoints.contains(&10),
            "a line added above moves it down"
        );
        keys(&mut w, "11Gdd");
        assert!(
            w.active_doc().unwrap().breakpoints.is_empty(),
            "deleting its line removes it"
        );
    }

    #[test]
    fn clicking_starts_typing_and_dragging_selects() {
        let mut w = ws();
        w.press_text(0, Pos::new(0, 4), 1, false);
        assert_eq!(w.vim.mode, Mode::Insert, "a click into text starts typing");
        keys(&mut w, "ABC");
        assert!(w
            .active_doc()
            .unwrap()
            .buf
            .line(0)
            .starts_with("//! ABCThe tour"));
        w.press_text(0, Pos::new(0, 0), 1, false);
        w.drag_text(0, Pos::new(0, 3));
        w.release();
        assert_eq!(w.vim.mode, Mode::Select);
        keys(&mut w, "X");
        assert!(
            w.active_doc().unwrap().buf.line(0).starts_with("X ABC"),
            "typing replaces the selection"
        );
        // A double click selects a word.
        w.press_text(0, Pos::new(5, 6), 2, false);
        assert_eq!(w.vim.mode, Mode::Select);
        let (a, b, _) = w.vim.selection(&w.docs[0].buf, &w.panes[0].cur).unwrap();
        assert_eq!(w.docs[0].buf.slice(a, b), "shapes");
        // With clicks set to normal mode, a click only moves.
        w.run(Action::ClickMode);
        w.press_text(0, Pos::new(2, 2), 1, false);
        assert_eq!(w.vim.mode, Mode::Normal);
        assert_eq!(w.panes[0].cur.pos, Pos::new(2, 2));
    }

    #[test]
    fn select_all_copy_and_paste() {
        let mut w = ws();
        keys(&mut w, ":e README.md<CR>");
        assert!(w.shortcut('a', false));
        assert!(w.shortcut('c', false));
        let readme = read(&mut w, "README.md");
        assert_eq!(w.vim.register('"').unwrap().text, readme);
        w.input(Input::Paste(String::from("pasted ")));
        assert!(w.active_doc().unwrap().buf.text().contains("pasted "));
    }

    #[test]
    fn closing_with_unsaved_files_asks_once() {
        let mut w = ws();
        keys(&mut w, "x");
        assert!(!w.close_requested());
        assert_eq!(w.prompt.as_ref().unwrap().kind, PromptKind::CloseUnsaved);
        keys(&mut w, "s");
        assert_eq!(w.host, Some(HostRequest::Close));
        assert_eq!(w.dirty_count(), 0);
        let mut w = ws();
        keys(&mut w, "x");
        assert!(!w.close_requested());
        keys(&mut w, "<Esc>");
        assert!(w.host.is_none());
        assert!(!w.close_requested(), "a cancel asks again next time");
        keys(&mut w, "d");
        assert_eq!(w.host, Some(HostRequest::Close));
    }

    #[test]
    fn colon_works_with_nothing_open() {
        let mut w = Workspace::new();
        keys(&mut w, ":theme light<CR>");
        assert_eq!(w.settings.theme, ThemeChoice::Light);
        assert!(w.settings_dirty);
        keys(&mut w, "x");
        assert!(w.message().unwrap().0.contains("take the tour"));
        keys(&mut w, ":nonsense<CR>");
        assert!(w.message().unwrap().1);
    }

    #[test]
    fn settings_round_trip() {
        let s = Settings {
            theme: ThemeChoice::Dark,
            click_inserts: false,
            relative_numbers: true,
            tree: false,
            font: 15.5,
            format_on_save: false,
        };
        assert_eq!(Settings::from_text(&s.to_text()), s);
        assert_eq!(Settings::from_text("garbage"), Settings::default());
    }

    #[test]
    fn scrolling_follows_the_cursor() {
        let mut w = ws();
        w.panes[0].rows = 10;
        keys(&mut w, "G");
        let p = w.panes[0];
        let last = w.docs[0].buf.last_line() as f32;
        assert!(p.top <= last && p.top + 10.0 > last);
        keys(&mut w, "gg");
        assert_eq!(w.panes[0].top, 0.0);
        w.scroll(0, 5.5, 0.0);
        assert_eq!(w.panes[0].top, 5.5);
        keys(&mut w, "zt");
        let line = w.panes[0].cur.pos.line as f32;
        assert_eq!(w.panes[0].top, line);
    }

    #[test]
    fn a_big_file_stays_fast() {
        let mut w = ws();
        let mut big = String::new();
        for i in 0..20_000 {
            big.push_str("    let value = compute(");
            big.push_str(&text::num(i));
            big.push_str("); // line\n");
        }
        w.project
            .as_mut()
            .unwrap()
            .fs
            .write("src/big.rs", &big)
            .unwrap();
        keys(&mut w, ":e src/big.rs<CR>G");
        assert_eq!(w.panes[0].cur.pos.line, 20_000);
        // Highlight state down to the bottom, then edit at the top.
        let d = w.active_doc_index().unwrap();
        let doc = &mut w.docs[d];
        let buf = doc.buf.clone();
        doc.hl.state_at(20_000, |i| String::from(buf.line(i)));
        keys(&mut w, "ggdd:%s/compute/calc/g<CR>");
        assert!(w.message().unwrap().0.starts_with("19999 substitutions"));
        keys(&mut w, "u");
        assert_eq!(w.docs[d].buf.line_count(), 20_000);
    }
}
