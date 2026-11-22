//! The whole app's state and what every key and click does to it.
//!
//! Nothing here draws or calls the host: the guest draws an [`App`] and
//! turns window events into calls on it, and asks the host for the few
//! things only it can do ([`HostRequest`]: a folder picker, the clipboard).
//! That keeps every behaviour a person relies on -- opening, renaming,
//! linking, searching, switching -- provable with `cargo test`.

use crate::buffer::Pos;
use crate::editor::{Doc, Mode};
use crate::search::Job;
use crate::text::{fuzzy, lower};
use crate::vault::{Backlink, Vault};
use crate::vfs::{file_name, join, parent};
use alloc::boxed::Box;
use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec::Vec;

/// Modifier keys, as the host reports them. `primary` is Cmd on a Mac and
/// Ctrl elsewhere.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub primary: bool,
    pub alt: bool,
    pub ctrl: bool,
}

/// A single-line text input: the search box, the switcher, a rename.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Input {
    pub text: String,
    pub caret: usize,
    pub anchor: Option<usize>,
}

impl Input {
    pub fn with(text: &str) -> Input {
        Input {
            text: String::from(text),
            caret: text.len(),
            anchor: None,
        }
    }

    pub fn selection(&self) -> Option<(usize, usize)> {
        let a = self.anchor?;
        (a != self.caret).then(|| (a.min(self.caret), a.max(self.caret)))
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.caret = self.text.len();
    }

    fn cut(&mut self) -> bool {
        if let Some((a, b)) = self.selection() {
            self.text.replace_range(a..b, "");
            self.caret = a;
            self.anchor = None;
            return true;
        }
        self.anchor = None;
        false
    }

    fn prev(&self, at: usize) -> usize {
        self.text
            .get(..at)
            .and_then(|h| h.char_indices().next_back())
            .map_or(0, |(i, _)| i)
    }

    fn next(&self, at: usize) -> usize {
        self.text
            .get(at..)
            .and_then(|t| t.chars().next())
            .map_or(self.text.len(), |c| at + c.len_utf8())
    }

    /// Type text over the selection. Line breaks become spaces.
    pub fn insert(&mut self, s: &str) {
        let clean: String = s
            .chars()
            .map(|c| if c == '\n' || c == '\t' { ' ' } else { c })
            .filter(|c| !c.is_control())
            .collect();
        if clean.is_empty() {
            return;
        }
        self.cut();
        self.text.insert_str(self.caret, &clean);
        self.caret += clean.len();
    }

    /// The editing keys. True when the text changed.
    pub fn key(&mut self, key: &str, m: Mods) -> bool {
        match key {
            "Backspace" => {
                if self.cut() {
                    return true;
                }
                if self.caret == 0 {
                    return false;
                }
                let from = if m.primary {
                    0
                } else if m.alt {
                    let head = self.text.get(..self.caret).unwrap_or("");
                    head.trim_end().rfind(' ').map_or(0, |i| i + 1)
                } else {
                    self.prev(self.caret)
                };
                self.text.replace_range(from..self.caret, "");
                self.caret = from;
                true
            }
            "Delete" => {
                if self.cut() {
                    return true;
                }
                if self.caret >= self.text.len() {
                    return false;
                }
                let to = self.next(self.caret);
                self.text.replace_range(self.caret..to, "");
                true
            }
            "ArrowLeft" | "ArrowRight" | "Home" | "End" => {
                let to = match key {
                    "Home" => 0,
                    "End" => self.text.len(),
                    "ArrowLeft" if m.primary => 0,
                    "ArrowRight" if m.primary => self.text.len(),
                    "ArrowLeft" => match (self.selection(), m.shift) {
                        (Some((a, _)), false) => a,
                        _ => self.prev(self.caret),
                    },
                    _ => match (self.selection(), m.shift) {
                        (Some((_, b)), false) => b,
                        _ => self.next(self.caret),
                    },
                };
                if m.shift {
                    self.anchor.get_or_insert(self.caret);
                } else {
                    self.anchor = None;
                }
                self.caret = to;
                false
            }
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeftPane {
    Files,
    Search,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RightPane {
    Backlinks,
    Outgoing,
    Tags,
    Outline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Editor,
    Tree,
    Search,
}

/// One row of the file tree as drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Folder(usize),
    Note(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub depth: u16,
    pub kind: RowKind,
}

/// A name being changed in the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rename {
    pub path: String,
    pub folder: bool,
    pub input: Input,
}

pub struct Explorer {
    pub expanded: BTreeSet<String>,
    pub rows: Vec<Row>,
    rows_for: u64,
    rows_dirty: bool,
    pub scroll: f32,
    /// The row picked with the keyboard or a click (a path).
    pub selected: Option<String>,
    pub rename: Option<Rename>,
}

/// One open tab: a note, or the empty "New tab" page.
pub struct Tab {
    pub doc: Option<Doc>,
    back: Vec<String>,
    fwd: Vec<String>,
}

impl Tab {
    pub fn title(&self) -> &str {
        self.doc.as_ref().map_or("New tab", |d| d.title.as_str())
    }
}

/// Everything the command palette can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    QuickSwitcher,
    Palette,
    SearchVault,
    NewNote,
    NewFolder,
    NewTab,
    CloseTab,
    NextTab,
    PrevTab,
    Save,
    ToggleReading,
    ToggleSource,
    ToggleLeft,
    ToggleRight,
    ShowFiles,
    ShowBacklinks,
    ShowOutgoing,
    ShowTags,
    ShowOutline,
    Rename,
    Delete,
    MoveTo,
    Back,
    Forward,
    Bold,
    Italic,
    Strike,
    Highlight,
    InlineCode,
    ToggleTask,
    InsertLink,
    Undo,
    Redo,
    SelectAll,
    Theme,
    OpenVault,
    Reveal,
    CollapseAll,
    CloseVault,
}

pub struct Command {
    pub cmd: Cmd,
    pub title: &'static str,
    pub keys: &'static str,
}

const fn c(cmd: Cmd, title: &'static str, keys: &'static str) -> Command {
    Command { cmd, title, keys }
}

/// Every command, in the order the palette lists them.
pub const COMMANDS: &[Command] = &[
    c(Cmd::QuickSwitcher, "Quick switcher: Open a note", "Cmd+O"),
    c(
        Cmd::SearchVault,
        "Search: Search in all notes",
        "Cmd+Shift+F",
    ),
    c(Cmd::NewNote, "Create new note", "Cmd+N"),
    c(Cmd::NewFolder, "Files: Create new folder", ""),
    c(Cmd::NewTab, "Open a new tab", "Cmd+T"),
    c(Cmd::CloseTab, "Close current tab", "Cmd+W"),
    c(Cmd::NextTab, "Go to next tab", "Ctrl+Tab"),
    c(Cmd::PrevTab, "Go to previous tab", "Ctrl+Shift+Tab"),
    c(Cmd::Save, "Save current note", "Cmd+S"),
    c(Cmd::ToggleReading, "Toggle reading view", "Cmd+E"),
    c(Cmd::ToggleSource, "Toggle live preview and source mode", ""),
    c(Cmd::ToggleLeft, "Toggle left sidebar", "Cmd+\\"),
    c(Cmd::ToggleRight, "Toggle right sidebar", "Cmd+Shift+\\"),
    c(Cmd::ShowFiles, "Files: Show file explorer", "Cmd+Shift+E"),
    c(Cmd::ShowBacklinks, "Backlinks: Show backlinks", ""),
    c(Cmd::ShowOutgoing, "Outgoing links: Show outgoing links", ""),
    c(Cmd::ShowTags, "Tags: Show tags", ""),
    c(Cmd::ShowOutline, "Outline: Show outline", ""),
    c(Cmd::Rename, "Rename note", "F2"),
    c(Cmd::Delete, "Delete current note", ""),
    c(Cmd::MoveTo, "Move current note to another folder", ""),
    c(Cmd::Back, "Navigate back", "Cmd+Alt+Left"),
    c(Cmd::Forward, "Navigate forward", "Cmd+Alt+Right"),
    c(Cmd::Bold, "Toggle bold", "Cmd+B"),
    c(Cmd::Italic, "Toggle italic", "Cmd+I"),
    c(Cmd::Strike, "Toggle strikethrough", ""),
    c(Cmd::Highlight, "Toggle highlight", ""),
    c(Cmd::InlineCode, "Toggle code", ""),
    c(Cmd::ToggleTask, "Toggle checkbox status", "Cmd+L"),
    c(Cmd::InsertLink, "Add internal link", "Cmd+K"),
    c(Cmd::Undo, "Undo", "Cmd+Z"),
    c(Cmd::Redo, "Redo", "Cmd+Shift+Z"),
    c(Cmd::SelectAll, "Select all", "Cmd+A"),
    c(Cmd::Theme, "Switch between dark and light", ""),
    c(Cmd::Reveal, "Files: Reveal current note in the tree", ""),
    c(Cmd::CollapseAll, "Files: Collapse all folders", ""),
    c(Cmd::OpenVault, "Open another vault", ""),
    c(Cmd::CloseVault, "Close this vault", ""),
];

/// What a row of a list prompt does when chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    Open(String),
    Create(String),
    Run(Cmd),
    MoveInto(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub title: String,
    pub detail: String,
    pub hint: String,
    pub pick: Pick,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListKind {
    Switcher,
    Palette,
    /// Choosing the folder a note or folder (the path) moves into.
    MoveTo(String),
}

pub struct ListPrompt {
    pub kind: ListKind,
    pub input: Input,
    pub items: Vec<Item>,
    pub sel: usize,
    pub first: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuAction {
    NewNote(String),
    NewFolder(String),
    OpenInNewTab(String),
    Rename(String),
    MoveTo(String),
    Delete(String),
}

pub struct Menu {
    pub x: f32,
    pub y: f32,
    pub items: Vec<(String, MenuAction)>,
}

pub struct Confirm {
    pub title: String,
    pub body: String,
    pub yes: &'static str,
    pub path: String,
}

pub enum Overlay {
    List(ListPrompt),
    Menu(Menu),
    Confirm(Confirm),
}

/// `[[` completion under the caret.
pub struct Completion {
    pub query: String,
    pub items: Vec<(String, String)>,
    pub sel: usize,
}

/// What only the host can do, asked for by the app and done by the guest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostRequest {
    /// Show the folder picker; `create` when making a new vault there.
    PickVault {
        create: bool,
    },
    /// Open the sample vault (held in memory).
    Sample,
    Copy(String),
}

/// What a press landed on, as the guest drew it.
#[derive(Debug, Clone, PartialEq)]
pub enum Hit {
    Welcome(u8),
    Ribbon(Cmd),
    LeftTab(LeftPane),
    RightTab(RightPane),
    TreeRow(usize),
    TreeBlank,
    Tab(usize),
    TabClose(usize),
    NewTab,
    Nav(bool),
    ModeToggle,
    SearchBox,
    SearchFile(usize),
    SearchLine(usize, usize),
    Backlink(usize, usize),
    Outgoing(usize),
    Tag(usize),
    Outline(usize),
    ListItem(usize),
    ListBox,
    MenuItem(usize),
    ConfirmYes,
    ConfirmNo,
    Completion(usize),
    Scrim,
    Command(Cmd),
    LinkNote(String),
}

/// A pane's answer computed from the open note, recomputed only when the
/// note changed and a moment has passed, never per keystroke.
struct Cached<T> {
    path: String,
    version: u64,
    generation: u64,
    at: u64,
    data: T,
}

pub struct App {
    pub vault: Option<Vault>,
    pub tabs: Vec<Tab>,
    pub active: usize,
    pub left: LeftPane,
    pub right: RightPane,
    pub left_open: bool,
    pub right_open: bool,
    pub left_w: f32,
    pub right_w: f32,
    pub focus: Focus,
    pub explorer: Explorer,
    pub search: Input,
    pub job: Option<Job>,
    pub search_scroll: f32,
    pub search_collapsed: BTreeSet<String>,
    pub overlay: Option<Overlay>,
    pub completion: Option<Completion>,
    pub host: Vec<HostRequest>,
    pub status: Option<(String, u64)>,
    pub dark: bool,
    pub now: u64,
    pub recent: Vec<String>,
    pub right_scroll: f32,
    backlinks: Option<Cached<Vec<Backlink>>>,
    outline: Option<Cached<Vec<crate::md::Heading>>>,
    outgoing: Option<Cached<Vec<(String, bool)>>>,
    tags: Option<(u64, Vec<(String, usize)>)>,
    /// Notes saved this run, for the report.
    pub saves: usize,
    pub closing: bool,
}

/// How long a pause in typing is before the note is saved.
pub const SAVE_AFTER_MS: u64 = 2_000;
const PANE_SETTLE_MS: u64 = 400;
const LIST_MAX: usize = 60;
const BACKLINK_CONTEXT: usize = 100;

impl Default for App {
    fn default() -> Self {
        App::new()
    }
}

impl App {
    pub fn new() -> App {
        App {
            vault: None,
            tabs: alloc::vec![Tab {
                doc: None,
                back: Vec::new(),
                fwd: Vec::new()
            }],
            active: 0,
            left: LeftPane::Files,
            right: RightPane::Backlinks,
            left_open: true,
            right_open: true,
            left_w: 260.0,
            right_w: 280.0,
            focus: Focus::Editor,
            explorer: Explorer {
                expanded: BTreeSet::new(),
                rows: Vec::new(),
                rows_for: 0,
                rows_dirty: true,
                scroll: 0.0,
                selected: None,
                rename: None,
            },
            search: Input::default(),
            job: None,
            search_scroll: 0.0,
            search_collapsed: BTreeSet::new(),
            overlay: None,
            completion: None,
            host: Vec::new(),
            status: None,
            dark: true,
            now: 0,
            recent: Vec::new(),
            right_scroll: 0.0,
            backlinks: None,
            outline: None,
            outgoing: None,
            tags: None,
            saves: 0,
            closing: false,
        }
    }

    // ---- the vault -------------------------------------------------------

    pub fn open_vault(&mut self, vault: Vault) {
        self.save_all();
        self.vault = Some(vault);
        self.tabs = alloc::vec![Tab {
            doc: None,
            back: Vec::new(),
            fwd: Vec::new()
        }];
        self.active = 0;
        self.explorer.expanded.clear();
        self.explorer.rows_dirty = true;
        self.explorer.scroll = 0.0;
        self.explorer.selected = None;
        self.explorer.rename = None;
        self.job = None;
        self.search = Input::default();
        self.overlay = None;
        self.completion = None;
        self.recent.clear();
        self.backlinks = None;
        self.outline = None;
        self.outgoing = None;
        self.tags = None;
        let problems = self
            .vault
            .as_ref()
            .map(|v| v.problems.clone())
            .unwrap_or_default();
        if let Some(p) = problems.first() {
            let mut msg = String::from("Could not read ");
            msg.push_str(p);
            self.say(&msg);
        }
    }

    pub fn close_vault(&mut self) {
        self.save_all();
        *self = App {
            dark: self.dark,
            ..App::new()
        };
    }

    pub fn say(&mut self, msg: &str) {
        self.status = Some((String::from(msg), self.now + 4_000));
    }

    // ---- tabs and notes ---------------------------------------------------

    pub fn tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    pub fn doc(&self) -> Option<&Doc> {
        self.tabs.get(self.active).and_then(|t| t.doc.as_ref())
    }

    pub fn doc_mut(&mut self) -> Option<&mut Doc> {
        self.tabs.get_mut(self.active).and_then(|t| t.doc.as_mut())
    }

    /// The text of a note as a tab holds it (unsaved edits included).
    pub fn open_text(&self, path: &str) -> Option<String> {
        self.tabs
            .iter()
            .filter_map(|t| t.doc.as_ref())
            .find(|d| d.path == path)
            .map(Doc::text)
    }

    fn load(&mut self, path: &str) -> Option<Doc> {
        // A note open in another tab is read from there, unsaved text and all.
        let held = self.open_text(path);
        let v = self.vault.as_mut()?;
        let title = v.note(path).map(|n| n.name.clone())?;
        let text = match held {
            Some(t) => t,
            None => match read_note(v, path) {
                Ok(t) => t,
                Err(why) => {
                    let mut msg = String::from("Could not open ");
                    msg.push_str(file_name(path));
                    msg.push_str(": ");
                    msg.push_str(&why);
                    self.say(&msg);
                    return None;
                }
            },
        };
        Some(Doc::new(path, &title, &text))
    }

    /// Open a note: in this tab (keeping the way back), or in a new one.
    pub fn open(&mut self, path: &str, new_tab: bool) {
        if !new_tab && self.doc().is_some_and(|d| d.path == path) {
            self.focus = Focus::Editor;
            return;
        }
        let Some(doc) = self.load(path) else {
            return;
        };
        if new_tab || self.tabs.is_empty() {
            self.tabs.push(Tab {
                doc: Some(doc),
                back: Vec::new(),
                fwd: Vec::new(),
            });
            self.active = self.tabs.len() - 1;
        } else {
            self.save_active();
            let Some(tab) = self.tabs.get_mut(self.active) else {
                return;
            };
            if let Some(old) = tab.doc.take() {
                tab.back.push(old.path);
            }
            tab.fwd.clear();
            tab.doc = Some(doc);
        }
        self.after_open(path);
    }

    fn after_open(&mut self, path: &str) {
        self.recent.retain(|p| p != path);
        self.recent.insert(0, String::from(path));
        self.recent.truncate(30);
        self.explorer.selected = Some(String::from(path));
        self.focus = Focus::Editor;
        self.completion = None;
        self.right_scroll = 0.0;
    }

    /// Back (`false`) or forward through this tab's history.
    pub fn navigate(&mut self, forward: bool) {
        let target = {
            let Some(tab) = self.tabs.get_mut(self.active) else {
                return;
            };
            let stack = if forward { &mut tab.fwd } else { &mut tab.back };
            match stack.pop() {
                Some(p) => p,
                None => return,
            }
        };
        let Some(doc) = self.load(&target) else {
            return;
        };
        self.save_active();
        if let Some(tab) = self.tabs.get_mut(self.active) {
            if let Some(old) = tab.doc.take() {
                if forward {
                    tab.back.push(old.path);
                } else {
                    tab.fwd.push(old.path);
                }
            }
            tab.doc = Some(doc);
        }
        self.after_open(&target);
    }

    pub fn can_navigate(&self, forward: bool) -> bool {
        self.tab().is_some_and(|t| {
            if forward {
                !t.fwd.is_empty()
            } else {
                !t.back.is_empty()
            }
        })
    }

    pub fn new_tab(&mut self) {
        self.tabs.push(Tab {
            doc: None,
            back: Vec::new(),
            fwd: Vec::new(),
        });
        self.active = self.tabs.len() - 1;
        self.completion = None;
    }

    pub fn close_tab(&mut self, i: usize) {
        self.save_tab(i);
        if i < self.tabs.len() {
            self.tabs.remove(i);
        }
        if self.tabs.is_empty() {
            self.new_tab();
        }
        if self.active >= self.tabs.len() || (i < self.active) {
            self.active = self.active.saturating_sub(1).min(self.tabs.len() - 1);
        }
        self.completion = None;
    }

    pub fn select_tab(&mut self, i: usize) {
        if i < self.tabs.len() {
            self.save_active();
            self.active = i;
            self.completion = None;
            if let Some(p) = self.doc().map(|d| d.path.clone()) {
                self.explorer.selected = Some(p);
            }
        }
    }

    // ---- saving ------------------------------------------------------------

    fn save_tab(&mut self, i: usize) -> bool {
        let Some(v) = self.vault.as_mut() else {
            return false;
        };
        let Some(doc) = self.tabs.get_mut(i).and_then(|t| t.doc.as_mut()) else {
            return false;
        };
        if !doc.modified() {
            doc.dirty_since = None;
            return true;
        }
        let text = doc.text();
        match write_note(v, &doc.path, &text) {
            Ok(()) => {
                doc.mark_saved();
                v.index_text(&doc.path, &text);
                self.saves += 1;
                true
            }
            Err(why) => {
                let mut msg = String::from("Could not save ");
                msg.push_str(&doc.title);
                msg.push_str(": ");
                msg.push_str(&why);
                doc.save_error = Some(why);
                doc.dirty_since = Some(self.now);
                self.say(&msg);
                false
            }
        }
    }

    pub fn save_active(&mut self) -> bool {
        self.save_tab(self.active)
    }

    pub fn save_all(&mut self) {
        for i in 0..self.tabs.len() {
            self.save_tab(i);
        }
    }

    // ---- the tree ----------------------------------------------------------

    /// The rows of the file tree, rebuilt when the tree or what is
    /// expanded changed.
    pub fn rows(&mut self) -> &[Row] {
        let gen = self.vault.as_ref().map_or(0, |v| v.generation);
        if self.explorer.rows_dirty || self.explorer.rows_for != gen {
            let mut rows = Vec::new();
            if let Some(v) = self.vault.as_ref() {
                push_rows(v, 0, 0, &self.explorer.expanded, &mut rows);
            }
            self.explorer.rows = rows;
            self.explorer.rows_dirty = false;
            self.explorer.rows_for = gen;
        }
        &self.explorer.rows
    }

    pub fn row_path(&self, row: &Row) -> Option<String> {
        let v = self.vault.as_ref()?;
        Some(match row.kind {
            RowKind::Folder(f) => v.folders.get(f)?.path.clone(),
            RowKind::Note(n) => v.notes.get(n)?.path.clone(),
        })
    }

    pub fn toggle_folder(&mut self, path: &str) {
        if !self.explorer.expanded.remove(path) {
            self.explorer.expanded.insert(String::from(path));
        }
        self.explorer.rows_dirty = true;
    }

    /// Open every folder above `path`, so its row is in the tree.
    pub fn reveal(&mut self, path: &str) {
        let mut d = parent(path);
        while !d.is_empty() {
            self.explorer.expanded.insert(String::from(d));
            d = parent(d);
        }
        self.explorer.selected = Some(String::from(path));
        self.explorer.rows_dirty = true;
        self.left = LeftPane::Files;
        self.left_open = true;
    }

    /// The folder new notes go in: the one picked in the tree, or the one
    /// the picked note is in, or the root.
    fn target_folder(&self) -> String {
        let Some(v) = self.vault.as_ref() else {
            return String::new();
        };
        match self.explorer.selected.as_deref() {
            Some(p) if v.folder_id(p).is_some() => String::from(p),
            Some(p) if v.note(p).is_some() => String::from(parent(p)),
            _ => String::new(),
        }
    }

    pub fn new_note_in(&mut self, folder: &str) {
        let made = match self.vault.as_mut() {
            Some(v) => v.create_note(folder, "Untitled", ""),
            None => return,
        };
        match made {
            Ok(path) => {
                self.reveal(&path);
                self.open(&path, false);
                // Name it straight away, the way a new note asks for one.
                self.start_rename(&path);
            }
            Err(why) => self.say(&why),
        }
    }

    pub fn new_folder_in(&mut self, parent_dir: &str) {
        let made = match self.vault.as_mut() {
            Some(v) => v.create_folder(parent_dir, "Untitled"),
            None => return,
        };
        match made {
            Ok(path) => {
                self.reveal(&path);
                self.start_rename(&path);
            }
            Err(why) => self.say(&why),
        }
    }

    pub fn start_rename(&mut self, path: &str) {
        let Some(v) = self.vault.as_ref() else {
            return;
        };
        let folder = v.folder_id(path).is_some();
        let shown = if folder {
            String::from(file_name(path))
        } else {
            match v.note(path) {
                Some(n) => n.name.clone(),
                None => return,
            }
        };
        let mut input = Input::with(&shown);
        input.select_all();
        self.reveal(path);
        self.explorer.rename = Some(Rename {
            path: String::from(path),
            folder,
            input,
        });
        self.focus = Focus::Tree;
    }

    /// Finish a rename: move the file, follow it in every tab, and rewrite
    /// the links to it in other notes.
    pub fn commit_rename(&mut self) {
        let Some(r) = self.explorer.rename.take() else {
            return;
        };
        self.focus = Focus::Editor;
        let name = r.input.text.trim();
        if name.is_empty() {
            return;
        }
        let mut to = join(parent(&r.path), name);
        if !r.folder {
            to.push_str(".md");
        }
        if to == r.path {
            return;
        }
        self.move_path(&r.path, &to);
    }

    /// Move or rename `from` to `to` (a note or a folder).
    pub fn move_path(&mut self, from: &str, to: &str) {
        self.save_all();
        let Some(v) = self.vault.as_mut() else {
            return;
        };
        let old_note = v.note(from).map(|n| (n.name.clone(), n.key.clone()));
        let linking: Vec<Backlink> = match &old_note {
            Some(_) => v.backlinks(from),
            None => Vec::new(),
        };
        if let Err(why) = v.rename(from, to) {
            self.say(&why);
            return;
        }
        // Tabs follow what moved.
        let mut prefix = String::from(from);
        prefix.push('/');
        for t in self.tabs.iter_mut() {
            for p in t.back.iter_mut().chain(t.fwd.iter_mut()) {
                if let Some(n) = moved_path(p, from, &prefix, to) {
                    *p = n;
                }
            }
            if let Some(d) = t.doc.as_mut() {
                if let Some(n) = moved_path(&d.path, from, &prefix, to) {
                    d.path = n;
                    if d.path == to {
                        d.title = String::from(strip_md(file_name(to)));
                    }
                }
            }
        }
        for p in self.recent.iter_mut() {
            if let Some(n) = moved_path(p, from, &prefix, to) {
                *p = n;
            }
        }
        if let Some(sel) = self.explorer.selected.clone() {
            if let Some(n) = moved_path(&sel, from, &prefix, to) {
                self.explorer.selected = Some(n);
            }
        }
        // A new name: links to the old one are rewritten to keep working.
        let new_name = String::from(strip_md(file_name(to)));
        if let Some((old_name, old_key)) = old_note {
            if old_name != new_name {
                let mut fixed = 0usize;
                for b in linking {
                    let src = moved_path(&b.path, from, &prefix, to).unwrap_or(b.path);
                    if self.rewrite_links_in(&src, &old_key, &new_name) {
                        fixed += 1;
                    }
                }
                if fixed > 0 {
                    let mut msg = String::from("Updated links in ");
                    msg.push_str(&crate::text::count(fixed, "note", "notes"));
                    self.say(&msg);
                }
            }
        }
        self.explorer.rows_dirty = true;
        self.backlinks = None;
    }

    /// Rewrite `[[old]]` links in one note to `[[new]]`. True if any were.
    fn rewrite_links_in(&mut self, path: &str, old_key: &str, new_name: &str) -> bool {
        let now = self.now;
        // An open note is edited in its tab, so the change can be undone.
        for t in self.tabs.iter_mut() {
            if let Some(d) = t.doc.as_mut().filter(|d| d.path == path) {
                let mut any = false;
                for n in 0..d.buf.line_count() {
                    let line = String::from(d.buf.line(n));
                    if let Some(newer) = rewrite_line(&line, old_key, new_name) {
                        d.replace(Pos::new(n, 0), Pos::new(n, line.len()), &newer, now);
                        any = true;
                    }
                }
                return any;
            }
        }
        let Some(v) = self.vault.as_mut() else {
            return false;
        };
        let Ok(text) = read_note(v, path) else {
            return false;
        };
        let mut out = String::with_capacity(text.len());
        let mut any = false;
        for (i, line) in text.split('\n').enumerate() {
            if i > 0 {
                out.push('\n');
            }
            match rewrite_line(line, old_key, new_name) {
                Some(n) => {
                    out.push_str(&n);
                    any = true;
                }
                None => out.push_str(line),
            }
        }
        if any && write_note(v, path, &out).is_ok() {
            v.index_text(path, &out);
            return true;
        }
        false
    }

    pub fn ask_delete(&mut self, path: &str) {
        let Some(v) = self.vault.as_ref() else {
            return;
        };
        let folder = v.folder_id(path).is_some();
        let shown = if folder {
            String::from(file_name(path))
        } else {
            v.note(path).map(|n| n.name.clone()).unwrap_or_default()
        };
        let mut body = String::from("Are you sure you want to delete \u{201c}");
        body.push_str(&shown);
        body.push('\u{201d}');
        if folder {
            body.push_str(" and everything in it");
        }
        body.push_str("? It will be moved to the vault's .trash folder.");
        self.overlay = Some(Overlay::Confirm(Confirm {
            title: String::from(if folder {
                "Delete folder"
            } else {
                "Delete file"
            }),
            body,
            yes: "Delete",
            path: String::from(path),
        }));
    }

    pub fn delete(&mut self, path: &str) {
        let Some(v) = self.vault.as_mut() else {
            return;
        };
        match v.trash(path) {
            Ok(_) => {
                let mut prefix = String::from(path);
                prefix.push('/');
                let gone = |p: &str| p == path || p.starts_with(prefix.as_str());
                for t in self.tabs.iter_mut() {
                    if t.doc.as_ref().is_some_and(|d| gone(&d.path)) {
                        t.doc = None;
                    }
                    t.back.retain(|p| !gone(p));
                    t.fwd.retain(|p| !gone(p));
                }
                self.recent.retain(|p| !gone(p));
                if self.explorer.selected.as_deref().is_some_and(gone) {
                    self.explorer.selected = None;
                }
                self.explorer.rows_dirty = true;
                let mut msg = String::from("Moved ");
                msg.push_str(strip_md(file_name(path)));
                msg.push_str(" to .trash");
                self.say(&msg);
            }
            Err(why) => self.say(&why),
        }
    }

    // ---- links ---------------------------------------------------------------

    /// Follow a `[[link]]`: open the note it names, or make it when it is
    /// not there yet.
    pub fn follow(&mut self, name: &str, new_tab: bool) {
        let name = name.trim();
        let target = name.split(['#', '|']).next().unwrap_or("").trim();
        if target.is_empty() {
            return;
        }
        let Some(v) = self.vault.as_mut() else {
            return;
        };
        let path = match v.resolve(target) {
            Some(i) => v.notes.get(i).map(|n| n.path.clone()),
            None => {
                let mut p = String::from(target.strip_suffix(".md").unwrap_or(target));
                p.push_str(".md");
                match v.create_at(&p) {
                    Ok(p) => Some(p),
                    Err(why) => {
                        self.say(&why);
                        None
                    }
                }
            }
        };
        if let Some(p) = path {
            self.open(&p, new_tab);
        }
    }

    /// Show the notes with a tag in the search pane.
    pub fn search_tag(&mut self, tag: &str) {
        let mut q = String::from("tag:#");
        q.push_str(tag.trim_start_matches('#'));
        self.search = Input::with(&q);
        self.run_search();
        self.left = LeftPane::Search;
        self.left_open = true;
    }

    pub fn resolved(&self, name: &str) -> bool {
        let target = name.split(['#', '|']).next().unwrap_or("");
        self.vault
            .as_ref()
            .is_some_and(|v| v.resolve(target).is_some())
    }

    /// Recompute `[[` completion for the caret.
    pub fn refresh_completion(&mut self) {
        let query = match self.doc() {
            Some(d) if d.mode != Mode::Reading && self.focus == Focus::Editor => d.link_query(),
            _ => None,
        };
        let Some((_, q)) = query else {
            self.completion = None;
            return;
        };
        if self.completion.as_ref().is_some_and(|c| c.query == q) {
            return;
        }
        let Some(v) = self.vault.as_ref() else {
            return;
        };
        let mut scored: Vec<(i32, usize)> = Vec::new();
        for (i, n) in v.notes.iter().enumerate() {
            if let Some(s) = fuzzy(&q, &n.name) {
                scored.push((s, i));
            }
        }
        top_by_score(&mut scored, 12, |i| {
            v.notes.get(i).map_or("", |n| n.name.as_str())
        });
        let items: Vec<(String, String)> = scored
            .iter()
            .filter_map(|(_, i)| v.notes.get(*i))
            .map(|n| (n.name.clone(), n.path.clone()))
            .collect();
        self.completion = Some(Completion {
            query: q,
            items,
            sel: 0,
        });
    }

    pub fn accept_completion(&mut self, i: usize) {
        let Some(name) = self
            .completion
            .as_ref()
            .and_then(|c| c.items.get(i))
            .map(|(n, _)| n.clone())
        else {
            return;
        };
        let now = self.now;
        if let Some(d) = self.doc_mut() {
            d.accept_link(&name, now);
        }
        self.completion = None;
    }

    // ---- search ----------------------------------------------------------------

    pub fn run_search(&mut self) {
        self.search_scroll = 0.0;
        self.search_collapsed.clear();
        self.job = self
            .vault
            .as_ref()
            .map(|v| Job::new(&self.search.text, v))
            .filter(|j| !j.query.trim().is_empty());
    }

    pub fn open_search(&mut self) {
        self.left = LeftPane::Search;
        self.left_open = true;
        self.focus = Focus::Search;
        if let Some(sel) = self
            .doc()
            .map(|d| d.selected_text())
            .filter(|s| !s.is_empty() && !s.contains('\n'))
        {
            self.search = Input::with(&sel);
            self.run_search();
        }
        self.search.select_all();
    }

    /// Open a search result at its line.
    pub fn open_at(&mut self, path: &str, line: usize, col: usize, len: usize) {
        self.open(path, false);
        if let Some(d) = self.doc_mut() {
            let a = d.buf.clamp(Pos::new(line, col));
            let b = d.buf.clamp(Pos::new(line, col + len));
            d.select_range(a, b);
            d.top = line.saturating_sub(3) + 1;
            d.top_off = 0.0;
        }
        self.focus = Focus::Editor;
    }

    // ---- the side panes --------------------------------------------------------

    pub fn backlinks(&mut self) -> &[Backlink] {
        let path = self.doc().map(|d| d.path.clone()).unwrap_or_default();
        let gen = self.vault.as_ref().map_or(0, |v| v.generation);
        let stale = self.backlinks.as_ref().is_none_or(|c| {
            c.path != path || (c.generation != gen && self.now >= c.at + PANE_SETTLE_MS)
        });
        if stale {
            let mut data = match self.vault.as_ref() {
                Some(v) if !path.is_empty() => v.backlinks(&path),
                _ => Vec::new(),
            };
            // The lines that link, read from the linking notes (the first
            // hundred notes; a pane shows no more than that at once).
            for b in data.iter_mut().take(BACKLINK_CONTEXT) {
                let text = match self.open_text(&b.path) {
                    Some(t) => Some(t),
                    None => self.vault.as_mut().and_then(|v| read_note(v, &b.path).ok()),
                };
                if let Some(t) = text {
                    let lines: Vec<&str> = t.split('\n').collect();
                    b.context = b
                        .lines
                        .iter()
                        .map(|l| String::from(lines.get(*l as usize).copied().unwrap_or("").trim()))
                        .collect();
                }
            }
            self.backlinks = Some(Cached {
                path,
                version: 0,
                generation: gen,
                at: self.now,
                data,
            });
        }
        self.backlinks.as_ref().map_or(&[], |c| c.data.as_slice())
    }

    pub fn outline(&mut self) -> &[crate::md::Heading] {
        let (path, version) = self
            .doc()
            .map(|d| (d.path.clone(), d.buf.version))
            .unwrap_or_default();
        let now = self.now;
        let stale = self.outline.as_ref().is_none_or(|c| {
            c.path != path || (c.version != version && now >= c.at + PANE_SETTLE_MS)
        });
        if stale {
            let data = self.doc().map(Doc::headings).unwrap_or_default();
            self.outline = Some(Cached {
                path,
                version,
                generation: 0,
                at: now,
                data,
            });
        }
        self.outline.as_ref().map_or(&[], |c| c.data.as_slice())
    }

    pub fn outgoing(&mut self) -> &[(String, bool)] {
        let (path, version) = self
            .doc()
            .map(|d| (d.path.clone(), d.buf.version))
            .unwrap_or_default();
        let gen = self.vault.as_ref().map_or(0, |v| v.generation);
        let now = self.now;
        let stale = self.outgoing.as_ref().is_none_or(|c| {
            c.path != path
                || ((c.version != version || c.generation != gen) && now >= c.at + PANE_SETTLE_MS)
        });
        if stale {
            let links = self.doc().map(Doc::outgoing).unwrap_or_default();
            let data: Vec<(String, bool)> = links
                .into_iter()
                .map(|l| {
                    let ok = self.resolved(&l);
                    (l, ok)
                })
                .collect();
            self.outgoing = Some(Cached {
                path,
                version,
                generation: gen,
                at: now,
                data,
            });
        }
        self.outgoing.as_ref().map_or(&[], |c| c.data.as_slice())
    }

    pub fn tags(&mut self) -> &[(String, usize)] {
        let gen = self.vault.as_ref().map_or(0, |v| v.generation);
        let stale = self.tags.as_ref().is_none_or(|(g, _)| *g != gen);
        if stale {
            let data = self.vault.as_ref().map(Vault::tags).unwrap_or_default();
            self.tags = Some((gen, data));
        }
        self.tags.as_ref().map_or(&[], |(_, d)| d.as_slice())
    }

    // ---- list prompts ------------------------------------------------------------

    pub fn open_list(&mut self, kind: ListKind) {
        self.completion = None;
        let mut p = ListPrompt {
            kind,
            input: Input::default(),
            items: Vec::new(),
            sel: 0,
            first: 0,
        };
        self.fill_list(&mut p);
        self.overlay = Some(Overlay::List(p));
    }

    fn fill_list(&self, p: &mut ListPrompt) {
        let q = p.input.text.trim();
        p.items.clear();
        p.sel = 0;
        p.first = 0;
        match &p.kind {
            ListKind::Switcher => {
                let Some(v) = self.vault.as_ref() else {
                    return;
                };
                if q.is_empty() {
                    for r in self.recent.iter().take(LIST_MAX) {
                        if let Some(n) = v.note(r) {
                            p.items.push(note_item(&n.name, &n.path));
                        }
                    }
                    for n in v.notes.iter() {
                        if p.items.len() >= LIST_MAX {
                            break;
                        }
                        if !self.recent.contains(&n.path) {
                            p.items.push(note_item(&n.name, &n.path));
                        }
                    }
                    return;
                }
                let mut scored: Vec<(i32, usize)> = Vec::new();
                for (i, n) in v.notes.iter().enumerate() {
                    if let Some(s) = fuzzy(q, &n.name) {
                        scored.push((s + 8, i));
                    } else if let Some(s) = fuzzy(q, &n.path) {
                        scored.push((s, i));
                    }
                }
                top_by_score(&mut scored, LIST_MAX, |i| {
                    v.notes.get(i).map_or("", |n| n.name.as_str())
                });
                for (_, i) in &scored {
                    if let Some(n) = v.notes.get(*i) {
                        p.items.push(note_item(&n.name, &n.path));
                    }
                }
                let exact = v.notes.iter().any(|n| lower(&n.name) == lower(q));
                if !exact {
                    let mut title = String::from(q);
                    let mut hint = String::from("Shift+Enter to create");
                    if p.items.is_empty() {
                        hint = String::from("Enter to create");
                        title = String::from(q);
                    }
                    p.items.push(Item {
                        title,
                        detail: String::from("No note with this name yet"),
                        hint,
                        pick: Pick::Create(String::from(q)),
                    });
                }
            }
            ListKind::Palette => {
                let mut scored: Vec<(i32, usize)> = Vec::new();
                for (i, cm) in COMMANDS.iter().enumerate() {
                    if let Some(s) = fuzzy(q, cm.title) {
                        scored.push((if q.is_empty() { -(i as i32) } else { s }, i));
                    }
                }
                scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
                for (_, i) in scored {
                    if let Some(cm) = COMMANDS.get(i) {
                        p.items.push(Item {
                            title: String::from(cm.title),
                            detail: String::new(),
                            hint: String::from(cm.keys),
                            pick: Pick::Run(cm.cmd),
                        });
                    }
                }
            }
            ListKind::MoveTo(moving) => {
                let Some(v) = self.vault.as_ref() else {
                    return;
                };
                let mut scored: Vec<(i32, String)> = Vec::new();
                for f in v.folder_paths() {
                    // A folder cannot move into itself or below itself.
                    if f == *moving
                        || f.starts_with(moving.as_str())
                            && f.as_bytes().get(moving.len()) == Some(&b'/')
                    {
                        continue;
                    }
                    let shown = if f.is_empty() {
                        String::from("/")
                    } else {
                        f.clone()
                    };
                    if let Some(s) = fuzzy(q, &shown) {
                        scored.push((s, f));
                    }
                }
                scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
                for (_, f) in scored.into_iter().take(LIST_MAX) {
                    p.items.push(Item {
                        title: if f.is_empty() {
                            String::from("/")
                        } else {
                            f.clone()
                        },
                        detail: String::new(),
                        hint: String::new(),
                        pick: Pick::MoveInto(f),
                    });
                }
            }
        }
    }

    fn list_mut(&mut self) -> Option<&mut ListPrompt> {
        match self.overlay.as_mut() {
            Some(Overlay::List(p)) => Some(p),
            _ => None,
        }
    }

    fn refill_list(&mut self) {
        if let Some(Overlay::List(mut p)) = self.overlay.take() {
            self.fill_list(&mut p);
            self.overlay = Some(Overlay::List(p));
        }
    }

    pub fn choose(&mut self, i: usize, m: Mods) {
        let Some(Overlay::List(p)) = self.overlay.take() else {
            return;
        };
        let Some(item) = p.items.get(i).cloned() else {
            self.overlay = Some(Overlay::List(p));
            return;
        };
        let kind = p.kind.clone();
        match item.pick {
            Pick::Open(path) => {
                if m.shift && kind == ListKind::Switcher && !p.input.text.trim().is_empty() {
                    // Shift+Enter makes a note with what was typed.
                    let q = String::from(p.input.text.trim());
                    self.create_named(&q);
                } else {
                    self.open(&path, m.primary);
                }
            }
            Pick::Create(name) => self.create_named(&name),
            Pick::Run(cmd) => self.run(cmd),
            Pick::MoveInto(folder) => {
                if let ListKind::MoveTo(path) = kind {
                    let to = join(&folder, file_name(&path));
                    self.move_path(&path, &to);
                }
            }
        }
    }

    fn create_named(&mut self, name: &str) {
        let name = name.trim().trim_end_matches(".md");
        if name.is_empty() {
            return;
        }
        let mut p = String::from(name);
        p.push_str(".md");
        let made = self.vault.as_mut().map(|v| v.create_at(&p));
        match made {
            Some(Ok(p)) => {
                self.open(&p, false);
                self.reveal(&p);
            }
            Some(Err(why)) => self.say(&why),
            None => {}
        }
    }

    // ---- commands ------------------------------------------------------------------

    pub fn run(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::QuickSwitcher => {
                if self.vault.is_some() {
                    self.open_list(ListKind::Switcher);
                }
            }
            Cmd::Palette => self.open_list(ListKind::Palette),
            Cmd::SearchVault => self.open_search(),
            Cmd::NewNote => {
                if self.vault.is_some() {
                    let f = self.target_folder();
                    self.new_note_in(&f);
                }
            }
            Cmd::NewFolder => {
                if self.vault.is_some() {
                    let f = self.target_folder();
                    self.new_folder_in(&f);
                }
            }
            Cmd::NewTab => self.new_tab(),
            Cmd::CloseTab => self.close_tab(self.active),
            Cmd::NextTab => {
                let n = self.tabs.len().max(1);
                self.select_tab((self.active + 1) % n);
            }
            Cmd::PrevTab => {
                let n = self.tabs.len().max(1);
                self.select_tab((self.active + n - 1) % n);
            }
            Cmd::Save => {
                if self.save_active() && self.doc().is_some() {
                    self.say("Saved");
                }
            }
            Cmd::ToggleReading => {
                if let Some(d) = self.doc_mut() {
                    d.mode = if d.mode == Mode::Reading {
                        Mode::Live
                    } else {
                        Mode::Reading
                    };
                    d.anchor = None;
                }
                self.completion = None;
            }
            Cmd::ToggleSource => {
                if let Some(d) = self.doc_mut() {
                    d.mode = if d.mode == Mode::Source {
                        Mode::Live
                    } else {
                        Mode::Source
                    };
                }
            }
            Cmd::ToggleLeft => self.left_open = !self.left_open,
            Cmd::ToggleRight => self.right_open = !self.right_open,
            Cmd::ShowFiles => {
                self.left = LeftPane::Files;
                self.left_open = true;
            }
            Cmd::ShowBacklinks => self.show_right(RightPane::Backlinks),
            Cmd::ShowOutgoing => self.show_right(RightPane::Outgoing),
            Cmd::ShowTags => self.show_right(RightPane::Tags),
            Cmd::ShowOutline => self.show_right(RightPane::Outline),
            Cmd::Rename => {
                if let Some(p) = self.doc().map(|d| d.path.clone()) {
                    self.start_rename(&p);
                }
            }
            Cmd::Delete => {
                if let Some(p) = self.doc().map(|d| d.path.clone()) {
                    self.ask_delete(&p);
                }
            }
            Cmd::MoveTo => {
                if let Some(p) = self.doc().map(|d| d.path.clone()) {
                    self.open_list(ListKind::MoveTo(p));
                }
            }
            Cmd::Back => self.navigate(false),
            Cmd::Forward => self.navigate(true),
            Cmd::Bold => self.edit(|d, now| d.toggle_wrap("**", now)),
            Cmd::Italic => self.edit(|d, now| d.toggle_wrap("*", now)),
            Cmd::Strike => self.edit(|d, now| d.toggle_wrap("~~", now)),
            Cmd::Highlight => self.edit(|d, now| d.toggle_wrap("==", now)),
            Cmd::InlineCode => self.edit(|d, now| d.toggle_wrap("`", now)),
            Cmd::ToggleTask => self.edit(|d, now| {
                let (a, b) = d.selection().unwrap_or((d.cur, d.cur));
                for n in a.line..=b.line {
                    d.toggle_task(n, now);
                }
            }),
            Cmd::InsertLink => self.edit(|d, now| {
                let sel = d.selected_text();
                match d.selection() {
                    Some((a, b)) if !sel.contains('\n') => {
                        let mut s = String::from("[[");
                        s.push_str(&sel);
                        s.push_str("]]");
                        d.cur = d.replace(a, b, &s, now);
                        d.anchor = None;
                    }
                    _ => {
                        let at = d.cur;
                        d.replace(at, at, "[[]]", now);
                        d.cur = Pos::new(at.line, at.col + 2);
                        d.anchor = None;
                    }
                }
            }),
            Cmd::Undo => self.edit(|d, _| d.undo()),
            Cmd::Redo => self.edit(|d, _| d.redo()),
            Cmd::SelectAll => {
                if let Some(d) = self.doc_mut() {
                    d.select_all();
                }
            }
            Cmd::Theme => self.dark = !self.dark,
            Cmd::OpenVault => self.host.push(HostRequest::PickVault { create: false }),
            Cmd::CloseVault => self.close_vault(),
            Cmd::Reveal => {
                if let Some(p) = self.doc().map(|d| d.path.clone()) {
                    self.reveal(&p);
                }
            }
            Cmd::CollapseAll => {
                self.explorer.expanded.clear();
                self.explorer.rows_dirty = true;
            }
        }
    }

    fn show_right(&mut self, pane: RightPane) {
        self.right = pane;
        self.right_open = true;
        self.right_scroll = 0.0;
    }

    /// Run an edit on the open note, unless it is in reading view.
    fn edit(&mut self, f: impl FnOnce(&mut Doc, u64)) {
        let now = self.now;
        if let Some(d) = self.doc_mut() {
            if d.mode != Mode::Reading {
                f(d, now);
            }
        }
        self.refresh_completion();
    }

    // ---- keys and text ---------------------------------------------------------------

    /// A key press. True when the app used it (and should redraw).
    pub fn key(&mut self, key: &str, m: Mods) -> bool {
        // Chords that work everywhere.
        if m.primary && !m.alt {
            let k = lower(key);
            let cmd = match (k.as_str(), m.shift) {
                ("o", false) => Some(Cmd::QuickSwitcher),
                ("p", false) => Some(Cmd::Palette),
                ("f", true) => Some(Cmd::SearchVault),
                ("e", true) => Some(Cmd::ShowFiles),
                ("n", false) => Some(Cmd::NewNote),
                ("t", false) => Some(Cmd::NewTab),
                ("w", false) => Some(Cmd::CloseTab),
                ("s", false) => Some(Cmd::Save),
                ("\\", false) => Some(Cmd::ToggleLeft),
                ("\\", true) | ("|", _) => Some(Cmd::ToggleRight),
                _ => None,
            };
            if let Some(cmd) = cmd {
                if self.vault.is_some() || matches!(cmd, Cmd::Palette) {
                    if matches!(self.overlay, Some(Overlay::List(_)))
                        && matches!(cmd, Cmd::QuickSwitcher | Cmd::Palette)
                    {
                        self.overlay = None;
                    }
                    self.run(cmd);
                    return true;
                }
            }
        }
        if m.ctrl && key == "Tab" {
            self.run(if m.shift { Cmd::PrevTab } else { Cmd::NextTab });
            return true;
        }
        if m.primary && m.alt && matches!(key, "ArrowLeft" | "ArrowRight") {
            self.navigate(key == "ArrowRight");
            return true;
        }
        // An overlay has the keyboard.
        if let Some(ov) = self.overlay.as_mut() {
            match ov {
                Overlay::Confirm(cf) => {
                    match key {
                        "Enter" => {
                            let p = cf.path.clone();
                            self.overlay = None;
                            self.delete(&p);
                        }
                        "Escape" => self.overlay = None,
                        _ => {}
                    }
                    return true;
                }
                Overlay::Menu(_) => {
                    if key == "Escape" {
                        self.overlay = None;
                    }
                    return true;
                }
                Overlay::List(_) => return self.list_key(key, m),
            }
        }
        if let Some(r) = self.explorer.rename.as_mut() {
            match key {
                "Enter" => self.commit_rename(),
                "Escape" => {
                    self.explorer.rename = None;
                    self.focus = Focus::Editor;
                }
                _ => {
                    if m.primary && lower(key) == "a" {
                        r.input.select_all();
                    } else {
                        r.input.key(key, m);
                    }
                }
            }
            return true;
        }
        if self.focus == Focus::Search {
            match key {
                "Enter" => self.run_search(),
                "Escape" => self.focus = Focus::Editor,
                "ArrowDown" => self.focus = Focus::Editor,
                _ => {
                    if m.primary && lower(key) == "a" {
                        self.search.select_all();
                    } else if self.search.key(key, m) {
                        self.run_search();
                    }
                }
            }
            return true;
        }
        if self.focus == Focus::Tree {
            return self.tree_key(key, m);
        }
        if key == "F2" {
            self.run(Cmd::Rename);
            return true;
        }
        self.editor_key(key, m)
    }

    fn list_key(&mut self, key: &str, m: Mods) -> bool {
        match key {
            "Escape" => {
                self.overlay = None;
                return true;
            }
            "Enter" => {
                let sel = self.list_mut().map_or(0, |p| p.sel);
                self.choose(sel, m);
                return true;
            }
            "ArrowDown" | "ArrowUp" | "Tab" => {
                if let Some(p) = self.list_mut() {
                    let n = p.items.len();
                    if n > 0 {
                        p.sel = if key == "ArrowUp" || (key == "Tab" && m.shift) {
                            (p.sel + n - 1) % n
                        } else {
                            (p.sel + 1) % n
                        };
                    }
                }
                return true;
            }
            _ => {}
        }
        let changed = match self.list_mut() {
            Some(p) => {
                if m.primary && lower(key) == "a" {
                    p.input.select_all();
                    false
                } else {
                    p.input.key(key, m)
                }
            }
            None => false,
        };
        if changed {
            self.refill_list();
        }
        true
    }

    fn tree_key(&mut self, key: &str, _m: Mods) -> bool {
        let rows: Vec<Row> = self.rows().to_vec();
        let at = self.explorer.selected.clone().and_then(|s| {
            rows.iter()
                .position(|r| self.row_path(r).as_deref() == Some(s.as_str()))
        });
        match key {
            "ArrowDown" | "ArrowUp" => {
                let n = rows.len();
                if n == 0 {
                    return true;
                }
                let i = match at {
                    Some(i) if key == "ArrowDown" => (i + 1).min(n - 1),
                    Some(i) => i.saturating_sub(1),
                    None => 0,
                };
                if let Some(r) = rows.get(i) {
                    self.explorer.selected = self.row_path(r);
                }
            }
            "ArrowRight" | "ArrowLeft" | "Enter" => {
                let Some(r) = at.and_then(|i| rows.get(i)).copied() else {
                    return true;
                };
                let Some(p) = self.row_path(&r) else {
                    return true;
                };
                match r.kind {
                    RowKind::Folder(_) => {
                        let open = self.explorer.expanded.contains(&p);
                        if (key == "ArrowRight" && !open)
                            || (key == "ArrowLeft" && open)
                            || key == "Enter"
                        {
                            self.toggle_folder(&p);
                        }
                    }
                    RowKind::Note(_) => {
                        if key == "Enter" {
                            self.open(&p, false);
                        } else if key == "ArrowLeft" {
                            let up = String::from(parent(&p));
                            if !up.is_empty() {
                                self.explorer.selected = Some(up);
                            }
                        }
                    }
                }
            }
            "F2" => {
                if let Some(p) = self.explorer.selected.clone() {
                    self.start_rename(&p);
                }
            }
            "Delete" | "Backspace" => {
                if let Some(p) = self.explorer.selected.clone() {
                    self.ask_delete(&p);
                }
            }
            "Escape" => self.focus = Focus::Editor,
            _ => return false,
        }
        true
    }

    fn editor_key(&mut self, key: &str, m: Mods) -> bool {
        // `[[` completion has the arrows and Enter while it is up.
        if let Some(c) = self.completion.as_mut() {
            match key {
                "ArrowDown" | "ArrowUp" => {
                    let n = c.items.len().max(1);
                    c.sel = if key == "ArrowUp" {
                        (c.sel + n - 1) % n
                    } else {
                        (c.sel + 1) % n
                    };
                    return true;
                }
                "Enter" | "Tab" if !c.items.is_empty() => {
                    let s = c.sel;
                    self.accept_completion(s);
                    return true;
                }
                "Escape" => {
                    self.completion = None;
                    return true;
                }
                _ => {}
            }
        }
        let now = self.now;
        if m.primary && !m.alt {
            let k = lower(key);
            let cmd = match (k.as_str(), m.shift) {
                ("b", false) => Some(Cmd::Bold),
                ("i", false) => Some(Cmd::Italic),
                ("e", false) => Some(Cmd::ToggleReading),
                ("l", false) => Some(Cmd::ToggleTask),
                ("k", false) => Some(Cmd::InsertLink),
                ("z", false) => Some(Cmd::Undo),
                ("z", true) | ("y", false) => Some(Cmd::Redo),
                ("a", false) => Some(Cmd::SelectAll),
                _ => None,
            };
            if let Some(cmd) = cmd {
                self.run(cmd);
                return true;
            }
            if k == "c" || k == "x" {
                let sel = self.doc().map(Doc::selected_text).unwrap_or_default();
                if !sel.is_empty() {
                    self.host.push(HostRequest::Copy(sel));
                    if k == "x" {
                        self.edit(|d, now| d.backspace(now));
                    }
                }
                return true;
            }
        }
        let reading = self.doc().is_some_and(|d| d.mode == Mode::Reading);
        let Some(d) = self.doc_mut() else {
            return false;
        };
        let shift = m.shift;
        let used = match key {
            "ArrowLeft" if m.primary => {
                let p = Pos::new(d.cur.line, 0);
                d.place(p, shift);
                true
            }
            "ArrowRight" if m.primary => {
                let p = Pos::new(d.cur.line, d.buf.line_len(d.cur.line));
                d.place(p, shift);
                true
            }
            "ArrowLeft" if m.alt => {
                let p = d.word_left_of(d.cur);
                d.place(p, shift);
                true
            }
            "ArrowRight" if m.alt => {
                let p = d.word_right_of(d.cur);
                d.place(p, shift);
                true
            }
            "ArrowUp" if m.primary => {
                d.place(Pos::new(0, 0), shift);
                true
            }
            "ArrowDown" if m.primary => {
                let e = d.buf.end();
                d.place(e, shift);
                true
            }
            "ArrowLeft" => {
                d.left(shift);
                true
            }
            "ArrowRight" => {
                d.right(shift);
                true
            }
            "Home" => {
                let p = Pos::new(d.cur.line, 0);
                d.place(p, shift);
                true
            }
            "End" => {
                let p = Pos::new(d.cur.line, d.buf.line_len(d.cur.line));
                d.place(p, shift);
                true
            }
            "Escape" => {
                d.anchor = None;
                true
            }
            _ if reading => false,
            "Backspace" if m.primary => {
                let to = Pos::new(d.cur.line, 0);
                d.delete_back_to(to, now);
                true
            }
            "Backspace" if m.alt => {
                let to = d.word_left_of(d.cur);
                d.delete_back_to(to, now);
                true
            }
            "Backspace" => {
                d.backspace(now);
                true
            }
            "Delete" => {
                d.delete_forward(now);
                true
            }
            "Enter" => {
                d.newline(now);
                true
            }
            "Tab" => {
                d.indent(shift, now);
                true
            }
            _ => false,
        };
        if used {
            self.refresh_completion();
        }
        used
    }

    /// Typed text (after the keyboard layout and any input method).
    pub fn text(&mut self, s: &str) {
        if let Some(ov) = self.overlay.as_mut() {
            if let Overlay::List(p) = ov {
                p.input.insert(s);
                self.refill_list();
            }
            return;
        }
        if let Some(r) = self.explorer.rename.as_mut() {
            r.input.insert(s);
            return;
        }
        if self.focus == Focus::Search {
            self.search.insert(s);
            self.run_search();
            return;
        }
        if self.focus == Focus::Tree {
            return;
        }
        let now = self.now;
        if let Some(d) = self.doc_mut() {
            if d.mode == Mode::Reading {
                return;
            }
            let clean: String = s
                .chars()
                .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
                .collect();
            let clean = clean.replace("\r\n", "\n");
            if clean.is_empty() {
                return;
            }
            if clean == "\n" {
                d.newline(now);
            } else {
                d.insert(&clean, now);
            }
        }
        self.refresh_completion();
    }

    // ---- clicks --------------------------------------------------------------------------

    pub fn click(&mut self, hit: Hit, m: Mods) {
        // A press anywhere else ends a rename (keeping what was typed).
        if self.explorer.rename.is_some() && !matches!(hit, Hit::TreeRow(_)) {
            self.commit_rename();
        }
        match hit {
            Hit::Welcome(0) => self.host.push(HostRequest::PickVault { create: true }),
            Hit::Welcome(1) => self.host.push(HostRequest::PickVault { create: false }),
            Hit::Welcome(_) => self.host.push(HostRequest::Sample),
            Hit::Ribbon(cmd) | Hit::Command(cmd) => self.run(cmd),
            Hit::LeftTab(p) => {
                self.left = p;
                self.left_open = true;
                if p == LeftPane::Search {
                    self.focus = Focus::Search;
                }
            }
            Hit::RightTab(p) => self.show_right(p),
            Hit::TreeRow(i) => {
                let Some(r) = self.rows().get(i).copied() else {
                    return;
                };
                let Some(p) = self.row_path(&r) else {
                    return;
                };
                if self.explorer.rename.as_ref().is_some_and(|rn| rn.path != p) {
                    self.commit_rename();
                }
                if self.explorer.rename.is_some() {
                    return;
                }
                self.explorer.selected = Some(p.clone());
                match r.kind {
                    RowKind::Folder(_) => {
                        self.toggle_folder(&p);
                        self.focus = Focus::Tree;
                    }
                    RowKind::Note(_) => self.open(&p, m.primary),
                }
            }
            Hit::TreeBlank => {
                self.explorer.selected = None;
                self.focus = Focus::Tree;
            }
            Hit::Tab(i) => self.select_tab(i),
            Hit::TabClose(i) => self.close_tab(i),
            Hit::NewTab => self.new_tab(),
            Hit::Nav(fwd) => self.navigate(fwd),
            Hit::ModeToggle => self.run(Cmd::ToggleReading),
            Hit::SearchBox => self.focus = Focus::Search,
            Hit::SearchFile(i) => {
                if let Some(p) = self
                    .job
                    .as_ref()
                    .and_then(|j| j.results.get(i))
                    .map(|r| r.path.clone())
                {
                    if !self.search_collapsed.remove(&p) {
                        self.search_collapsed.insert(p);
                    }
                }
            }
            Hit::SearchLine(f, l) => {
                let found = self
                    .job
                    .as_ref()
                    .and_then(|j| j.results.get(f))
                    .and_then(|r| {
                        r.lines
                            .get(l)
                            .map(|h| (r.path.clone(), h.line as usize, h.col, h.at.1 - h.at.0))
                    });
                if let Some((p, line, col, len)) = found {
                    self.open_at(&p, line, col, len);
                }
            }
            Hit::Backlink(i, l) => {
                let found = self.backlinks().get(i).map(|b| {
                    (
                        b.path.clone(),
                        b.lines.get(l).copied().unwrap_or(0) as usize,
                    )
                });
                if let Some((p, line)) = found {
                    let key = self.doc().map(|d| lower(&d.title)).unwrap_or_default();
                    self.open(&p, m.primary);
                    if let Some(d) = self.doc_mut() {
                        let text = lower(d.line(line));
                        let col = crate::text::find_ci(&text, &key, 0).unwrap_or(0);
                        let a = d.buf.clamp(Pos::new(line, col));
                        d.place(a, false);
                        d.top = line.saturating_sub(3) + 1;
                        d.top_off = 0.0;
                    }
                }
            }
            Hit::Outgoing(i) => {
                if let Some(name) = self.outgoing().get(i).map(|(n, _)| n.clone()) {
                    self.follow(&name, m.primary);
                }
            }
            Hit::Tag(i) => {
                if let Some(t) = self.tags().get(i).map(|(t, _)| t.clone()) {
                    self.search_tag(&t);
                }
            }
            Hit::Outline(i) => {
                if let Some(line) = self.outline().get(i).map(|h| h.line) {
                    if let Some(d) = self.doc_mut() {
                        d.place(Pos::new(line, d.buf.line_len(line)), false);
                        d.top = line + 1;
                        d.top_off = 0.0;
                    }
                    self.focus = Focus::Editor;
                }
            }
            Hit::ListItem(i) => self.choose(i, m),
            Hit::ListBox => {}
            Hit::MenuItem(i) => {
                let action = match self.overlay.take() {
                    Some(Overlay::Menu(menu)) => menu.items.get(i).map(|(_, a)| a.clone()),
                    other => {
                        self.overlay = other;
                        None
                    }
                };
                if let Some(a) = action {
                    self.menu_action(a);
                }
            }
            Hit::ConfirmYes => {
                if let Some(Overlay::Confirm(cf)) = self.overlay.take() {
                    self.delete(&cf.path);
                }
            }
            Hit::ConfirmNo | Hit::Scrim => self.overlay = None,
            Hit::Completion(i) => self.accept_completion(i),
            Hit::LinkNote(name) => self.follow(&name, m.primary),
        }
    }

    /// A right-click on a tree row (or the empty tree below the rows).
    pub fn context(&mut self, row: Option<usize>, x: f32, y: f32) {
        let r = row.and_then(|i| self.rows().get(i).copied());
        let path = r.and_then(|r| self.row_path(&r));
        let mut items: Vec<(String, MenuAction)> = Vec::new();
        match (r.map(|r| r.kind), path) {
            (Some(RowKind::Folder(_)), Some(p)) => {
                items.push((String::from("New note"), MenuAction::NewNote(p.clone())));
                items.push((String::from("New folder"), MenuAction::NewFolder(p.clone())));
                items.push((
                    String::from("Move folder to..."),
                    MenuAction::MoveTo(p.clone()),
                ));
                items.push((String::from("Rename..."), MenuAction::Rename(p.clone())));
                items.push((String::from("Delete"), MenuAction::Delete(p.clone())));
                self.explorer.selected = Some(p);
            }
            (Some(RowKind::Note(_)), Some(p)) => {
                items.push((
                    String::from("Open in new tab"),
                    MenuAction::OpenInNewTab(p.clone()),
                ));
                items.push((
                    String::from("Move file to..."),
                    MenuAction::MoveTo(p.clone()),
                ));
                items.push((String::from("Rename..."), MenuAction::Rename(p.clone())));
                items.push((String::from("Delete"), MenuAction::Delete(p.clone())));
                self.explorer.selected = Some(p);
            }
            _ => {
                items.push((String::from("New note"), MenuAction::NewNote(String::new())));
                items.push((
                    String::from("New folder"),
                    MenuAction::NewFolder(String::new()),
                ));
            }
        }
        self.overlay = Some(Overlay::Menu(Menu { x, y, items }));
    }

    fn menu_action(&mut self, a: MenuAction) {
        match a {
            MenuAction::NewNote(f) => self.new_note_in(&f),
            MenuAction::NewFolder(f) => self.new_folder_in(&f),
            MenuAction::OpenInNewTab(p) => self.open(&p, true),
            MenuAction::Rename(p) => self.start_rename(&p),
            MenuAction::MoveTo(p) => self.open_list(ListKind::MoveTo(p)),
            MenuAction::Delete(p) => self.ask_delete(&p),
        }
    }

    /// A tree row dragged onto a folder row (or onto the root).
    pub fn drop_on(&mut self, from_row: usize, onto: Option<usize>) {
        let rows: Vec<Row> = self.rows().to_vec();
        let Some(from) = rows.get(from_row).and_then(|r| self.row_path(r)) else {
            return;
        };
        let folder = match onto.and_then(|i| rows.get(i)) {
            Some(r) => {
                let Some(p) = self.row_path(r) else {
                    return;
                };
                match r.kind {
                    RowKind::Folder(_) => p,
                    RowKind::Note(_) => String::from(parent(&p)),
                }
            }
            None => String::new(),
        };
        if folder == parent(&from)
            || folder == from
            || folder.starts_with(&{
                let mut s = from.clone();
                s.push('/');
                s
            })
        {
            return;
        }
        let to = join(&folder, file_name(&from));
        self.move_path(&from, &to);
        if !folder.is_empty() {
            self.explorer.expanded.insert(folder);
            self.explorer.rows_dirty = true;
        }
    }

    // ---- background work -------------------------------------------------------------

    /// Work between frames: index notes, carry the search on, save notes
    /// whose typing has paused. `more` says whether there is time left this
    /// frame. True when something visible changed.
    pub fn tick(&mut self, more: &mut dyn FnMut() -> bool) -> bool {
        let mut changed = false;
        if let Some(st) = &self.status {
            if self.now >= st.1 {
                self.status = None;
                changed = true;
            }
        }
        // Save notes a moment after typing stops.
        let due: Vec<usize> = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| {
                t.doc.as_ref().is_some_and(|d| {
                    d.modified() && d.dirty_since.is_some_and(|s| self.now >= s + SAVE_AFTER_MS)
                })
            })
            .map(|(i, _)| i)
            .collect();
        for i in due {
            self.save_tab(i);
            changed = true;
        }
        // Search first: a person is waiting on it.
        if let (Some(job), Some(v)) = (self.job.as_mut(), self.vault.as_mut()) {
            if !job.done {
                let tabs = &self.tabs;
                let open = |p: &str| {
                    tabs.iter()
                        .filter_map(|t| t.doc.as_ref())
                        .find(|d| d.path == p && d.modified())
                        .map(Doc::text)
                };
                job.step(v, &open, more);
                changed = true;
            }
        }
        if let Some(v) = self.vault.as_mut() {
            if v.index_pending() > 0 && more() && v.index_some(more) {
                changed = true;
            }
        }
        changed
    }

    /// Take in what changed on disk outside the app: the tree is listed
    /// again, changed notes are read again, and a tab showing a changed
    /// note with no edits of its own shows the new text. True if anything
    /// changed.
    pub fn outside_changes(&mut self) -> bool {
        let Some(v) = self.vault.as_mut() else {
            return false;
        };
        let changed = v.fs.outside_changes();
        if changed.is_empty() {
            return false;
        }
        v.scan();
        for p in &changed {
            if v.note(p).is_some() {
                v.queue_index(p);
            }
        }
        let mut reload: Vec<(usize, String)> = Vec::new();
        for (i, t) in self.tabs.iter().enumerate() {
            if let Some(d) = t.doc.as_ref() {
                if !d.modified() && changed.contains(&d.path) {
                    reload.push((i, d.path.clone()));
                }
            }
        }
        for (i, p) in reload {
            let fresh = self.vault.as_mut().and_then(|v| read_note(v, &p).ok());
            if let (Some(text), Some(t)) = (fresh, self.tabs.get_mut(i)) {
                if let Some(d) = t.doc.as_mut() {
                    let mut nd = Doc::new(&d.path, &d.title, &text);
                    nd.mode = d.mode;
                    nd.cur = nd.buf.clamp(d.cur);
                    nd.top = d.top.min(nd.buf.line_count());
                    *d = nd;
                }
            }
        }
        // A note gone from disk closes in its tab.
        let live: Vec<bool> = self
            .tabs
            .iter()
            .map(|t| {
                t.doc.as_ref().is_none_or(|d| {
                    d.modified()
                        || self
                            .vault
                            .as_ref()
                            .is_some_and(|v| v.note(&d.path).is_some())
                })
            })
            .collect();
        for (t, keep) in self.tabs.iter_mut().zip(live) {
            if !keep {
                t.doc = None;
            }
        }
        self.explorer.rows_dirty = true;
        true
    }

    /// Whether there is background work left (so the guest keeps ticking).
    pub fn busy(&self) -> bool {
        self.job.as_ref().is_some_and(|j| !j.done)
            || self.vault.as_ref().is_some_and(|v| v.index_pending() > 0)
    }

    /// When the next timed thing is due (a save, a status message ending),
    /// in ms from now; `None` when nothing is waiting.
    pub fn next_due(&self) -> Option<u64> {
        let mut due: Option<u64> = self.status.as_ref().map(|s| s.1);
        for t in &self.tabs {
            if let Some(s) = t
                .doc
                .as_ref()
                .filter(|d| d.modified())
                .and_then(|d| d.dirty_since)
            {
                let at = s + SAVE_AFTER_MS;
                due = Some(due.map_or(at, |d| d.min(at)));
            }
        }
        due.map(|d| d.saturating_sub(self.now))
    }

    /// The window is closing: save everything first.
    pub fn close(&mut self) {
        self.save_all();
        self.closing = true;
    }

    /// The state, as `key:value` lines, for automated runs.
    pub fn report(&self) -> String {
        let mut out = String::new();
        let mut put = |k: &str, v: &str| {
            out.push_str(k);
            out.push(':');
            out.push_str(v);
            out.push('\n');
        };
        let num = crate::text::num;
        match &self.vault {
            Some(v) => {
                put("vault", &v.name);
                put("notes", &num(v.note_count()));
                put("folders", &num(v.folder_count()));
                put("indexed", &num(v.indexed()));
            }
            None => put("vault", "none"),
        }
        put("tabs", &num(self.tabs.len()));
        if let Some(d) = self.doc() {
            put("note", &d.path);
            put("lines", &num(d.buf.line_count()));
            put("words", &num(d.words));
            put("characters", &num(d.chars));
            put(
                "mode",
                match d.mode {
                    Mode::Live => "live",
                    Mode::Source => "source",
                    Mode::Reading => "reading",
                },
            );
            put("unsaved", if d.modified() { "yes" } else { "no" });
        }
        if let Some(j) = &self.job {
            put("search", &j.query);
            put("search_notes", &num(j.notes_matched));
            put("search_matches", &num(j.matches));
        }
        put("saves", &num(self.saves));
        out
    }
}

fn note_item(name: &str, path: &str) -> Item {
    let folder = parent(path);
    Item {
        title: String::from(name),
        detail: String::from(folder),
        hint: String::new(),
        pick: Pick::Open(String::from(path)),
    }
}

/// Keep the best `k` of `scored` (highest score first, then by name).
fn top_by_score<'a>(scored: &mut Vec<(i32, usize)>, k: usize, name: impl Fn(usize) -> &'a str) {
    if scored.len() > k * 4 {
        // Cut most of the list without sorting it all.
        scored.select_nth_unstable_by(k, |a, b| b.0.cmp(&a.0));
        scored.truncate(k + 1);
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| name(a.1).cmp(name(b.1))));
    scored.truncate(k);
}

fn push_rows(v: &Vault, folder: usize, depth: u16, open: &BTreeSet<String>, out: &mut Vec<Row>) {
    let Some(f) = v.folders.get(folder) else {
        return;
    };
    for &c in &f.folders {
        out.push(Row {
            depth,
            kind: RowKind::Folder(c),
        });
        if v.folders.get(c).is_some_and(|cf| open.contains(&cf.path)) {
            push_rows(v, c, depth + 1, open, out);
        }
    }
    for &n in &f.notes {
        out.push(Row {
            depth,
            kind: RowKind::Note(n),
        });
    }
}

fn strip_md(name: &str) -> &str {
    name.strip_suffix(".md").unwrap_or(name)
}

fn moved_path(p: &str, from: &str, prefix: &str, to: &str) -> Option<String> {
    if p == from {
        return Some(String::from(to));
    }
    p.strip_prefix(prefix).map(|rest| {
        let mut n = String::from(to);
        n.push('/');
        n.push_str(rest);
        n
    })
}

/// `line` with every `[[old...]]` link pointing at `new_name` instead,
/// keeping any `#heading` or `|alias`. `None` when nothing changed.
pub fn rewrite_line(line: &str, old_key: &str, new_name: &str) -> Option<String> {
    if !line.contains("[[") {
        return None;
    }
    let mut out = String::new();
    let mut rest = line;
    let mut any = false;
    while let Some(open) = rest.find("[[") {
        out.push_str(rest.get(..open + 2).unwrap_or(""));
        let after = rest.get(open + 2..).unwrap_or("");
        let Some(close) = after.find("]]") else {
            rest = after;
            break;
        };
        let inner = after.get(..close).unwrap_or("");
        let name_end = inner.find(['#', '|']).unwrap_or(inner.len());
        let name = inner.get(..name_end).unwrap_or("");
        let bare = name.trim().strip_suffix(".md").unwrap_or(name.trim());
        // A path link keeps its folder part only if it named the note's.
        let key_part = bare.rsplit('/').next().unwrap_or(bare);
        if lower(key_part) == old_key && !bare.contains('/') {
            out.push_str(new_name);
            out.push_str(inner.get(name_end..).unwrap_or(""));
            any = true;
        } else {
            out.push_str(inner);
        }
        out.push_str("]]");
        rest = after.get(close + 2..).unwrap_or("");
    }
    out.push_str(rest);
    any.then_some(out)
}

/// Read a note, whole, in large pieces (a 200,000-line note is megabytes).
pub fn read_note(v: &mut Vault, path: &str) -> Result<String, String> {
    v.fs.read(path)
}

pub fn write_note(v: &mut Vault, path: &str, text: &str) -> Result<(), String> {
    v.fs.write(path, text)
}

/// A vault in memory, for tests and the sample.
pub fn mem_vault(name: &str, files: &[(&str, &str)]) -> Vault {
    let mut fs = crate::vfs::MemFs::new();
    for (p, t) in files {
        fs.put(p, t);
    }
    Vault::open(name, Box::new(fs))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let mut a = App::new();
        a.open_vault(mem_vault(
            "Test",
            &[
                ("Welcome.md", "# Welcome\nGo to [[Plan]].\n#start"),
                (
                    "Projects/Plan.md",
                    "# Plan\n## Goals\nShip it.\nSee [[Welcome]]",
                ),
                ("Projects/Notes.md", "Some [[plan|the plan]] text\n#work"),
                (
                    "Journal/2026-11-22.md",
                    "Today I worked on [[Plan#Goals]]. #work",
                ),
            ],
        ));
        a.vault.as_mut().unwrap().index_some(&mut || true);
        a
    }

    const NONE: Mods = Mods {
        shift: false,
        primary: false,
        alt: false,
        ctrl: false,
    };
    const CMD: Mods = Mods {
        shift: false,
        primary: true,
        alt: false,
        ctrl: false,
    };

    #[test]
    fn the_tree_expands_and_opens_notes() {
        let mut a = app();
        let rows: Vec<Row> = a.rows().to_vec();
        // Journal, Projects, then Welcome.
        assert_eq!(rows.len(), 3);
        a.click(Hit::TreeRow(1), NONE);
        assert!(a.explorer.expanded.contains("Projects"));
        assert_eq!(a.rows().len(), 5);
        a.click(Hit::TreeRow(3), NONE);
        assert_eq!(a.doc().unwrap().path, "Projects/Plan.md");
        assert_eq!(a.tabs.len(), 1);
        a.click(Hit::TreeRow(2), CMD);
        assert_eq!(a.tabs.len(), 2);
        assert_eq!(a.doc().unwrap().path, "Projects/Notes.md");
    }

    #[test]
    fn switcher_finds_notes_fuzzily_and_creates_missing_ones() {
        let mut a = app();
        assert!(a.key("o", CMD));
        a.text("pln");
        let first = match &a.overlay {
            Some(Overlay::List(p)) => p.items.first().cloned(),
            _ => None,
        };
        assert_eq!(first.map(|i| i.title), Some(String::from("Plan")));
        a.key("Enter", NONE);
        assert_eq!(a.doc().unwrap().title, "Plan");
        assert!(a.overlay.is_none());
        a.key("o", CMD);
        a.text("Brand new idea");
        a.key(
            "Enter",
            Mods {
                shift: true,
                ..NONE
            },
        );
        assert_eq!(a.doc().unwrap().path, "Brand new idea.md");
        assert!(a
            .vault
            .as_ref()
            .unwrap()
            .note("Brand new idea.md")
            .is_some());
    }

    #[test]
    fn palette_runs_commands() {
        let mut a = app();
        a.open("Welcome.md", false);
        a.key("p", CMD);
        a.text("reading view");
        a.key("Enter", NONE);
        assert_eq!(a.doc().unwrap().mode, Mode::Reading);
        // Typing does nothing in reading view.
        a.text("x");
        assert!(!a.doc().unwrap().modified());
        a.key("e", CMD);
        assert_eq!(a.doc().unwrap().mode, Mode::Live);
    }

    #[test]
    fn typing_saves_after_a_pause_and_reindexes() {
        let mut a = app();
        a.open("Welcome.md", false);
        a.now = 1_000;
        let end = a.doc().unwrap().buf.end();
        a.doc_mut().unwrap().place(end, false);
        a.text(" [[Notes");
        assert!(a.completion.is_some());
        a.key("Enter", NONE);
        assert!(a.doc().unwrap().text().ends_with("[[Notes]]"));
        a.now = 2_000;
        a.tick(&mut || true);
        assert!(
            a.doc().unwrap().modified(),
            "not yet: typing only just stopped"
        );
        a.now = 4_000;
        a.tick(&mut || true);
        assert!(!a.doc().unwrap().modified());
        let saved = a.vault.as_mut().unwrap().fs.read("Welcome.md").unwrap();
        assert!(saved.ends_with("[[Notes]]"));
        let back = a.vault.as_ref().unwrap().backlinks("Projects/Notes.md");
        assert_eq!(back.len(), 1);
    }

    #[test]
    fn backlinks_follow_the_open_note() {
        let mut a = app();
        a.open("Projects/Plan.md", false);
        let from: Vec<String> = a.backlinks().iter().map(|b| b.path.clone()).collect();
        assert_eq!(
            from,
            ["Journal/2026-11-22.md", "Projects/Notes.md", "Welcome.md"]
        );
        a.click(Hit::Backlink(2, 0), NONE);
        assert_eq!(a.doc().unwrap().path, "Welcome.md");
        assert_eq!(a.doc().unwrap().cur.line, 1);
        assert!(a.can_navigate(false));
        a.navigate(false);
        assert_eq!(a.doc().unwrap().path, "Projects/Plan.md");
        a.navigate(true);
        assert_eq!(a.doc().unwrap().path, "Welcome.md");
    }

    #[test]
    fn renaming_rewrites_links_and_follows_tabs() {
        let mut a = app();
        a.open("Welcome.md", false);
        a.open("Projects/Plan.md", true);
        a.start_rename("Projects/Plan.md");
        a.explorer.rename.as_mut().unwrap().input = Input::with("Roadmap");
        a.key("Enter", NONE);
        assert_eq!(a.doc().unwrap().path, "Projects/Roadmap.md");
        assert_eq!(a.doc().unwrap().title, "Roadmap");
        // The open Welcome tab was edited in place; the others on disk.
        let welcome = a.tabs[0].doc.as_ref().unwrap().text();
        assert!(welcome.contains("[[Roadmap]]"), "{welcome}");
        let notes = a
            .vault
            .as_mut()
            .unwrap()
            .fs
            .read("Projects/Notes.md")
            .unwrap();
        assert!(notes.contains("[[Roadmap|the plan]]"), "{notes}");
        let journal = a
            .vault
            .as_mut()
            .unwrap()
            .fs
            .read("Journal/2026-11-22.md")
            .unwrap();
        assert!(journal.contains("[[Roadmap#Goals]]"), "{journal}");
    }

    #[test]
    fn delete_asks_first_and_moves_to_trash() {
        let mut a = app();
        a.open("Projects/Notes.md", false);
        a.run(Cmd::Delete);
        assert!(matches!(a.overlay, Some(Overlay::Confirm(_))));
        a.click(Hit::ConfirmNo, NONE);
        assert!(a
            .vault
            .as_ref()
            .unwrap()
            .note("Projects/Notes.md")
            .is_some());
        a.run(Cmd::Delete);
        a.click(Hit::ConfirmYes, NONE);
        assert!(a
            .vault
            .as_ref()
            .unwrap()
            .note("Projects/Notes.md")
            .is_none());
        assert!(a.doc().is_none());
        assert!(a.vault.as_mut().unwrap().fs.exists(".trash/Notes.md"));
    }

    #[test]
    fn move_and_drag_into_folders() {
        let mut a = app();
        a.open("Welcome.md", false);
        a.run(Cmd::MoveTo);
        a.text("journal");
        a.key("Enter", NONE);
        assert_eq!(a.doc().unwrap().path, "Journal/Welcome.md");
        // Drag the Journal folder into Projects.
        let rows: Vec<Row> = a.rows().to_vec();
        let journal = rows
            .iter()
            .position(|r| a.row_path(r).as_deref() == Some("Journal"))
            .unwrap();
        let projects = rows
            .iter()
            .position(|r| a.row_path(r).as_deref() == Some("Projects"))
            .unwrap();
        a.drop_on(journal, Some(projects));
        assert_eq!(a.doc().unwrap().path, "Projects/Journal/Welcome.md");
    }

    #[test]
    fn new_note_asks_for_its_name() {
        let mut a = app();
        a.key("n", CMD);
        assert_eq!(a.doc().unwrap().path, "Untitled.md");
        assert!(a.explorer.rename.is_some());
        a.text("x");
        a.key("Backspace", NONE);
        a.text("Shopping list");
        a.key("Enter", NONE);
        assert_eq!(a.doc().unwrap().path, "Shopping list.md");
        a.text("milk");
        assert_eq!(a.doc().unwrap().text(), "milk");
    }

    #[test]
    fn search_runs_as_a_job() {
        let mut a = app();
        a.key("f", Mods { shift: true, ..CMD });
        assert_eq!(a.focus, Focus::Search);
        a.text("plan");
        assert!(a.busy());
        a.tick(&mut || true);
        let j = a.job.as_ref().unwrap();
        assert!(j.done);
        assert_eq!(j.notes_matched, 4);
        a.click(Hit::SearchLine(0, 0), NONE);
        assert!(a.doc().is_some());
        a.search_tag("work");
        a.tick(&mut || true);
        assert_eq!(a.job.as_ref().unwrap().notes_matched, 2);
    }

    #[test]
    fn following_a_missing_link_makes_the_note() {
        let mut a = app();
        a.follow("Someday", false);
        assert_eq!(a.doc().unwrap().path, "Someday.md");
        a.follow("Plan#Goals", true);
        assert_eq!(a.doc().unwrap().path, "Projects/Plan.md");
        assert_eq!(a.tabs.len(), 2);
    }

    #[test]
    fn rewrite_line_keeps_aliases_and_headings() {
        assert_eq!(
            rewrite_line("a [[Old]] b [[old|x]] [[Old#h]] [[Other]]", "old", "New").as_deref(),
            Some("a [[New]] b [[New|x]] [[New#h]] [[Other]]")
        );
        assert_eq!(rewrite_line("nothing here", "old", "New"), None);
    }

    #[test]
    fn report_says_what_is_open() {
        let mut a = app();
        a.open("Welcome.md", false);
        let r = a.report();
        assert!(r.contains("notes:4\n"));
        assert!(r.contains("note:Welcome.md\n"));
        assert!(r.contains("words:"));
    }
}
