//! The core plugins on top of the app's state: the graph, canvas and base
//! views as tabs, daily notes, templates, the note composer, file
//! recovery, bookmarks, unlinked mentions, nested tags, all properties, the
//! settings window and the hotkeys. Plain code, tested natively; the guest
//! draws what is here.

use crate::app::{
    App, AskFor, Cmd, Confirm, ConfirmAction, Focus, GraphTab, Input, LeftPane, Menu, MenuAction,
    Mods, Overlay, RecoveryTab, RightPane, SettingsPage, SideTab, Tab, Unlinked, View, COMMANDS,
};
use crate::bookmarks::{self, Bookmark, Kind as BKind};
use crate::buffer::Pos;
use crate::canvas::CardKind;
use crate::dates::{fill_template, When};
use crate::graph::{self, Graph};
use crate::json::Value;
use crate::text::lower;
use crate::vfs::{file_name, join, parent};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

/// What a row of the settings window shows on its right.
#[derive(Debug, Clone, PartialEq)]
pub enum Control {
    Toggle(bool),
    /// A dropdown, showing its current choice.
    Choice(String),
    Text(String),
    Button(&'static str),
    /// A hotkey, as text; whether the person set it; whether another
    /// command has the same one.
    Keys(String, bool, bool),
    Heading,
    None,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SettingRow {
    pub name: String,
    pub desc: String,
    pub control: Control,
}

/// Which settings file a row writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum F {
    App,
    Appearance,
    Daily,
    Templates,
    Recovery,
}

/// What pressing a row does.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Act {
    None,
    Bool(F, &'static str, bool),
    /// Flip a value the app stores the other way round.
    NotBool(F, &'static str, bool),
    Cycle(F, &'static str, &'static [(&'static str, &'static str)]),
    Text(F, &'static str, &'static str),
    Num(F, &'static str, f64),
    Plugin(&'static str),
    ClearRecovery,
    OpenVault,
    /// Record a hotkey for the command at this place in the list.
    Hotkey(usize),
}

fn h(name: &str) -> (SettingRow, Act) {
    (
        SettingRow {
            name: String::from(name),
            desc: String::new(),
            control: Control::Heading,
        },
        Act::None,
    )
}

const VIEW_MODES: &[(&str, &str)] = &[("source", "Editing view"), ("preview", "Reading view")];
const EDIT_MODES: &[(&str, &str)] = &[("true", "Live Preview"), ("false", "Source mode")];
const TRASH: &[(&str, &str)] = &[
    ("system", "Move to system trash"),
    ("local", "Move to Obsidian trash (.trash folder)"),
    ("none", "Permanently delete"),
];
const NEW_FILE: &[(&str, &str)] = &[
    ("root", "Vault folder"),
    ("current", "Same folder as current file"),
    ("folder", "In the folder specified below"),
];
const LINK_FORMAT: &[(&str, &str)] = &[
    ("shortest", "Shortest path when possible"),
    ("relative", "Relative path to file"),
    ("absolute", "Absolute path in vault"),
];
const THEMES: &[(&str, &str)] = &[
    ("obsidian", "Dark"),
    ("moonstone", "Light"),
    ("system", "Adapt to system"),
];
const FONT_SIZES: &[(&str, &str)] = &[
    ("14", "14"),
    ("15", "15"),
    ("16", "16"),
    ("17", "17"),
    ("18", "18"),
    ("20", "20"),
];

impl App {
    // ---- hotkeys -------------------------------------------------------------

    /// The command a key press runs, if any: the person's own hotkey from
    /// `hotkeys.json` first, else the default.
    pub fn hotkey(&self, key: &str, m: Mods) -> Option<Cmd> {
        let k = lower(match key {
            "ArrowLeft" => "left",
            "ArrowRight" => "right",
            "ArrowUp" => "up",
            "ArrowDown" => "down",
            other => other,
        });
        for c in COMMANDS {
            if !c.plugin.is_empty() && !self.settings.plugin_on(c.plugin) {
                continue;
            }
            if let Some(own) = self.settings.hotkeys_for(c.id) {
                // Set in hotkeys.json (empty: the person removed it).
                if own.iter().any(|(p, s, a, key)| {
                    *p == m.primary && *s == m.shift && *a == m.alt && lower(key) == k
                }) {
                    return Some(c.cmd);
                }
                continue;
            }
            if c.keys.is_empty() {
                continue;
            }
            let parts: Vec<&str> = c.keys.split('+').collect();
            let Some((last, mods)) = parts.split_last() else {
                continue;
            };
            let want_ctrl = mods.contains(&"Ctrl");
            let want_cmd = mods.contains(&"Cmd");
            let want_shift = mods.contains(&"Shift");
            let want_alt = mods.contains(&"Alt");
            let primary_ok = if want_ctrl {
                m.ctrl
            } else {
                m.primary == want_cmd
            };
            if primary_ok && m.shift == want_shift && m.alt == want_alt && lower(last) == k {
                return Some(c.cmd);
            }
        }
        None
    }

    /// A command's keys as shown (the person's own first).
    pub fn keys_for(&self, cmd: Cmd) -> String {
        let Some(c) = crate::app::command(cmd) else {
            return String::new();
        };
        if let Some(own) = self.settings.hotkeys_for(c.id) {
            let mut out = String::new();
            if let Some((p, s, a, k)) = own.first() {
                let k = match k.as_str() {
                    "ArrowLeft" => "Left",
                    "ArrowRight" => "Right",
                    "ArrowUp" => "Up",
                    "ArrowDown" => "Down",
                    other => other,
                };
                if *p {
                    out.push_str("Cmd+");
                }
                if *a {
                    out.push_str("Alt+");
                }
                if *s {
                    out.push_str("Shift+");
                }
                out.push_str(k);
            }
            return out;
        }
        String::from(c.keys)
    }

    // ---- settings ------------------------------------------------------------

    pub fn save_settings(&mut self) {
        if let Some(v) = self.vault.as_mut() {
            if let Err(why) = self.settings.save(&mut *v.fs) {
                let mut msg = String::from("Could not save settings: ");
                msg.push_str(&why);
                self.say(&msg);
            }
        }
        self.dark = self.settings.dark(self.system_dark);
    }

    pub fn save_bookmarks(&mut self) {
        let mut data = core::mem::replace(&mut self.settings.bookmarks.data, Value::obj());
        bookmarks::write(&self.bookmarks, &mut data);
        self.settings.bookmarks.data = Value::obj();
        self.settings.bookmarks.set(
            "items",
            data.get("items").cloned().unwrap_or(Value::Arr(Vec::new())),
        );
        self.save_settings();
    }

    fn file(&mut self, f: F) -> &mut crate::settings::File {
        match f {
            F::App => &mut self.settings.app,
            F::Appearance => &mut self.settings.appearance,
            F::Daily => &mut self.settings.daily,
            F::Templates => &mut self.settings.templates,
            F::Recovery => &mut self.settings.recovery,
        }
    }

    fn file_ref(&self, f: F) -> &crate::settings::File {
        match f {
            F::App => &self.settings.app,
            F::Appearance => &self.settings.appearance,
            F::Daily => &self.settings.daily,
            F::Templates => &self.settings.templates,
            F::Recovery => &self.settings.recovery,
        }
    }

    fn spec(&self, page: SettingsPage) -> Vec<(SettingRow, Act)> {
        let row = |name: &str, desc: &str, control: Control, act: Act| {
            (
                SettingRow {
                    name: String::from(name),
                    desc: String::from(desc),
                    control,
                },
                act,
            )
        };
        let b = |f: F, k: &'static str, d: bool| self.file_ref(f).bool_or(k, d);
        let choice = |f: F, k: &'static str, d: &'static str, opts: &[(&str, &str)]| {
            let raw = match self.file_ref(f).data.get(k) {
                Some(Value::Bool(x)) => String::from(if *x { "true" } else { "false" }),
                Some(Value::Num(n)) => {
                    let mut s = String::new();
                    crate::json::push_num(&mut s, *n);
                    s
                }
                Some(Value::Str(s)) => s.clone(),
                _ => String::from(d),
            };
            let label = opts
                .iter()
                .find(|(v, _)| *v == raw)
                .map_or(raw.clone(), |(_, l)| String::from(*l));
            Control::Choice(label)
        };
        let t = |f: F, k: &'static str, d: &'static str| {
            Control::Text(String::from(self.file_ref(f).str_or(k, d)))
        };
        match page {
            SettingsPage::General => alloc::vec![
                h("App"),
                row("Current version", "Vault 0.1, made with Krate. It opens the same vault folder as the notes app, with the same settings.", Control::None, Act::None),
                row("Language", "Change the display language.", Control::Choice(String::from("English")), Act::None),
                row("Help", "Learn how to use the app.", Control::Button("Open"), Act::None),
                h("Vault"),
                row("Open another vault", "Switch to a different folder of notes.", Control::Button("Manage"), Act::OpenVault),
            ],
            SettingsPage::Editor => alloc::vec![
                h("General"),
                row("Default view for new tabs", "The default view that a new Markdown tab gets opened in.", choice(F::App, "defaultViewMode", "source", VIEW_MODES), Act::Cycle(F::App, "defaultViewMode", VIEW_MODES)),
                row("Default editing mode", "The default editing mode a new tab will start with.", choice(F::App, "livePreview", "true", EDIT_MODES), Act::Cycle(F::App, "livePreview", EDIT_MODES)),
                h("Display"),
                row("Readable line length", "Limit maximum line length.", Control::Toggle(b(F::App, "readableLineLength", true)), Act::Bool(F::App, "readableLineLength", true)),
                row("Strict line breaks", "Markdown specs ignore single line breaks in reading view. Turn this off to make single line breaks visible.", Control::Toggle(b(F::App, "strictLineBreaks", false)), Act::Bool(F::App, "strictLineBreaks", false)),
                row("Show line numbers", "Show line numbers in the gutter.", Control::Toggle(b(F::App, "showLineNumber", false)), Act::Bool(F::App, "showLineNumber", false)),
                row("Show indentation guides", "Show vertical relationship lines between list items.", Control::Toggle(b(F::App, "showIndentGuide", true)), Act::Bool(F::App, "showIndentGuide", true)),
                h("Behavior"),
                row("Spellcheck", "Turn on the spell checker.", Control::Toggle(b(F::App, "spellcheck", true)), Act::Bool(F::App, "spellcheck", true)),
                row("Auto-pair brackets", "Pair brackets and quotes automatically.", Control::Toggle(b(F::App, "autoPairBrackets", true)), Act::Bool(F::App, "autoPairBrackets", true)),
                row("Auto-pair Markdown syntax", "Pair symbols automatically for bold, italic, code, and more.", Control::Toggle(b(F::App, "autoPairMarkdown", true)), Act::Bool(F::App, "autoPairMarkdown", true)),
                row("Smart lists", "Automatically format and indent lists.", Control::Toggle(b(F::App, "smartIndentList", true)), Act::Bool(F::App, "smartIndentList", true)),
                row("Indent using tabs", "Use tabs to indent by pressing the \"Tab\" key.", Control::Toggle(b(F::App, "useTab", true)), Act::Bool(F::App, "useTab", true)),
                h("Advanced"),
                row("Fold heading", "Lets you fold all content under a heading.", Control::Toggle(b(F::App, "foldHeading", true)), Act::Bool(F::App, "foldHeading", true)),
                row("Fold indent", "Lets you fold part of an indentation, such as lists.", Control::Toggle(b(F::App, "foldIndent", true)), Act::Bool(F::App, "foldIndent", true)),
            ],
            SettingsPage::Files => {
                let mut v = alloc::vec![
                    row("Confirm file deletion", "Prompt before deleting a file.", Control::Toggle(b(F::App, "promptDelete", true)), Act::Bool(F::App, "promptDelete", true)),
                    row("Deleted files", "What happens to a file after you delete it.", choice(F::App, "trashOption", "system", TRASH), Act::Cycle(F::App, "trashOption", TRASH)),
                    h("Links"),
                    row("Automatically update internal links", "When you rename a file, automatically update internal links to it.", Control::Toggle(b(F::App, "alwaysUpdateLinks", false)), Act::Bool(F::App, "alwaysUpdateLinks", false)),
                    row("Default location for new notes", "Where newly created notes are placed.", choice(F::App, "newFileLocation", "root", NEW_FILE), Act::Cycle(F::App, "newFileLocation", NEW_FILE)),
                ];
                if self.settings.new_file_location() == "folder" {
                    v.push(row("Folder to create new notes in", "Newly created notes will appear under this folder.", t(F::App, "newFileFolderPath", ""), Act::Text(F::App, "newFileFolderPath", "")));
                }
                v.extend([
                    row("New link format", "What links to insert when auto-generating internal links.", choice(F::App, "newLinkFormat", "shortest", LINK_FORMAT), Act::Cycle(F::App, "newLinkFormat", LINK_FORMAT)),
                    row("Use [[Wikilinks]]", "Auto-generate Wikilinks for [[links]] and ![[images]] instead of Markdown links.", Control::Toggle(!b(F::App, "useMarkdownLinks", false)), Act::NotBool(F::App, "useMarkdownLinks", false)),
                    row("Default location for new attachments", "Where newly added attachments are placed.", t(F::App, "attachmentFolderPath", "/"), Act::Text(F::App, "attachmentFolderPath", "/")),
                    h("Advanced"),
                    row("Detect all file extensions", "Show files with any extension even if they are not supported.", Control::Toggle(b(F::App, "showUnsupportedFiles", false)), Act::Bool(F::App, "showUnsupportedFiles", false)),
                ]);
                v
            }
            SettingsPage::Appearance => alloc::vec![
                row("Base color scheme", "Choose the app's default color scheme.", choice(F::Appearance, "theme", "obsidian", THEMES), Act::Cycle(F::Appearance, "theme", THEMES)),
                row("Accent color", "Choose the accent color used throughout the app.", Control::Choice(String::from("Default")), Act::None),
                h("Font"),
                row("Font size", "Font size in pixels that affects reading view and editing view.", choice(F::Appearance, "baseFontSize", "16", FONT_SIZES), Act::Cycle(F::Appearance, "baseFontSize", FONT_SIZES)),
                h("Interface"),
                row("Show inline title", "Display the file name as an editable title inline with the file contents.", Control::Toggle(b(F::Appearance, "showInlineTitle", true)), Act::Bool(F::Appearance, "showInlineTitle", true)),
                row("Show tab title bar", "Display the header at the top of every tab.", Control::Toggle(b(F::Appearance, "showViewHeader", true)), Act::Bool(F::Appearance, "showViewHeader", true)),
                row("Show ribbon", "Display vertical toolbar on the side of the window.", Control::Toggle(b(F::Appearance, "showRibbon", false)), Act::Bool(F::Appearance, "showRibbon", false)),
            ],
            SettingsPage::Hotkeys => COMMANDS
                .iter()
                .enumerate()
                .filter(|(_, c)| c.plugin.is_empty() || self.settings.plugin_on(c.plugin))
                .map(|(ci, c)| {
                    let keys = self.keys_for(c.cmd);
                    let recording = self.recording == Some(ci);
                    let clash = if keys.is_empty() {
                        Vec::new()
                    } else {
                        self.conflicts(ci)
                    };
                    let desc = if recording {
                        String::from("Press a hotkey... Esc to cancel, Backspace to remove it")
                    } else if !clash.is_empty() {
                        let mut d = String::from("This hotkey conflicts with ");
                        d.push_str(&clash.join(", "));
                        d
                    } else {
                        String::new()
                    };
                    (
                        SettingRow {
                            name: String::from(c.title),
                            desc,
                            control: Control::Keys(
                                if recording {
                                    String::from("Press hotkey...")
                                } else if keys.is_empty() {
                                    String::from("Blank")
                                } else {
                                    keys
                                },
                                self.settings.hotkeys.data.get(c.id).is_some(),
                                !clash.is_empty(),
                            ),
                        },
                        Act::Hotkey(ci),
                    )
                })
                .collect(),
            SettingsPage::CorePlugins => crate::settings::CORE_PLUGINS
                .iter()
                .map(|(id, name, _)| {
                    let desc = match *id {
                        "sync" => "Sync is a paid service of the other app; Vault has no sync.",
                        "file-explorer" => "See all files in your vault.",
                        "global-search" => "Search for keywords in your vault.",
                        "switcher" => "Jump to files via keyboard.",
                        "graph" => "Shows a graph representation of the connections between notes.",
                        "backlink" => "Show links to the active note.",
                        "canvas" => "An infinite canvas for your notes, media, and links.",
                        "outgoing-link" => "Show links from the active note.",
                        "tag-pane" => "Show all tags and their counts.",
                        "properties" => "Show all properties in your vault.",
                        "page-preview" => "Preview internal links on hover.",
                        "daily-notes" => "Create and open daily notes.",
                        "templates" => "Insert snippets of text from template files into notes.",
                        "note-composer" => "Merge notes, or split a note into new notes.",
                        "command-palette" => "Run commands via keyboard.",
                        "bookmarks" => "Bookmark files, folders, searches, and headings.",
                        "outline" => "Show the headings of the current note.",
                        "word-count" => "Show the word and character count in the status bar.",
                        "file-recovery" => "Recover lost work with automatic snapshots.",
                        "bases" => "Create database-like views of notes.",
                        _ => "",
                    };
                    row(name, desc, Control::Toggle(self.settings.plugin_on(id)), Act::Plugin(id))
                })
                .collect(),
            SettingsPage::DailyNotes => alloc::vec![
                row("Date format", "Your current syntax looks like this: the moment.js format of the note's name.", t(F::Daily, "format", "YYYY-MM-DD"), Act::Text(F::Daily, "format", "YYYY-MM-DD")),
                row("New file location", "New daily notes will be placed here.", t(F::Daily, "folder", ""), Act::Text(F::Daily, "folder", "")),
                row("Template file location", "Choose the file to use as a template.", t(F::Daily, "template", ""), Act::Text(F::Daily, "template", "")),
                row("Open daily note on startup", "Open your daily note automatically whenever you open this vault.", Control::Toggle(b(F::Daily, "autorun", false)), Act::Bool(F::Daily, "autorun", false)),
            ],
            SettingsPage::Templates => alloc::vec![
                row("Template folder location", "Files in this folder will be available as templates.", t(F::Templates, "folder", ""), Act::Text(F::Templates, "folder", "")),
                row("Date format", "{{date}} in the template file will be replaced with this value.", t(F::Templates, "dateFormat", "YYYY-MM-DD"), Act::Text(F::Templates, "dateFormat", "YYYY-MM-DD")),
                row("Time format", "{{time}} in the template file will be replaced with this value.", t(F::Templates, "timeFormat", "HH:mm"), Act::Text(F::Templates, "timeFormat", "HH:mm")),
            ],
            SettingsPage::FileRecovery => {
                let mut n = String::new();
                crate::json::push_num(&mut n, self.settings.recovery.num_or("intervalMinutes", 5.0));
                let mut d = String::new();
                crate::json::push_num(&mut d, self.settings.recovery.num_or("keepDays", 7.0));
                alloc::vec![
                    row("Snapshot interval", "Minimum number of minutes between two snapshots.", Control::Text(n), Act::Num(F::Recovery, "intervalMinutes", 5.0)),
                    row("History length", "Number of days to keep snapshots.", Control::Text(d), Act::Num(F::Recovery, "keepDays", 7.0)),
                    row("Clear history", "Delete all snapshots.", Control::Button("Clear"), Act::ClearRecovery),
                ]
            }
        }
    }

    /// The rows of the settings page that is open.
    pub fn settings_rows(&self) -> Vec<SettingRow> {
        match &self.overlay {
            Some(Overlay::Settings(p)) => self.spec(*p).into_iter().map(|(r, _)| r).collect(),
            _ => Vec::new(),
        }
    }

    pub fn setting_click(&mut self, i: usize) {
        let Some(Overlay::Settings(page)) = self.overlay.as_ref().cloned_page() else {
            return;
        };
        if let Some((row, _)) = self.setting_edit.take() {
            if row == i {
                return;
            }
            self.commit_setting(page, row);
        }
        let Some((_, act)) = self.spec(page).into_iter().nth(i) else {
            return;
        };
        match act {
            Act::None => {}
            Act::Bool(f, k, d) => {
                let on = !self.file_ref(f).bool_or(k, d);
                self.file(f).set(k, Value::Bool(on));
            }
            Act::NotBool(f, k, d) => {
                let on = !self.file_ref(f).bool_or(k, d);
                self.file(f).set(k, Value::Bool(on));
            }
            Act::Cycle(f, k, opts) => {
                let cur = match self.file_ref(f).data.get(k) {
                    Some(Value::Bool(x)) => String::from(if *x { "true" } else { "false" }),
                    Some(Value::Num(n)) => {
                        let mut s = String::new();
                        crate::json::push_num(&mut s, *n);
                        s
                    }
                    Some(Value::Str(s)) => s.clone(),
                    _ => String::from(opts.first().map_or("", |o| o.0)),
                };
                let at = opts.iter().position(|(v, _)| *v == cur).unwrap_or(0);
                let (next, _) = opts[(at + 1) % opts.len()];
                let v = match next {
                    "true" => Value::Bool(true),
                    "false" => Value::Bool(false),
                    n if n.bytes().all(|c| c.is_ascii_digit()) => {
                        Value::Num(n.parse().unwrap_or(16.0))
                    }
                    s => Value::s(s),
                };
                self.file(f).set(k, v);
            }
            Act::Text(f, k, d) => {
                let cur = String::from(self.file_ref(f).str_or(k, d));
                let mut inp = Input::with(&cur);
                inp.select_all();
                self.setting_edit = Some((i, inp));
                return;
            }
            Act::Num(f, k, d) => {
                let mut cur = String::new();
                crate::json::push_num(&mut cur, self.file_ref(f).num_or(k, d));
                let mut inp = Input::with(&cur);
                inp.select_all();
                self.setting_edit = Some((i, inp));
                return;
            }
            Act::Plugin(id) => {
                if id == "sync" {
                    self.say("Vault has no sync: keep the vault in a synced folder instead");
                } else {
                    let on = !self.settings.plugin_on(id);
                    self.settings.set_plugin(id, on);
                }
            }
            Act::Hotkey(ci) => {
                self.recording = if self.recording == Some(ci) {
                    None
                } else {
                    Some(ci)
                };
                return;
            }
            Act::ClearRecovery => {
                let paths: Vec<String> = self.recovery.files.keys().cloned().collect();
                self.recovery.files.clear();
                self.recovery.dirty = paths;
                self.say("Snapshots cleared");
            }
            Act::OpenVault => {
                self.overlay = None;
                self.run(Cmd::OpenVault);
                return;
            }
        }
        self.save_settings();
    }

    fn commit_setting(&mut self, page: SettingsPage, row: usize) {
        let Some((r, inp)) = self.setting_edit.take().or(Some((row, Input::default()))) else {
            return;
        };
        let _ = r;
        let Some((_, act)) = self.spec(page).into_iter().nth(row) else {
            return;
        };
        match act {
            Act::Text(f, k, _) => {
                let v = String::from(inp.text.trim());
                self.file(f).set(k, Value::Str(v));
            }
            Act::Num(f, k, d) => {
                let n = inp.text.trim().parse::<f64>().unwrap_or(d).max(1.0);
                self.file(f).set(k, Value::Num(n));
            }
            _ => {}
        }
        self.save_settings();
    }

    pub(crate) fn settings_key(&mut self, key: &str, m: Mods) -> bool {
        let page = match &self.overlay {
            Some(Overlay::Settings(p)) => *p,
            _ => return false,
        };
        if let Some(ci) = self.recording {
            match key {
                "Shift" | "Meta" | "Control" | "Alt" | "Super" => {}
                "Escape" => self.recording = None,
                "Backspace" | "Delete" if !m.primary && !m.alt && !m.shift => {
                    self.set_hotkey(ci, None);
                    self.recording = None;
                }
                _ => {
                    self.set_hotkey(ci, Some((m, key)));
                    self.recording = None;
                }
            }
            return true;
        }
        if let Some((row, inp)) = self.setting_edit.as_mut() {
            match key {
                "Enter" => {
                    let (row, inp) = (*row, inp.clone());
                    self.setting_edit = Some((row, inp));
                    self.commit_setting(page, row);
                }
                "Escape" => self.setting_edit = None,
                _ => {
                    if m.primary && lower(key) == "a" {
                        inp.select_all();
                    } else {
                        inp.key(key, m);
                    }
                }
            }
            return true;
        }
        if key == "Escape" {
            self.overlay = None;
            self.save_settings();
        }
        true
    }

    /// "Restore default" on a settings row.
    pub fn setting_reset(&mut self, row: usize) {
        let page = match &self.overlay {
            Some(Overlay::Settings(p)) => *p,
            _ => return,
        };
        if let Some((_, Act::Hotkey(ci))) = self.spec(page).into_iter().nth(row) {
            self.reset_hotkey(ci);
        }
    }

    /// Typed text while the settings window is open.
    pub(crate) fn settings_text(&mut self, s: &str) {
        if let Some(ci) = self.recording.take() {
            let k = String::from(s.trim());
            if !k.is_empty() {
                self.set_hotkey(ci, Some((Mods::default(), k.as_str())));
            }
            return;
        }
        if let Some((_, inp)) = self.setting_edit.as_mut() {
            inp.insert(s);
        }
    }

    /// Bind command `ci` to a key (`None`: no key at all), in hotkeys.json
    /// as the notes app writes it.
    pub fn set_hotkey(&mut self, ci: usize, to: Option<(Mods, &str)>) {
        let Some(c) = COMMANDS.get(ci) else {
            return;
        };
        let list = match to {
            None => Value::Arr(Vec::new()),
            Some((m, key)) => {
                let mut mods = Vec::new();
                if m.primary && !(m.ctrl && cfg!(target_os = "macos")) {
                    mods.push(Value::s("Mod"));
                }
                if m.ctrl && m.primary && cfg!(target_os = "macos") {
                    mods.push(Value::s("Ctrl"));
                }
                if m.alt {
                    mods.push(Value::s("Alt"));
                }
                if m.shift {
                    mods.push(Value::s("Shift"));
                }
                let k = if key.chars().count() == 1 {
                    key.to_uppercase()
                } else {
                    String::from(key)
                };
                let mut h = Value::obj();
                h.set("modifiers", Value::Arr(mods));
                h.set("key", Value::Str(k));
                Value::Arr(alloc::vec![h])
            }
        };
        self.settings.hotkeys.set(c.id, list);
        self.save_settings();
    }

    /// Put a command's hotkey back to the default.
    pub fn reset_hotkey(&mut self, ci: usize) {
        if let Some(c) = COMMANDS.get(ci) {
            if self.settings.hotkeys.data.get(c.id).is_some() {
                self.settings.hotkeys.data.remove(c.id);
                self.settings.hotkeys.dirty = true;
                self.save_settings();
            }
        }
    }

    /// The other commands bound to the same keys as command `ci`.
    pub fn conflicts(&self, ci: usize) -> Vec<String> {
        let Some(me) = COMMANDS.get(ci) else {
            return Vec::new();
        };
        let mine = lower(&self.keys_for(me.cmd));
        if mine.is_empty() {
            return Vec::new();
        }
        COMMANDS
            .iter()
            .enumerate()
            .filter(|(i, c)| {
                *i != ci
                    && (c.plugin.is_empty() || self.settings.plugin_on(c.plugin))
                    && lower(&self.keys_for(c.cmd)) == mine
            })
            .map(|(_, c)| String::from(c.title))
            .collect()
    }

    // ---- a line of text asked for -----------------------------------------------

    pub(crate) fn ask_key(&mut self, key: &str, m: Mods) -> bool {
        match key {
            "Enter" => self.ask_ok(),
            "Escape" => self.overlay = None,
            _ => {
                if let Some(Overlay::Ask(a)) = self.overlay.as_mut() {
                    if m.primary && lower(key) == "a" {
                        a.input.select_all();
                    } else {
                        a.input.key(key, m);
                    }
                }
            }
        }
        true
    }

    pub(crate) fn ask_ok(&mut self) {
        let Some(Overlay::Ask(a)) = self.overlay.take() else {
            return;
        };
        let text = String::from(a.input.text.trim());
        if text.is_empty()
            && !matches!(
                a.purpose,
                AskFor::GraphSearch | AskFor::BaseCell(..) | AskFor::BaseRename(_)
            )
        {
            return;
        }
        match a.purpose {
            AskFor::BookmarkGroup => {
                let mut b = Bookmark::new(BKind::Group { items: Vec::new() }, self.now);
                b.title = Some(text);
                self.bookmarks.push(b);
                self.save_bookmarks();
                self.left = LeftPane::Bookmarks;
                self.left_open = true;
            }
            AskFor::CanvasLink => self.canvas_add(CardKind::Link, &text),
            AskFor::CanvasGroup => self.canvas_add(CardKind::Group, &text),
            AskFor::BaseRename(p) => {
                self.base_apply(|d, _| d.set_display_name(&p, &text));
            }
            AskFor::BaseFilterValue(g, path) => self.base_filter_value(g, path, &text),
            AskFor::BaseFilterFormula(g, path) => self.base_filter_formula(g, path, &text),
            AskFor::BaseCell(path, prop) => {
                self.set_property(&path, &prop, &text);
            }
            AskFor::GraphSearch | AskFor::GraphGroup => {
                if let Some(View::Graph(g)) = self.view_mut() {
                    if a.purpose == AskFor::GraphSearch {
                        g.opt.search = text;
                    } else {
                        const COLORS: [u32; 6] =
                            [0xE93147, 0xE0AC00, 0x08B94E, 0x00BFBC, 0x086DDD, 0x7852EE];
                        let c = COLORS[g.opt.groups.len() % COLORS.len()];
                        g.opt.groups.push((text, c));
                        g.opt.open_groups = true;
                    }
                    g.stale = true;
                }
                self.save_graph_options();
            }
        }
    }

    // ---- new files ------------------------------------------------------------

    pub fn new_file(&mut self, base: &str, ext: &str, text: &str) {
        let f = self.target_folder();
        self.new_file_in(&f, base, ext, text);
    }

    pub fn new_file_in(&mut self, folder: &str, base: &str, ext: &str, text: &str) {
        let made = match self.vault.as_mut() {
            Some(v) => v.create_file(folder, base, ext, text),
            None => return,
        };
        match made {
            Ok(path) => {
                self.reveal(&path);
                self.open_file(&path, false);
            }
            Err(why) => self.say(&why),
        }
    }

    // ---- the graph ----------------------------------------------------------------

    /// Open the graph of the vault (in a tab), or a note's local graph (to
    /// the right, linked to the note's tab).
    pub fn open_graph(&mut self, local: Option<String>) {
        if self.vault.is_none() {
            return;
        }
        let opt = if local.is_some() {
            // A local graph shows the links to and from its note, not the
            // links among its neighbours, until asked.
            graph::Options {
                neighbors: false,
                ..graph::Options::default()
            }
        } else {
            graph::Options::from_json(&self.settings.graph.data)
        };
        let view = View::Graph(Box::new(GraphTab {
            local: local.clone(),
            depth: 1,
            graph: None,
            opt,
            cam: (0.0, 0.0),
            zoom: 1.0,
            fitted: false,
            controls: true,
            hover: None,
            stale: true,
            animate: None,
        }));
        if local.is_some() {
            self.open_linked_beside(view);
            return;
        }
        let reuse = self
            .tab()
            .is_some_and(|t| t.doc.is_none() && t.view.is_none());
        if !reuse {
            self.new_tab();
        }
        if let Some(t) = self.tab_mut() {
            t.view = Some(view);
        }
    }

    /// Open a view to the right of the active tab, linked to it so it
    /// follows the active tab's note.
    pub fn open_linked_beside(&mut self, view: View) {
        let i = self.active;
        self.link_tab(i);
        let group = self.tabs.get(i).and_then(|t| t.group);
        let me = self.pane;
        let other = self.window_panes(me).into_iter().find(|p| *p != me);
        match other {
            Some(p) => self.focus_pane(p),
            None => {
                let new = self.panes.len();
                self.panes.push(crate::app::Pane {
                    tabs: Vec::new(),
                    active: 0,
                    stacked: false,
                });
                self.layout_with_mut(me).split(me, new, false);
                self.focus_pane(new);
            }
        }
        let mut t = Tab::empty();
        t.view = Some(view);
        t.group = group;
        if self.tabs.len() == 1 && self.tabs[0].path().is_none() && self.tabs[0].view.is_none() {
            self.tabs.clear();
        }
        self.tabs.push(t);
        self.active = self.tabs.len() - 1;
    }

    /// "Open backlinks / outline / outgoing links for the current note": a
    /// tab beside the note, linked to it.
    pub fn open_side_view(&mut self, pane: RightPane) {
        let Some(path) = self.doc().map(|d| d.path.clone()) else {
            self.say("Open a note first");
            return;
        };
        let title = side_title(pane, &path);
        self.open_linked_beside(View::Side(Box::new(SideTab {
            pane,
            note: Some(path),
            title,
        })));
    }

    /// Link the active tab with tab `ti` of group `pi`.
    pub fn link_with(&mut self, pi: usize, ti: usize) {
        let i = self.active;
        let group = match self.tabs.get(i).and_then(|t| t.group).or_else(|| {
            self.panes
                .get(pi)
                .and_then(|p| p.tabs.get(ti))
                .and_then(|t| t.group)
        }) {
            Some(g) => g,
            None => {
                let g = self.next_group;
                self.next_group += 1;
                g
            }
        };
        if let Some(t) = self.tabs.get_mut(i) {
            t.group = Some(group);
        }
        if let Some(t) = self.panes.get_mut(pi).and_then(|p| p.tabs.get_mut(ti)) {
            t.group = Some(group);
        }
        if let Some(p) = self.active_path() {
            self.follow_linked_pub(&p);
        }
    }

    /// The note the side panes describe: a linked side tab's note, or the
    /// open note.
    pub fn subject_path(&self) -> Option<String> {
        if let Some(o) = &self.side_override {
            return o.clone();
        }
        match self.view() {
            Some(View::Side(sd)) => sd.note.clone(),
            _ => self.doc().map(|d| d.path.clone()),
        }
    }

    /// The subject's path and a version that changes when its text does.
    pub fn subject_version(&self) -> (String, u64) {
        let p = self.subject_path().unwrap_or_default();
        match self.doc().filter(|d| d.path == p) {
            Some(d) => (p, d.buf.version),
            None => (p, self.vault.as_ref().map_or(0, |v| v.generation)),
        }
    }

    /// Run `f` on the subject note, open or read from the vault.
    pub fn with_subject<T>(&mut self, f: impl Fn(&crate::editor::Doc) -> T) -> Option<T> {
        let p = self.subject_path()?;
        if let Some(d) = self.doc().filter(|d| d.path == p) {
            return Some(f(d));
        }
        let text = self.text_of(&p)?;
        let title = String::from(crate::app::strip_ext(file_name(&p)));
        Some(f(&crate::editor::Doc::new(&p, &title, &text)))
    }

    /// Move the heading under the cursor, with everything under it, out
    /// into a new note named after it, leaving a link.
    pub fn extract_heading(&mut self) {
        let Some((path, start, end, title)) = self.doc().and_then(|d| {
            let hs = d.headings();
            let at = hs.iter().rposition(|h| h.line <= d.cur.line)?;
            let h = hs.get(at)?;
            let end = hs
                .iter()
                .skip(at + 1)
                .find(|x| x.level <= h.level)
                .map_or(d.buf.line_count(), |x| x.line);
            Some((d.path.clone(), h.line, end, h.text.clone()))
        }) else {
            self.say("Put the cursor in a section with a heading first");
            return;
        };
        let body: String = {
            let Some(d) = self.doc() else {
                return;
            };
            let lines: Vec<&str> = (start + 1..end).map(|n| d.line(n)).collect();
            let mut b = lines.join("\n");
            while b.starts_with('\n') {
                b.remove(0);
            }
            String::from(b.trim_end())
        };
        let folder = self.target_folder();
        let name: String = title
            .chars()
            .filter(|c| !"\\/:*?\"<>|#^[]".contains(*c))
            .collect();
        let made = self
            .vault
            .as_mut()
            .map(|v| v.create_file(&folder, name.trim(), ".md", &body));
        let made = match made {
            Some(Ok(p)) => p,
            Some(Err(why)) => {
                self.say(&why);
                return;
            }
            None => return,
        };
        let new_name = String::from(crate::app::strip_ext(file_name(&made)));
        let now = self.now;
        if let Some(d) = self.doc_mut().filter(|d| d.path == path) {
            let a = Pos::new(start, 0);
            let b = if end >= d.buf.line_count() {
                d.buf.end()
            } else {
                Pos::new(end, 0)
            };
            let mut link = String::from("![[");
            link.push_str(&new_name);
            link.push_str("]]");
            if end < d.buf.line_count() {
                link.push('\n');
            }
            d.cur = d.replace(a, b, &link, now);
            d.anchor = None;
        }
        self.explorer_dirty();
        let mut msg = String::from("Extracted to ");
        msg.push_str(&new_name);
        self.say(&msg);
    }

    /// Copy a file the person picked into the vault, where the attachment
    /// setting says, and link it at the cursor.
    pub fn attach(&mut self, name: &str, bytes: &[u8]) {
        let Some(note) = self.doc().map(|d| d.path.clone()) else {
            return;
        };
        let setting = String::from(self.settings.app.str_or("attachmentFolderPath", "/"));
        let folder = attachment_folder(&setting, &note);
        let made = self.vault.as_mut().map(|v| -> Result<String, String> {
            if !folder.is_empty() && !v.fs.exists(&folder) {
                v.fs.mkdir(&folder)?;
            }
            let (base, ext) = match name.rfind('.') {
                Some(i) if i > 0 => (&name[..i], &name[i..]),
                _ => (name, ""),
            };
            let path = v.free_name(&folder, base, ext);
            v.fs.write_bytes(&path, bytes)?;
            v.scan();
            Ok(path)
        });
        match made {
            Some(Ok(path)) => {
                let mut link = String::from("![[");
                link.push_str(file_name(&path));
                link.push_str("]]");
                let now = self.now;
                if let Some(d) = self.doc_mut() {
                    d.insert(&link, now);
                }
                self.explorer_dirty();
            }
            Some(Err(why)) => {
                let mut m = String::from("Could not add the attachment: ");
                m.push_str(&why);
                self.say(&m);
            }
            None => {}
        }
    }

    /// A base cell pressed: ask for its new value (the name column opens
    /// the note instead, formulas and file fields are not edited).
    /// A base's "New": an Untitled note, opened in a new tab; from a board
    /// column it starts with that column's value of the property the
    /// board groups by, as the notes app makes it.
    pub fn base_new(&mut self, group: usize) {
        let Some(View::Base(b)) = self.view() else {
            return;
        };
        let prop = b
            .base
            .views
            .get(b.view)
            .and_then(|w| w.group_by.as_ref())
            .map(|(p, _)| String::from(p.strip_prefix("note.").unwrap_or(p)));
        let value = b.groups.get(group).map(|(l, _)| l.clone());
        let text = match (prop, value) {
            (Some(p), Some(v)) if !v.is_empty() && !p.starts_with("file.") => {
                set_property_text("", &p, &v)
            }
            _ => String::new(),
        };
        let made = match self.vault.as_mut() {
            Some(v) => v.create_file("", "Untitled", ".md", &text),
            None => return,
        };
        match made {
            Ok(path) => {
                if let Some(View::Base(b)) = self.view_mut() {
                    b.generation = 0;
                }
                self.reveal(&path);
                self.open_file(&path, true);
            }
            Err(why) => self.say(&why),
        }
    }

    /// A board card dropped on another column: the note's grouping
    /// property takes that column's value (none clears it).
    pub fn base_move_card(&mut self, row: usize, group: usize) {
        let Some(View::Base(b)) = self.view() else {
            return;
        };
        let Some(path) = b.rows.get(row).cloned() else {
            return;
        };
        let Some((prop, _)) = b.base.views.get(b.view).and_then(|w| w.group_by.clone()) else {
            return;
        };
        let Some((value, _)) = b.groups.get(group).cloned() else {
            return;
        };
        if b.groups
            .iter()
            .any(|(l, list)| *l == value && list.contains(&path))
        {
            return;
        }
        let key = String::from(prop.strip_prefix("note.").unwrap_or(&prop));
        if key.starts_with("file.") || key.starts_with("formula.") {
            self.say("A board grouped by a file's own property cannot move cards");
            return;
        }
        self.set_property(&path, &key, &value);
    }

    pub fn edit_base_cell(&mut self, row: usize, col: usize) {
        let Some(View::Base(b)) = self.view() else {
            return;
        };
        let Some(path) = b.rows.get(row).cloned() else {
            return;
        };
        let Some(prop) = b
            .base
            .views
            .get(b.view)
            .and_then(|w| w.order.get(col))
            .cloned()
        else {
            return;
        };
        if prop.starts_with("file.") || prop.starts_with("formula.") {
            if prop == "file.name" {
                self.open_file(&path, false);
            }
            return;
        }
        let key = String::from(prop.strip_prefix("note.").unwrap_or(&prop));
        let current = self
            .vault
            .as_ref()
            .and_then(|v| v.meta.get(&path))
            .and_then(|m| m.props.as_ref())
            .and_then(|p| p.get(&key))
            .map(crate::bases::show)
            .unwrap_or_default();
        let mut input = Input::with(&current);
        input.select_all();
        let mut title = String::from(b.base.column_name(&prop).as_str());
        title.push_str(" of ");
        title.push_str(crate::app::strip_ext(file_name(&path)));
        self.overlay = Some(Overlay::Ask(crate::app::Ask {
            title,
            input,
            purpose: AskFor::BaseCell(path, key),
        }));
    }

    /// Set a note's property, in its `---` block (made if missing), as YAML.
    pub fn set_property(&mut self, path: &str, key: &str, value: &str) {
        let Some(text) = self.text_of(path) else {
            return;
        };
        let out = set_property_text(&text, key, value);
        self.set_text(path, &out);
        self.save_all();
        if let Some(v) = self.vault.as_mut() {
            v.index_text(path, &out);
        }
        if let Some(View::Base(b)) = self.view_mut() {
            b.generation = 0;
        }
    }

    /// Save a graph's controls to graph.json (the global graph's only).
    pub fn save_graph_options(&mut self) {
        let opt = match self.view() {
            Some(View::Graph(g)) if g.local.is_none() => g.opt.clone(),
            _ => return,
        };
        let mut data = self.settings.graph.data.clone();
        opt.to_json(&mut data);
        if data != self.settings.graph.data {
            self.settings.graph.data = data;
            self.settings.graph.dirty = true;
            self.save_settings();
        }
    }

    /// Work for the views between frames: the graph's layout, a base's
    /// rows, a canvas saved after a pause, unlinked mentions, snapshots.
    pub(crate) fn tick_views(&mut self, more: &mut dyn FnMut() -> bool) -> bool {
        let mut changed = false;
        let now = self.now;
        let gen = self.vault.as_ref().map_or(0, |v| v.generation);
        let indexing = self.vault.as_ref().is_some_and(|v| v.index_pending() > 0);
        let active = self.active;
        let Some(v) = self.vault.as_ref() else {
            return false;
        };
        let today = self.local.format("YYYY-MM-DD");
        let mut save_canvas = false;
        let mut fetch: Vec<String> = Vec::new();
        for (i, t) in self
            .tabs
            .iter_mut()
            .enumerate()
            .map(|(i, t)| (Some(i), t))
            .chain(
                self.panes
                    .iter_mut()
                    .filter_map(|p| {
                        let a = p.active;
                        p.tabs.get_mut(a)
                    })
                    .map(|t| (None, t)),
            )
        {
            let showing = i.is_none_or(|i| i == active);
            match t.view.as_mut() {
                Some(View::Graph(g)) if showing => {
                    // Rebuilt when the vault changed (not while every note is
                    // still being read: that would restart the layout each
                    // frame), or when filters changed.
                    let rebuild = g.stale
                        || g.graph.is_none()
                        || (!indexing && g.graph.as_ref().is_some_and(|x| x.generation != gen));
                    if rebuild {
                        let local = g.local.clone();
                        let mut fresh =
                            Graph::build(v, &g.opt, local.as_deref().map(|p| (p, g.depth)));
                        // Keep where dots already were.
                        if let Some(old) = g.graph.as_ref() {
                            let pos: BTreeMap<&str, (f32, f32)> = old
                                .nodes
                                .iter()
                                .filter_map(|n| Some((n.path.as_deref()?, (n.x, n.y))))
                                .collect();
                            let mut kept = 0;
                            for n in fresh.nodes.iter_mut() {
                                if let Some(&(x, y)) = n.path.as_deref().and_then(|p| pos.get(p)) {
                                    n.x = x;
                                    n.y = y;
                                    kept += 1;
                                }
                            }
                            if kept * 2 > fresh.nodes.len() {
                                fresh.alpha = 0.3;
                            }
                        }
                        g.graph = Some(fresh);
                        g.stale = false;
                        changed = true;
                    }
                    if let Some(gr) = g.graph.as_mut() {
                        let mut steps = 0;
                        while gr.running() && steps < 30 && more() {
                            gr.tick(&g.opt);
                            steps += 1;
                        }
                        if steps > 0 {
                            changed = true;
                        }
                    }
                }
                Some(View::Base(b)) if showing && b.generation != gen => {
                    b.base.today = today.clone();
                    b.groups = b.base.groups(v, b.view);
                    // The search box keeps the rows that say what is typed.
                    let q = crate::text::lower(b.search.trim());
                    if !q.is_empty() {
                        for g in &mut b.groups {
                            g.1.retain(|p| {
                                crate::text::lower(p).contains(&q)
                                    || v.meta.get(p).and_then(|m| m.props.as_ref()).is_some_and(
                                        |pr| {
                                            crate::text::lower(&crate::bases::show(pr)).contains(&q)
                                        },
                                    )
                            });
                        }
                        b.groups.retain(|g| !g.1.is_empty());
                    }
                    b.rows = b.groups.iter().flat_map(|g| g.1.iter().cloned()).collect();
                    b.generation = gen;
                    changed = true;
                }
                Some(View::Canvas(c)) => {
                    if c.dirty_since
                        .is_some_and(|s| now >= s + crate::app::SAVE_AFTER_MS)
                        && i.is_some()
                    {
                        save_canvas = true;
                    }
                    if showing {
                        for n in &c.canvas.nodes {
                            if let Some(u) = n.url.as_ref() {
                                if !self.previews.contains_key(u) {
                                    fetch.push(u.clone());
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        for u in fetch {
            if !self.previews.contains_key(&u) {
                self.previews.insert(u.clone(), None);
                self.host.push(crate::app::HostRequest::Fetch(u));
            }
        }
        if save_canvas {
            for i in 0..self.tabs.len() {
                if matches!(&self.tabs[i].view, Some(View::Canvas(c)) if c.dirty_since.is_some()) {
                    let _ = self.save_tab_at(i);
                }
            }
            changed = true;
        }
        if self.step_unlinked(more) {
            changed = true;
        }
        changed
    }

    // ---- canvas ----------------------------------------------------------------------

    fn canvas_mut(&mut self) -> Option<&mut crate::app::CanvasTab> {
        match self.view_mut() {
            Some(View::Canvas(c)) => Some(c),
            _ => None,
        }
    }

    /// Note a change to the canvas (for undo and the save after a pause).
    pub fn canvas_changed(&mut self, before: String) {
        let now = self.now;
        if let Some(c) = self.canvas_mut() {
            c.undo.push(before);
            if c.undo.len() > 100 {
                c.undo.remove(0);
            }
            c.redo.clear();
            c.dirty_since = Some(now);
        }
    }

    /// Add a card in the middle of the view.
    pub fn canvas_add(&mut self, kind: CardKind, body: &str) {
        self.seed = self.seed.wrapping_add(self.now | 1);
        let seed = self.seed;
        let Some(c) = self.canvas_mut() else {
            return;
        };
        let before = c.canvas.to_text();
        let (w, hh) = match kind {
            CardKind::Text => (250.0, 60.0),
            CardKind::File => (400.0, 400.0),
            CardKind::Link => (400.0, 400.0),
            CardKind::Group => (500.0, 400.0),
        };
        let (x, y) = (c.cam.0 - w * 0.5, c.cam.1 - hh * 0.5);
        let id = c.canvas.add(kind, x, y, w, hh, body, seed);
        c.selected = Some(id.clone());
        self.canvas_changed(before);
        if kind == CardKind::Text {
            self.canvas_live_start(&id);
        }
    }

    pub(crate) fn canvas_add_note(&mut self, path: &str) {
        self.canvas_add(CardKind::File, path);
    }

    pub(crate) fn canvas_color(&mut self, id: &str, color: Option<&'static str>) {
        let Some(c) = self.canvas_mut() else {
            return;
        };
        let before = c.canvas.to_text();
        if let Some(n) = c.canvas.card_mut(id) {
            n.color = color.map(String::from);
        }
        self.canvas_changed(before);
    }

    pub(crate) fn canvas_remove(&mut self, id: &str) {
        let Some(c) = self.canvas_mut() else {
            return;
        };
        let before = c.canvas.to_text();
        c.canvas.remove(id);
        if c.selected.as_deref() == Some(id) {
            c.selected = None;
        }
        self.canvas_changed(before);
    }

    /// Start editing a card in place, with the note editor: a text card's
    /// Markdown, or a note card's note itself.
    pub fn canvas_live_start(&mut self, id: &str) {
        let Some(c) = self.canvas_mut() else {
            return;
        };
        let Some(n) = c.canvas.card(id).cloned() else {
            return;
        };
        let doc = match n.kind {
            CardKind::Text => {
                let mut path = String::from(CARD_DOC);
                path.push_str(id);
                Some(crate::editor::Doc::new(
                    &path,
                    "",
                    n.text.as_deref().unwrap_or(""),
                ))
            }
            CardKind::File => n.file.as_deref().and_then(|f| self.load(f)),
            _ => None,
        };
        let Some(mut doc) = doc else {
            return;
        };
        doc.mode = crate::editor::Mode::Live;
        // The caret at the end, as a new card starts.
        let last = doc.buf.line_count().saturating_sub(1);
        doc.cur = crate::buffer::Pos::new(last, doc.buf.line(last).len());
        if let Some(c) = self.canvas_mut() {
            c.selected = Some(String::from(id));
            c.editing = Some((String::from(id), Input::default()));
        }
        if let Some(t) = self.tab_mut() {
            t.doc = Some(doc);
        }
        self.focus = Focus::Editor;
    }

    /// Finish editing a card: a text card keeps what was typed, a note
    /// card's note is saved.
    pub fn canvas_commit(&mut self) {
        let live = self.tab_mut().and_then(|t| t.doc.take());
        if let Some(doc) = live {
            if let Some(c) = self.canvas_mut() {
                c.editing = None;
            }
            match doc.path.strip_prefix(CARD_DOC) {
                Some(id) => {
                    let id = String::from(id);
                    let text = doc.text();
                    let Some(c) = self.canvas_mut() else {
                        return;
                    };
                    let before = c.canvas.to_text();
                    let changed = match c.canvas.card_mut(&id) {
                        Some(n) if n.text.as_deref() != Some(text.as_str()) => {
                            n.text = Some(text);
                            true
                        }
                        _ => false,
                    };
                    if changed {
                        self.canvas_changed(before);
                    }
                }
                None => {
                    if doc.modified() {
                        let text = doc.text();
                        if let Some(v) = self.vault.as_mut() {
                            match crate::app::write_note(v, &doc.path, &text) {
                                Ok(()) => v.index_text(&doc.path, &text),
                                Err(why) => self.say(&why),
                            }
                        }
                    }
                }
            }
            return;
        }
        let Some(c) = self.canvas_mut() else {
            return;
        };
        let Some((id, inp)) = c.editing.take() else {
            return;
        };
        let before = c.canvas.to_text();
        let changed = match c.canvas.card_mut(&id) {
            Some(n) if n.text.as_deref() != Some(inp.text.as_str()) => {
                n.text = Some(inp.text);
                true
            }
            _ => false,
        };
        if changed {
            self.canvas_changed(before);
        }
    }

    pub(crate) fn canvas_key(&mut self, key: &str, m: Mods) -> bool {
        let editing = self.canvas_mut().is_some_and(|c| c.editing.is_some());
        if editing {
            if key == "Escape" {
                self.canvas_commit();
                return true;
            }
            if let Some((_, inp)) = self.canvas_mut().and_then(|c| c.editing.as_mut()) {
                if key == "Enter" {
                    inp.cut_selection();
                    inp.text.insert(inp.caret, '\n');
                    inp.caret += 1;
                } else if m.primary && lower(key) == "a" {
                    inp.select_all();
                } else {
                    inp.key(key, m);
                }
            }
            return true;
        }
        let k = lower(key);
        if m.primary && (k == "z" || k == "y") {
            let redo = m.shift || k == "y";
            let now = self.now;
            if let Some(c) = self.canvas_mut() {
                let (from, to) = if redo {
                    (&mut c.redo, &mut c.undo)
                } else {
                    (&mut c.undo, &mut c.redo)
                };
                if let Some(text) = from.pop() {
                    to.push(c.canvas.to_text());
                    if let Some(cv) = crate::canvas::Canvas::parse(&text) {
                        c.canvas = cv;
                    }
                    c.dirty_since = Some(now);
                    c.selected = None;
                }
            }
            return true;
        }
        match key {
            "Delete" | "Backspace" => {
                let sel = self.canvas_mut().and_then(|c| c.selected.clone());
                if let Some(id) = sel {
                    self.canvas_remove(&id);
                }
            }
            "Escape" => {
                if let Some(c) = self.canvas_mut() {
                    c.selected = None;
                }
            }
            "Enter" => {
                // Enter on a selected text card starts typing in it.
                let start = self.canvas_mut().and_then(|c| {
                    let id = c.selected.clone()?;
                    let n = c.canvas.card(&id)?;
                    (n.kind == CardKind::Text).then(|| (id, n.text.clone().unwrap_or_default()))
                });
                if let Some((id, text)) = start {
                    if let Some(c) = self.canvas_mut() {
                        c.editing = Some((id, Input::with(&text)));
                    }
                }
            }
            _ => return false,
        }
        true
    }

    /// Typed text while a canvas card is being edited.
    pub(crate) fn canvas_text(&mut self, s: &str) -> bool {
        // A card edited with the note editor takes text as a note does.
        if self.doc().is_some() {
            return false;
        }
        match self.canvas_mut().and_then(|c| c.editing.as_mut()) {
            Some((_, inp)) => {
                inp.cut_selection();
                let clean: String = s
                    .chars()
                    .filter(|c| !c.is_control() || *c == '\n')
                    .collect();
                inp.text.insert_str(inp.caret, &clean);
                inp.caret += clean.len();
                true
            }
            None => false,
        }
    }

    /// A right-click on a canvas card.
    pub fn card_menu(&mut self, id: &str, x: f32, y: f32) {
        let file = match self.view() {
            Some(View::Canvas(c)) => c.canvas.card(id).and_then(|n| n.file.clone()),
            _ => None,
        };
        let mut items: Vec<(String, MenuAction)> = Vec::new();
        if let Some(f) = file {
            items.push((String::from("Open in new tab"), MenuAction::CardOpen(f)));
        }
        for (label, c) in [
            ("No color", None),
            ("Red", Some("1")),
            ("Orange", Some("2")),
            ("Yellow", Some("3")),
            ("Green", Some("4")),
            ("Cyan", Some("5")),
            ("Purple", Some("6")),
        ] {
            items.push((
                String::from(label),
                MenuAction::CardColor(String::from(id), c),
            ));
        }
        items.push((
            String::from("Remove"),
            MenuAction::CardRemove(String::from(id)),
        ));
        self.overlay = Some(Overlay::Menu(Menu { x, y, items }));
    }

    // ---- bases ----------------------------------------------------------------------

    // ---- daily notes ----------------------------------------------------------------

    pub fn now_local(&self) -> When {
        self.local
    }

    /// The path of the daily note for a date.
    pub fn daily_path(&self, d: &When) -> String {
        let mut name = d.format(self.settings.daily_format());
        name.push_str(".md");
        join(self.settings.daily_folder().trim_matches('/'), &name)
    }

    /// Which date a daily note is for, by its path (looking a few years
    /// either way of today).
    fn daily_date(&self, path: &str) -> Option<When> {
        let today = self.local;
        for back in 0..2000i64 {
            for d in [today.plus_days(-back), today.plus_days(back)] {
                if self.daily_path(&d) == path {
                    return Some(d);
                }
            }
        }
        None
    }

    /// Open today's daily note (`step` 0), or the one before or after the
    /// open one that is there (`-1`, `1`).
    /// Random note: a note other than the one showing, picked at random.
    pub fn open_random_note(&mut self) {
        let here = self.active_path();
        let notes: Vec<String> = match self.vault.as_ref() {
            Some(v) => v
                .notes
                .iter()
                .filter(|n| n.path.ends_with(".md") && Some(&n.path) != here.as_ref())
                .map(|n| n.path.clone())
                .collect(),
            None => return,
        };
        if notes.is_empty() {
            return;
        }
        self.seed = self
            .seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(self.now | 1);
        let pick = (self.seed >> 33) as usize % notes.len();
        self.open_file(&notes[pick], false);
    }

    /// Unique note creator: a new note named by the time, as
    /// `.obsidian/zk-prefixer.json` says (`format`, `folder`, `template`;
    /// the notes app's default name is YYYYMMDDHHmm).
    pub fn new_unique_note(&mut self) {
        let cfg = self
            .vault
            .as_mut()
            .and_then(|v| v.fs.read(".obsidian/zk-prefixer.json").ok())
            .and_then(|t| crate::json::parse(&t))
            .unwrap_or_else(Value::obj);
        let format = cfg
            .get_str("format")
            .filter(|f| !f.is_empty())
            .unwrap_or("YYYYMMDDHHmm");
        let name = self.local.format(format);
        let folder = String::from(cfg.get_str("folder").unwrap_or("").trim_matches('/'));
        let template = cfg
            .get_str("template")
            .filter(|t| !t.is_empty())
            .map(|t| {
                let mut p = String::from(t);
                if !p.ends_with(".md") {
                    p.push_str(".md");
                }
                p
            })
            .and_then(|p| self.vault.as_mut().and_then(|v| v.fs.read(&p).ok()))
            .unwrap_or_default();
        self.new_file_in(&folder, &name, ".md", &template);
    }

    pub fn open_daily(&mut self, step: i64) {
        if self.vault.is_none() {
            return;
        }
        if step == 0 {
            let today = self.local;
            let path = self.daily_path(&today);
            self.open_or_make_daily(&path);
            return;
        }
        let from = self
            .active_path()
            .and_then(|p| self.daily_date(&p))
            .unwrap_or(self.local);
        for k in 1..3660i64 {
            let d = from.plus_days(k * step);
            let p = self.daily_path(&d);
            if self.vault.as_ref().is_some_and(|v| v.note(&p).is_some()) {
                self.open_file(&p, false);
                return;
            }
        }
        self.say(if step < 0 {
            "No previous daily note"
        } else {
            "No next daily note"
        });
    }

    fn open_or_make_daily(&mut self, path: &str) {
        if self.vault.as_ref().is_some_and(|v| v.note(path).is_some()) {
            self.open_file(path, false);
            return;
        }
        let tpl = String::from(self.settings.daily_template().trim_matches('/'));
        let mut text = String::new();
        if !tpl.is_empty() {
            let mut p = tpl.clone();
            if !p.ends_with(".md") {
                p.push_str(".md");
            }
            match self.vault.as_mut().map(|v| v.fs.read(&p)) {
                Some(Ok(t)) => {
                    let title = crate::app::strip_ext(file_name(path));
                    text = fill_template(
                        &t,
                        title,
                        &self.local,
                        self.settings.daily_format(),
                        self.settings.template_time_format(),
                    );
                }
                _ => self.say("Could not read the daily note template"),
            }
        }
        let made = self.vault.as_mut().map(|v| {
            let folder = String::from(parent(path));
            let base = String::from(crate::app::strip_ext(file_name(path)));
            v.create_file(&folder, &base, ".md", &text)
        });
        match made {
            Some(Ok(p)) => {
                self.open_file(&p, false);
                self.reveal(&p);
            }
            Some(Err(why)) => self.say(&why),
            None => {}
        }
    }

    // ---- templates -------------------------------------------------------------------

    pub(crate) fn insert_template(&mut self, path: &str) {
        let Some(text) = self.vault.as_mut().and_then(|v| v.fs.read(path).ok()) else {
            self.say("Could not read the template");
            return;
        };
        let title = self.doc().map(|d| d.title.clone()).unwrap_or_default();
        let filled = fill_template(
            &text,
            &title,
            &self.local,
            self.settings.template_date_format(),
            self.settings.template_time_format(),
        );
        let now = self.now;
        if let Some(d) = self.doc_mut() {
            d.insert(&filled, now);
        }
        self.focus = Focus::Editor;
    }

    // ---- note composer -----------------------------------------------------------------

    fn text_of(&mut self, path: &str) -> Option<String> {
        match self.open_text(path) {
            Some(t) => Some(t),
            None => self.vault.as_mut().and_then(|v| v.fs.read(path).ok()),
        }
    }

    /// Put `text` in a note, in its tab if it is open (undoable).
    fn set_text(&mut self, path: &str, text: &str) {
        let now = self.now;
        let mut in_tab = false;
        for t in self
            .tabs
            .iter_mut()
            .chain(self.panes.iter_mut().flat_map(|p| p.tabs.iter_mut()))
        {
            if let Some(d) = t.doc.as_mut().filter(|d| d.path == path) {
                let end = d.buf.end();
                d.replace(Pos::new(0, 0), end, text, now);
                in_tab = true;
            }
        }
        if !in_tab {
            if let Some(v) = self.vault.as_mut() {
                if crate::app::write_note(v, path, text).is_ok() {
                    v.index_text(path, text);
                }
            }
        }
    }

    /// Merge the note at `from` into `into`: its text goes at the end of
    /// `into`, links to it are pointed at `into`, and it is deleted.
    pub(crate) fn merge_into(&mut self, from: &str, into: &str) {
        self.save_all();
        let (Some(a), Some(b)) = (self.text_of(from), self.text_of(into)) else {
            return;
        };
        let mut merged = b.trim_end_matches('\n').to_string_owned();
        if !merged.is_empty() {
            merged.push_str("\n\n");
        }
        merged.push_str(&a);
        self.set_text(into, &merged);
        let into_name = String::from(crate::app::strip_ext(file_name(into)));
        let (old_key, linking) = match self.vault.as_ref() {
            Some(v) => (
                v.note(from).map(|n| n.key.clone()).unwrap_or_default(),
                v.backlinks(from),
            ),
            None => return,
        };
        for bl in linking {
            if bl.path != into {
                self.rewrite_links_in(&bl.path, &old_key, &into_name);
            }
        }
        if let Some(v) = self.vault.as_mut() {
            let _ = v.trash(from);
        }
        for t in self
            .tabs
            .iter_mut()
            .chain(self.panes.iter_mut().flat_map(|p| p.tabs.iter_mut()))
        {
            if t.doc.as_ref().is_some_and(|d| d.path == from) {
                t.doc = None;
            }
        }
        self.explorer_dirty();
        self.open_file(into, false);
        let mut msg = String::from("Merged into ");
        msg.push_str(&into_name);
        self.say(&msg);
    }

    /// Move the selection out of `from` into `target` (a new note named
    /// so, or the end of an existing one), leaving a link in its place.
    pub(crate) fn extract_to(&mut self, from: &str, target: &str, create: bool) {
        let sel = self
            .doc()
            .map(crate::editor::Doc::selected_text)
            .unwrap_or_default();
        if sel.is_empty() || self.doc().is_none_or(|d| d.path != from) {
            return;
        }
        let path = if create {
            let folder = self.target_folder();
            let made = self
                .vault
                .as_mut()
                .map(|v| v.create_file(&folder, target.trim(), ".md", &sel));
            match made {
                Some(Ok(p)) => p,
                Some(Err(why)) => {
                    self.say(&why);
                    return;
                }
                None => return,
            }
        } else {
            let Some(old) = self.text_of(target) else {
                return;
            };
            let mut t = old.trim_end_matches('\n').to_string_owned();
            if !t.is_empty() {
                t.push_str("\n\n");
            }
            t.push_str(&sel);
            self.set_text(target, &t);
            String::from(target)
        };
        let name = String::from(crate::app::strip_ext(file_name(&path)));
        let mut link = String::from("[[");
        link.push_str(&name);
        link.push_str("]]");
        let now = self.now;
        if let Some(d) = self.doc_mut() {
            if let Some((a, b)) = d.selection() {
                d.cur = d.replace(a, b, &link, now);
                d.anchor = None;
            }
        }
        self.explorer_dirty();
    }

    // ---- file recovery ------------------------------------------------------------------

    pub(crate) fn open_recovery(&mut self) {
        let Some(path) = self.doc().map(|d| d.path.clone()) else {
            self.say("Open a note to see its saved copies");
            return;
        };
        let n = self.recovery.snapshots(&path).len();
        if n == 0 {
            self.say("No saved copies of this note yet");
        }
        self.new_tab();
        if let Some(t) = self.tab_mut() {
            t.view = Some(View::Recovery(Box::new(RecoveryTab {
                path,
                pick: n.saturating_sub(1),
            })));
        }
    }

    pub(crate) fn ask_restore(&mut self) {
        let (path, pick) = match self.view() {
            Some(View::Recovery(r)) => (r.path.clone(), r.pick),
            _ => return,
        };
        if self.recovery.snapshots(&path).get(pick).is_none() {
            return;
        }
        self.overlay = Some(Overlay::Confirm(Confirm {
            title: String::from("Restore this copy?"),
            body: String::from("The note's text will be replaced with this saved copy. Its text now is kept as a copy too."),
            yes: "Restore",
            path,
            action: ConfirmAction::Restore(pick),
        }));
    }

    pub(crate) fn confirm(&mut self, path: &str, action: ConfirmAction) {
        match action {
            ConfirmAction::Delete => self.delete(path),
            ConfirmAction::Restore(i) => {
                let Some(text) = self.recovery.snapshots(path).get(i).map(|s| s.text.clone())
                else {
                    return;
                };
                if let Some(cur) = self.text_of(path) {
                    let now = self.now;
                    self.recovery.note(path, &cur, now, 0);
                }
                self.set_text(path, &text);
                self.save_all();
                self.open_file(path, false);
                self.say("Restored");
            }
        }
    }

    // ---- bookmarks ---------------------------------------------------------------------

    pub(crate) fn bookmark_path(&mut self, path: &str) {
        if bookmarks::has_file(&self.bookmarks, path) {
            self.say("Already bookmarked");
            return;
        }
        let is_folder = self
            .vault
            .as_ref()
            .is_some_and(|v| v.folder_id(path).is_some());
        let kind = if is_folder {
            BKind::Folder {
                path: String::from(path),
            }
        } else {
            BKind::File {
                path: String::from(path),
            }
        };
        self.bookmarks.push(Bookmark::new(kind, self.now));
        self.save_bookmarks();
    }

    pub(crate) fn bookmark_active(&mut self) {
        if let Some(p) = self.active_path() {
            self.bookmark_path(&p);
        }
    }

    pub(crate) fn bookmark_heading(&mut self) {
        let Some((path, heading)) = self.doc().and_then(|d| {
            let hs = d.headings();
            let h = hs.iter().rev().find(|h| h.line <= d.cur.line)?;
            Some((d.path.clone(), h.text.clone()))
        }) else {
            self.say("Put the cursor under a heading first");
            return;
        };
        self.bookmarks
            .push(Bookmark::new(BKind::Heading { path, heading }, self.now));
        self.save_bookmarks();
    }

    pub(crate) fn bookmark_search(&mut self) {
        let q = String::from(self.search.text.trim());
        if q.is_empty() {
            self.say("Search for something first");
            return;
        }
        self.bookmarks
            .push(Bookmark::new(BKind::Search { query: q }, self.now));
        self.save_bookmarks();
    }

    pub(crate) fn open_bookmark(&mut self, place: &[usize], new_tab: bool) {
        let Some(b) = bookmarks::at(&self.bookmarks, place).cloned() else {
            return;
        };
        match b.kind {
            BKind::File { path } => self.open_file(&path, new_tab),
            BKind::Heading { path, heading } => {
                self.open_file(&path, new_tab);
                let line = self.doc().and_then(|d| {
                    d.headings()
                        .into_iter()
                        .find(|h| h.text == heading)
                        .map(|h| h.line)
                });
                if let (Some(line), Some(d)) = (line, self.doc_mut()) {
                    d.place(Pos::new(line, d.buf.line_len(line)), false);
                    d.top = line + 1;
                    d.top_off = 0.0;
                }
            }
            BKind::Folder { path } => self.reveal(&path),
            BKind::Search { query } => {
                self.search = Input::with(&query);
                self.run_search();
                self.left = LeftPane::Search;
                self.left_open = true;
            }
            BKind::Group { .. } => {
                let key = place.to_vec();
                if !self.bookmarks_closed.remove(&key) {
                    self.bookmarks_closed.insert(key);
                }
            }
            BKind::Other => {}
        }
    }

    /// The bookmarks as rows: place, depth, label, kind.
    pub fn bookmark_rows(&self) -> Vec<(Vec<usize>, u16, String, u8)> {
        let mut out = Vec::new();
        let closed = |p: &[usize]| self.bookmarks_closed.contains(p);
        bookmarks::flatten(&self.bookmarks, 0, &mut Vec::new(), &closed, &mut out);
        out.into_iter()
            .map(|(p, d, b)| {
                let k = match b.kind {
                    BKind::File { .. } => 0,
                    BKind::Heading { .. } => 1,
                    BKind::Folder { .. } => 2,
                    BKind::Search { .. } => 3,
                    BKind::Group { .. } => 4,
                    BKind::Other => 5,
                };
                (p, d, b.label(), k)
            })
            .collect()
    }

    // ---- unlinked mentions ------------------------------------------------------------

    /// Look for unlinked mentions of the open note a few files at a time.
    fn step_unlinked(&mut self, more: &mut dyn FnMut() -> bool) -> bool {
        if !self.unlinked_open || !self.right_open || self.right != RightPane::Backlinks {
            return false;
        }
        let Some(path) = self.doc().map(|d| d.path.clone()) else {
            return false;
        };
        let gen = self.vault.as_ref().map_or(0, |v| v.generation);
        if self.unlinked.as_ref().is_none_or(|u| u.path != path) {
            let Some(v) = self.vault.as_ref() else {
                return false;
            };
            let name = v.note(&path).map(|n| n.name.clone()).unwrap_or_default();
            // Every other note, last first so they come off the end in
            // order; a note that also links still counts its plain mentions.
            let queue = v
                .notes
                .iter()
                .rev()
                .filter(|n| {
                    n.path != path && (n.path.ends_with(".md") || n.path.ends_with(".canvas"))
                })
                .map(|n| n.path.clone())
                .collect();
            self.unlinked = Some(Unlinked {
                path,
                name,
                queue,
                found: Vec::new(),
                generation: gen,
            });
        }
        let mut changed = false;
        while more() {
            let Some(next) = self.unlinked.as_mut().and_then(|u| u.queue.pop()) else {
                break;
            };
            let Some(text) = self.text_of(&next) else {
                continue;
            };
            let Some(u) = self.unlinked.as_mut() else {
                break;
            };
            let key = lower(&u.name);
            if key.is_empty() {
                break;
            }
            let mut lines = Vec::new();
            for (n, line) in text.split('\n').enumerate() {
                if mentions(line, &key) {
                    lines.push((n as u32, String::from(line.trim())));
                }
            }
            if !lines.is_empty() {
                let key_of = |p: &str| lower(file_name(p));
                let at = u
                    .found
                    .iter()
                    .position(|(q, _)| key_of(q) > key_of(&next))
                    .unwrap_or(u.found.len());
                u.found.insert(at, (next, lines));
                changed = true;
            }
        }
        changed
    }

    /// Turn the mentions in one note into links.
    pub(crate) fn link_mention(&mut self, f: usize) {
        let Some((path, name)) = self
            .unlinked
            .as_ref()
            .and_then(|u| u.found.get(f).map(|(p, _)| (p.clone(), u.name.clone())))
        else {
            return;
        };
        let Some(text) = self.text_of(&path) else {
            return;
        };
        let key = lower(&name);
        let mut out = String::with_capacity(text.len() + 8);
        for (i, line) in text.split('\n').enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(&link_first(line, &key, &name));
        }
        self.set_text(&path, &out);
        if let Some(u) = self.unlinked.as_mut() {
            u.found.remove(f);
        }
        self.save_all();
    }

    // ---- tags and properties ------------------------------------------------------------

    /// The tags pane as rows: full tag, shown label, count, depth, and
    /// whether it has nested tags.
    pub fn tag_rows(&mut self) -> Vec<(String, String, usize, u16, bool)> {
        let flat: Vec<(String, usize)> = self.tags().to_vec();
        // Counts summed up the tree, in the order of the flat list (most
        // used first), parents before children.
        let mut count: BTreeMap<String, (String, usize)> = BTreeMap::new();
        for (t, n) in &flat {
            let mut acc = String::new();
            for part in t.split('/') {
                if !acc.is_empty() {
                    acc.push('/');
                }
                acc.push_str(part);
                let e = count.entry(lower(&acc)).or_insert_with(|| (acc.clone(), 0));
                e.1 += n;
            }
        }
        let mut kids: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (k, _) in count.iter() {
            let up = match k.rfind('/') {
                Some(i) => String::from(&k[..i]),
                None => String::new(),
            };
            kids.entry(up).or_default().push(k.clone());
        }
        for list in kids.values_mut() {
            list.sort_by(|a, b| {
                let ca = count.get(a).map_or(0, |c| c.1);
                let cb = count.get(b).map_or(0, |c| c.1);
                cb.cmp(&ca).then_with(|| a.cmp(b))
            });
        }
        let mut out = Vec::new();
        fn walk(
            at: &str,
            depth: u16,
            kids: &BTreeMap<String, Vec<String>>,
            count: &BTreeMap<String, (String, usize)>,
            closed: &alloc::collections::BTreeSet<String>,
            out: &mut Vec<(String, String, usize, u16, bool)>,
        ) {
            let Some(list) = kids.get(at) else {
                return;
            };
            for k in list {
                let (full, n) = count.get(k).cloned().unwrap_or_default();
                let label = String::from(full.rsplit('/').next().unwrap_or(&full));
                let has = kids.contains_key(k.as_str());
                out.push((full, label, n, depth, has));
                if has && !closed.contains(k) {
                    walk(k, depth + 1, kids, count, closed, out);
                }
            }
        }
        walk("", 0, &kids, &count, &self.tags_closed, &mut out);
        out
    }

    /// Every property name in the vault and how many notes have it.
    pub fn all_properties(&self) -> Vec<(String, usize)> {
        let Some(v) = self.vault.as_ref() else {
            return Vec::new();
        };
        let mut count: BTreeMap<String, usize> = BTreeMap::new();
        for m in v.meta.values() {
            if let Some(Value::Obj(kv)) = &m.props {
                for (k, _) in kv {
                    *count.entry(k.clone()).or_default() += 1;
                }
            }
        }
        let mut out: Vec<(String, usize)> = count.into_iter().collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        out
    }

    fn explorer_dirty(&mut self) {
        self.explorer.rows_dirty = true;
    }
}

/// How many times a line names `key` (lower-cased) as a word, outside
/// links.
pub fn mention_count(line: &str, key: &str) -> usize {
    let l = lower(line);
    let mut from = 0;
    let mut n = 0;
    while let Some(i) = l.get(from..).and_then(|s| s.find(key)).map(|i| i + from) {
        let before = l[..i].chars().next_back();
        let after = l[i + key.len()..].chars().next();
        let word = before.is_none_or(|c| !c.is_alphanumeric())
            && after.is_none_or(|c| !c.is_alphanumeric());
        let in_link = l[..i].rfind("[[").is_some_and(|o| !l[o..i].contains("]]"));
        if word && !in_link {
            n += 1;
        }
        from = i + key.len().max(1);
    }
    n
}

/// Whether a line names `key` (lower-cased) as a word, outside a link.
/// A note's text with property `key` set to `value` (empty: removed).
pub fn set_property_text(text: &str, key: &str, value: &str) -> String {
    let v = value.trim();
    let needs_quotes = v.contains(": ")
        || v.starts_with(['#', '[', '{', '"', '\'', '-', '*', '&'])
        || v.ends_with(':');
    let mut line = String::from(key);
    line.push(':');
    if !v.is_empty() {
        line.push(' ');
        if needs_quotes {
            line.push('"');
            line.push_str(&v.replace('"', "\\\""));
            line.push('"');
        } else {
            line.push_str(v);
        }
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let close = if lines.first().map(|l| l.trim_end()) == Some("---") {
        lines
            .iter()
            .skip(1)
            .position(|l| l.trim_end() == "---")
            .map(|i| i + 1)
    } else {
        None
    };
    let mut out: Vec<String> = Vec::new();
    match close {
        Some(end) => {
            let mut done = false;
            let mut i = 0;
            while i < lines.len() {
                let l = lines[i];
                if i > 0 && i < end && !done {
                    let k = l.split(':').next().unwrap_or("").trim();
                    if k == key && !l.starts_with(' ') && !l.starts_with('-') {
                        // The key's line and any list lines under it go.
                        i += 1;
                        while i < end && (lines[i].starts_with(' ') || lines[i].starts_with('-')) {
                            i += 1;
                        }
                        out.push(line.clone());
                        done = true;
                        continue;
                    }
                }
                if i == end && !done {
                    out.push(line.clone());
                    done = true;
                }
                out.push(String::from(l));
                i += 1;
            }
        }
        None => {
            out.push(String::from("---"));
            out.push(line);
            out.push(String::from("---"));
            out.extend(lines.iter().map(|l| String::from(*l)));
        }
    }
    out.join("\n")
}

/// A linked side tab's title, as the notes app names it.
pub fn side_title(pane: RightPane, path: &str) -> String {
    let mut s = String::from(match pane {
        RightPane::Outline => return String::from("Outline"),
        RightPane::Outgoing => "Outgoing links from ",
        _ => "Backlinks for ",
    });
    s.push_str(crate::app::strip_ext(file_name(path)));
    s
}

/// Where a new attachment for `note` goes, by the "Default location for new
/// attachments" setting: `/` the vault, `./` beside the note, `./sub` a
/// folder beside the note, anything else that folder.
pub fn attachment_folder(setting: &str, note: &str) -> String {
    let s = setting.trim();
    let here = parent(note);
    if s.is_empty() || s == "/" {
        String::new()
    } else if s == "./" || s == "." {
        String::from(here)
    } else if let Some(sub) = s.strip_prefix("./") {
        join(here, sub.trim_matches('/'))
    } else {
        String::from(s.trim_matches('/'))
    }
}

fn mentions(line: &str, key: &str) -> bool {
    let l = lower(line);
    let mut from = 0;
    while let Some(i) = l.get(from..).and_then(|s| s.find(key)).map(|i| i + from) {
        let before = l[..i].chars().next_back();
        let after = l[i + key.len()..].chars().next();
        let word = before.is_none_or(|c| !c.is_alphanumeric())
            && after.is_none_or(|c| !c.is_alphanumeric());
        let in_link = l[..i].rfind("[[").is_some_and(|o| !l[o..i].contains("]]"));
        if word && !in_link {
            return true;
        }
        from = i + key.len().max(1);
    }
    false
}

/// The line with its first unlinked mention of `key` made a link.
fn link_first(line: &str, key: &str, name: &str) -> String {
    let l = lower(line);
    let mut from = 0;
    while let Some(i) = l.get(from..).and_then(|s| s.find(key)).map(|i| i + from) {
        let before = l[..i].chars().next_back();
        let after = l[i + key.len()..].chars().next();
        let word = before.is_none_or(|c| !c.is_alphanumeric())
            && after.is_none_or(|c| !c.is_alphanumeric());
        let in_link = l[..i].rfind("[[").is_some_and(|o| !l[o..i].contains("]]"));
        if word && !in_link && line.is_char_boundary(i) && line.is_char_boundary(i + key.len()) {
            let shown = &line[i..i + key.len()];
            let mut out = String::from(&line[..i]);
            out.push_str("[[");
            out.push_str(name);
            if shown != name {
                out.push('|');
                out.push_str(shown);
            }
            out.push_str("]]");
            out.push_str(&line[i + key.len()..]);
            return out;
        }
        from = i + key.len().max(1);
    }
    String::from(line)
}

trait OwnedStr {
    fn to_string_owned(&self) -> String;
}

impl OwnedStr for str {
    fn to_string_owned(&self) -> String {
        String::from(self)
    }
}

/// The settings page, read out of an overlay without holding it.
trait PageOf {
    fn cloned_page(self) -> Option<Overlay>;
}

impl PageOf for Option<&Overlay> {
    fn cloned_page(self) -> Option<Overlay> {
        match self {
            Some(Overlay::Settings(p)) => Some(Overlay::Settings(*p)),
            _ => None,
        }
    }
}

/// A note cut into slides, as the Slides plugin cuts it: at each line that
/// is only `---`, after the properties block.
pub fn slides_of(text: &str) -> Vec<String> {
    let skip = crate::yaml::front_matter(text).map_or(0, |(_, line)| line);
    let mut out = alloc::vec![String::new()];
    for line in text.split('\n').skip(skip) {
        if line.trim_end() == "---" {
            out.push(String::new());
            continue;
        }
        if let Some(last) = out.last_mut() {
            if !last.is_empty() {
                last.push('\n');
            }
            last.push_str(line);
        }
    }
    out.retain(|s| !s.trim().is_empty());
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// The made-up path of a text card being edited with the note editor.
pub const CARD_DOC: &str = "\u{1}card/";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{mem_vault, Hit};

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

    fn app() -> App {
        let mut a = App::new();
        a.open_vault(mem_vault(
            "W",
            &[
                ("Home.md", "# Home\nSee [[Plan]].\n#hub"),
                ("Plan.md", "---\nstatus: open\ntags: [work/deep]\n---\n# Plan\n## Goals\nShip. The home page matters.\n#work"),
                ("Notes.md", "Home is where the plan is. home again.\n[[Plan]]"),
                ("Templates/Meeting.md", "# {{title}}\n{{date}} {{time}}\n"),
                ("Board.canvas", ""),
            ],
        ));
        a.vault.as_mut().unwrap().index_some(&mut || true);
        a.local = When {
            year: 2026,
            month: 11,
            day: 22,
            hour: 9,
            minute: 30,
            second: 0,
        };
        a.now = 1_000_000;
        a
    }

    #[test]
    fn a_new_vault_gets_the_default_settings_files() {
        let mut a = app();
        assert!(a
            .vault
            .as_mut()
            .unwrap()
            .fs
            .exists(".obsidian/core-plugins.json"));
        assert!(!a.settings.show_ribbon());
        assert!(
            !a.right_open,
            "the right sidebar starts closed, as in the notes app"
        );
    }

    #[test]
    fn hotkeys_default_and_own() {
        let mut a = app();
        assert_eq!(a.hotkey("g", CMD), Some(Cmd::Graph));
        assert_eq!(a.hotkey(",", CMD), Some(Cmd::Settings));
        assert_eq!(
            a.hotkey("ArrowLeft", Mods { alt: true, ..CMD }),
            Some(Cmd::Back)
        );
        assert_eq!(
            a.hotkey("Tab", Mods { ctrl: true, ..NONE }),
            Some(Cmd::NextTab)
        );
        a.settings.hotkeys.data = crate::json::parse("{\"graph:open\":[{\"modifiers\":[\"Mod\",\"Shift\"],\"key\":\"G\"}],\"daily-notes\":[{\"modifiers\":[\"Mod\"],\"key\":\"D\"}]}").unwrap();
        assert_eq!(a.hotkey("g", CMD), None, "moved by the person");
        assert_eq!(a.hotkey("g", Mods { shift: true, ..CMD }), Some(Cmd::Graph));
        assert_eq!(a.hotkey("d", CMD), Some(Cmd::DailyNote));
        // A plugin turned off takes its hotkey with it.
        a.settings.set_plugin("graph", false);
        assert_eq!(a.hotkey("g", Mods { shift: true, ..CMD }), None);
    }

    #[test]
    fn daily_notes_open_today_with_the_template() {
        let mut a = app();
        a.settings.daily.set("folder", Value::s("Journal"));
        a.settings
            .daily
            .set("template", Value::s("Templates/Meeting"));
        a.settings.daily.set("format", Value::s("YYYY-MM-DD"));
        a.run(Cmd::DailyNote);
        let d = a.doc().unwrap();
        assert_eq!(d.path, "Journal/2026-11-22.md");
        assert_eq!(d.text(), "# 2026-11-22\n2026-11-22 09:30\n");
        a.local = a.local.plus_days(3);
        a.run(Cmd::DailyNote);
        assert_eq!(a.doc().unwrap().path, "Journal/2026-11-25.md");
        a.run(Cmd::DailyPrev);
        assert_eq!(a.doc().unwrap().path, "Journal/2026-11-22.md");
        a.run(Cmd::DailyNext);
        assert_eq!(a.doc().unwrap().path, "Journal/2026-11-25.md");
    }

    #[test]
    fn templates_insert_at_the_cursor() {
        let mut a = app();
        a.settings.templates.set("folder", Value::s("Templates"));
        a.open("Home.md", false);
        let end = a.doc().unwrap().buf.end();
        a.doc_mut().unwrap().place(end, false);
        a.run(Cmd::InsertTemplate);
        a.text("meet");
        a.key("Enter", NONE);
        assert!(
            a.doc()
                .unwrap()
                .text()
                .ends_with("#hub# Home\n2026-11-22 09:30\n"),
            "{}",
            a.doc().unwrap().text()
        );
    }

    #[test]
    fn note_composer_merges_and_extracts() {
        let mut a = app();
        a.open("Home.md", false);
        a.run(Cmd::MergeFile);
        a.text("notes");
        a.key("Enter", NONE);
        assert_eq!(a.doc().unwrap().path, "Notes.md");
        assert!(a
            .doc()
            .unwrap()
            .text()
            .ends_with("[[Plan]]\n\n# Home\nSee [[Plan]].\n#hub"));
        assert!(a.vault.as_ref().unwrap().note("Home.md").is_none());
        // Extract a selection into a new note.
        a.open("Plan.md", false);
        let d = a.doc_mut().unwrap();
        d.select_range(Pos::new(6, 0), Pos::new(6, 5));
        a.run(Cmd::ExtractSelection);
        a.text("Shipping");
        a.key("Enter", NONE);
        assert!(a.doc().unwrap().text().contains("[[Shipping]] The home"));
        let made = a.vault.as_mut().unwrap().fs.read("Shipping.md").unwrap();
        assert_eq!(made, "Ship.");
    }

    #[test]
    fn hotkeys_are_recorded_reset_and_clash() {
        let mut a = app();
        a.run(Cmd::Settings);
        a.click(crate::app::Hit::SettingsNav(SettingsPage::Hotkeys), NONE);
        let rows = a.settings_rows();
        let graph = rows
            .iter()
            .position(|r| r.name == "Graph view: Open graph view")
            .unwrap();
        a.click(crate::app::Hit::SettingsRow(graph), NONE);
        assert!(a.recording.is_some());
        a.key("j", Mods { shift: true, ..CMD });
        let saved = a
            .vault
            .as_mut()
            .unwrap()
            .fs
            .read(".obsidian/hotkeys.json")
            .unwrap();
        assert!(saved.contains("\"graph:open\": [\n    {\n      \"modifiers\": [\n        \"Mod\",\n        \"Shift\"\n      ],\n      \"key\": \"J\""), "{saved}");
        a.key("Escape", NONE);
        assert_eq!(a.hotkey("j", Mods { shift: true, ..CMD }), Some(Cmd::Graph));
        // Bound to the switcher's keys: both say so.
        a.run(Cmd::Settings);
        a.click(crate::app::Hit::SettingsNav(SettingsPage::Hotkeys), NONE);
        a.click(crate::app::Hit::SettingsRow(graph), NONE);
        a.key("o", CMD);
        let ci = COMMANDS.iter().position(|c| c.cmd == Cmd::Graph).unwrap();
        assert_eq!(a.conflicts(ci), ["Quick switcher: Open quick switcher"]);
        a.click(crate::app::Hit::SettingsReset(graph), NONE);
        assert_eq!(a.hotkey("g", CMD), Some(Cmd::Graph));
    }

    #[test]
    fn stacked_tabs_side_views_and_linking() {
        let mut a = app();
        a.open("Home.md", false);
        a.open("Plan.md", true);
        a.run(Cmd::StackTabs);
        assert!(a.pane_stacked(a.pane));
        a.select_tab(0);
        a.run(Cmd::OpenOutlineHere);
        assert!(matches!(a.view(), Some(View::Side(sd)) if sd.note.as_deref() == Some("Home.md")));
        assert_eq!(a.tab().unwrap().title(), "Outline");
        let other = a.pane;
        // The note's group keeps its stacking; the side view follows it.
        a.focus_pane(0);
        assert!(a.stacked);
        a.open("Plan.md", false);
        let (tabs, act) = a.pane_tabs(other);
        match &tabs[act].view {
            Some(View::Side(sd)) => assert_eq!(sd.note.as_deref(), Some("Plan.md")),
            _ => panic!("side view"),
        }
        a.focus_pane(other);
        assert_eq!(a.outline().first().map(|h| h.text.as_str()), Some("Plan"));
        // Link with a chosen tab.
        a.focus_pane(0);
        a.run(Cmd::LinkWith);
        a.key("Enter", NONE);
        assert!(a.tab().unwrap().group.is_some());
    }

    #[test]
    fn extract_heading_attachments_and_deleting() {
        let mut a = app();
        a.open("Plan.md", false);
        a.doc_mut().unwrap().place(Pos::new(6, 0), false);
        a.run(Cmd::ExtractHeading);
        let made = a.vault.as_mut().unwrap().fs.read("Goals.md").unwrap();
        assert_eq!(made, "Ship. The home page matters.\n#work");
        assert!(a.doc().unwrap().text().contains("![[Goals]]"));
        assert_eq!(attachment_folder("/", "A/B/n.md"), "");
        assert_eq!(attachment_folder("./", "A/B/n.md"), "A/B");
        assert_eq!(attachment_folder("./files", "A/n.md"), "A/files");
        assert_eq!(attachment_folder("Attachments", "A/n.md"), "Attachments");
        a.settings
            .app
            .set("attachmentFolderPath", Value::s("./files"));
        a.attach("notes.txt", b"hello");
        assert_eq!(
            a.vault
                .as_mut()
                .unwrap()
                .fs
                .read("files/notes.txt")
                .unwrap(),
            "hello"
        );
        assert!(a.doc().unwrap().text().contains("![[notes.txt]]"));
        // "Permanently delete" asks, then deletes for good.
        a.settings.app.set("trashOption", Value::s("none"));
        a.run(Cmd::Delete);
        match &a.overlay {
            Some(Overlay::Confirm(c)) => assert!(c.body.contains("permanently")),
            _ => panic!("confirm"),
        }
        a.click(crate::app::Hit::ConfirmYes, NONE);
        assert!(!a.vault.as_mut().unwrap().fs.exists("Plan.md"));
        assert!(!a.vault.as_mut().unwrap().fs.exists(".trash/Plan.md"));
    }

    #[test]
    fn base_cells_set_properties() {
        assert_eq!(
            set_property_text(
                "---\nstatus: open\ntags:\n  - a\n---\nBody",
                "status",
                "done"
            ),
            "---\nstatus: done\ntags:\n  - a\n---\nBody"
        );
        assert_eq!(
            set_property_text("---\nstatus: open\n---\nBody", "due", "2026-12-01"),
            "---\nstatus: open\ndue: 2026-12-01\n---\nBody"
        );
        assert_eq!(
            set_property_text("Body", "k", "a: b"),
            "---\nk: \"a: b\"\n---\nBody"
        );
        let mut a = app();
        a.set_property("Plan.md", "status", "done");
        let t = a.vault.as_mut().unwrap().fs.read("Plan.md").unwrap();
        assert!(
            t.starts_with("---\nstatus: done\ntags: [work/deep]\n---"),
            "{t}"
        );
    }

    #[test]
    fn a_board_column_new_starts_with_its_value() {
        let mut a = app();
        a.vault
            .as_mut()
            .unwrap()
            .fs
            .write(
                "B.base",
                "views:\n  - type: kanban\n    name: Board\n    groupBy:\n      property: status\n      direction: ASC\n",
            )
            .unwrap();
        a.vault.as_mut().unwrap().scan();
        a.open_file("B.base", false);
        for _ in 0..4 {
            a.tick(&mut || true);
        }
        let groups = match a.view() {
            Some(View::Base(b)) => b.groups.clone(),
            _ => panic!("a base tab"),
        };
        let open = groups
            .iter()
            .position(|(l, _)| l == "open")
            .expect("an open column");
        a.base_new(open);
        let path = a.active_path().unwrap();
        assert!(path.starts_with("Untitled"), "{path}");
        let t = a.vault.as_mut().unwrap().fs.read(&path).unwrap();
        assert!(t.starts_with("---\nstatus: open\n---"), "{t}");
    }

    #[test]
    fn a_board_card_moved_to_a_column_takes_its_value() {
        let mut a = app();
        a.vault
            .as_mut()
            .unwrap()
            .fs
            .write(
                "B.base",
                "views:\n  - type: kanban\n    name: Board\n    groupBy:\n      property: status\n      direction: ASC\n",
            )
            .unwrap();
        a.vault
            .as_mut()
            .unwrap()
            .fs
            .write("Done.md", "---\nstatus: done\n---\nx")
            .unwrap();
        a.vault.as_mut().unwrap().scan();
        a.vault.as_mut().unwrap().reindex_all();
        a.vault.as_mut().unwrap().index_some(&mut || true);
        a.open_file("B.base", false);
        for _ in 0..4 {
            a.tick(&mut || true);
        }
        let (rows, groups) = match a.view() {
            Some(View::Base(b)) => (b.rows.clone(), b.groups.clone()),
            _ => panic!("a base"),
        };
        let row = rows.iter().position(|p| p == "Plan.md").unwrap();
        let done = groups
            .iter()
            .position(|(l, _)| l == "done")
            .unwrap_or_else(|| panic!("{groups:?}"));
        a.base_move_card(row, done);
        let t = a.vault.as_mut().unwrap().fs.read("Plan.md").unwrap();
        assert!(t.starts_with("---\nstatus: done\n"), "{t}");
    }

    #[test]
    fn a_note_is_cut_into_slides() {
        let s = slides_of("---\ntags: [x]\n---\n# One\nhello\n---\n# Two\n\n---\n");
        assert_eq!(
            s,
            alloc::vec![String::from("# One\nhello"), String::from("# Two\n")]
        );
        let mut a = app();
        a.open_file("Plan.md", false);
        a.run(Cmd::StartPresentation);
        assert!(a.slides.is_some());
        a.key("ArrowRight", Mods::default());
        a.key("Escape", Mods::default());
        assert!(a.slides.is_none());
    }

    #[test]
    fn random_and_unique_notes() {
        let mut a = app();
        a.open_file("Home.md", false);
        a.open_random_note();
        let p = a.active_path().unwrap();
        assert!(p != "Home.md" && p.ends_with(".md"), "{p}");
        a.new_unique_note();
        assert_eq!(a.active_path().as_deref(), Some("202611220930.md"));
    }

    #[test]
    fn panes_split_pin_and_close() {
        let mut a = app();
        a.open("Home.md", false);
        a.run(Cmd::SplitRight);
        assert_eq!(a.pane_order().len(), 2);
        assert_eq!(a.active_path().as_deref(), Some("Home.md"));
        a.open("Plan.md", false);
        assert_eq!(a.pane_tabs(0).0[0].path(), Some("Home.md"));
        // Pin: a link opens beside it.
        a.run(Cmd::TogglePin);
        a.open("Notes.md", false);
        assert_eq!(a.tabs.len(), 2);
        assert!(a.tabs[0].pinned);
        a.select_tab(0);
        a.run(Cmd::CloseTab);
        assert_eq!(a.tabs.len(), 2, "a pinned tab stays");
        // Drag a tab into the other group.
        a.move_tab(1, 1, 0, 1);
        assert_eq!(a.pane, 0);
        assert_eq!(a.tabs.len(), 2);
        // Closing the last tab of a group takes the group away.
        a.focus_pane(1);
        a.toggle_pin(0);
        a.close_tab(0);
        assert_eq!(a.pane_order().len(), 1);
        a.run(Cmd::UndoCloseTab);
        assert_eq!(a.active_path().as_deref(), Some("Plan.md"));
    }

    #[test]
    fn linked_tabs_follow() {
        let mut a = app();
        a.open("Home.md", false);
        a.run(Cmd::LocalGraph);
        assert!(matches!(a.view(), Some(View::Graph(g)) if g.local.as_deref() == Some("Home.md")));
        a.focus_pane(0);
        a.open("Plan.md", false);
        let p = a.pane_order()[1];
        let (tabs, act) = a.pane_tabs(p);
        match &tabs[act].view {
            Some(View::Graph(g)) => assert_eq!(g.local.as_deref(), Some("Plan.md")),
            _ => panic!("the local graph tab"),
        }
    }

    #[test]
    fn graph_and_base_tabs_fill_between_frames() {
        let mut a = app();
        a.run(Cmd::Graph);
        a.tick(&mut || true);
        match a.view() {
            Some(View::Graph(g)) => assert!(g.graph.as_ref().unwrap().nodes.len() >= 4),
            _ => panic!("graph"),
        }
        a.vault
            .as_mut()
            .unwrap()
            .fs
            .write("Work.base", "filters:\n  and:\n    - file.hasTag(\"work\")\nviews:\n  - type: table\n    name: W\n    order:\n      - file.name\n      - status\n")
            .unwrap();
        a.vault.as_mut().unwrap().scan();
        a.open_file("Work.base", false);
        a.tick(&mut || true);
        match a.view() {
            Some(View::Base(b)) => assert_eq!(b.rows, ["Plan.md"]),
            _ => panic!("base"),
        }
    }

    #[test]
    fn canvas_cards_are_added_typed_saved_and_undone() {
        let mut a = app();
        a.open_file("Board.canvas", false);
        a.canvas_add(CardKind::Text, "");
        a.text("Hello");
        a.key("Enter", NONE);
        a.text("there");
        a.key("Escape", NONE);
        a.now += 5_000;
        a.tick(&mut || true);
        let saved = a.vault.as_mut().unwrap().fs.read("Board.canvas").unwrap();
        assert!(saved.contains("\"text\":\"Hello\\nthere\""), "{saved}");
        a.key("z", CMD);
        match a.view() {
            Some(View::Canvas(c)) => assert_eq!(c.canvas.nodes[0].text.as_deref(), Some("")),
            _ => panic!("canvas"),
        }
    }

    #[test]
    fn bookmarks_unlinked_mentions_tags_and_properties() {
        let mut a = app();
        a.open("Home.md", false);
        a.run(Cmd::BookmarkFile);
        a.search = Input::with("tag:#work");
        a.run(Cmd::BookmarkSearch);
        let saved = a
            .vault
            .as_mut()
            .unwrap()
            .fs
            .read(".obsidian/bookmarks.json")
            .unwrap();
        assert!(saved.contains("\"path\": \"Home.md\""), "{saved}");
        assert_eq!(a.bookmark_rows().len(), 2);
        a.click(Hit::Bookmark(alloc::vec![0]), NONE);
        assert_eq!(a.active_path().as_deref(), Some("Home.md"));
        // Unlinked mentions: Plan and Notes name "home" without a link.
        a.right_open = true;
        a.right = RightPane::Backlinks;
        a.unlinked_open = true;
        a.tick(&mut || true);
        let found: Vec<String> = a
            .unlinked
            .as_ref()
            .unwrap()
            .found
            .iter()
            .map(|f| f.0.clone())
            .collect();
        assert_eq!(found.len(), 2, "{found:?}");
        let ni = found.iter().position(|p| p == "Notes.md").unwrap();
        a.link_mention(ni);
        let notes = a.vault.as_mut().unwrap().fs.read("Notes.md").unwrap();
        assert!(notes.starts_with("[[Home]] is where"), "{notes}");
        // Nested tags and properties.
        let rows = a.tag_rows();
        let labels: Vec<(String, usize, u16)> =
            rows.iter().map(|r| (r.1.clone(), r.2, r.3)).collect();
        assert!(labels.contains(&(String::from("work"), 2, 0)), "{labels:?}");
        assert!(labels.contains(&(String::from("deep"), 1, 1)), "{labels:?}");
        let props = a.all_properties();
        assert!(props.contains(&(String::from("status"), 1)));
    }

    #[test]
    fn settings_rows_change_the_files() {
        let mut a = app();
        a.run(Cmd::Settings);
        a.click(Hit::SettingsNav(SettingsPage::Appearance), NONE);
        let rows = a.settings_rows();
        let ribbon = rows.iter().position(|r| r.name == "Show ribbon").unwrap();
        a.click(Hit::SettingsRow(ribbon), NONE);
        assert!(a.settings.show_ribbon());
        let theme = rows
            .iter()
            .position(|r| r.name == "Base color scheme")
            .unwrap();
        a.click(Hit::SettingsRow(theme), NONE);
        assert!(!a.dark, "dark, then light");
        let saved = a
            .vault
            .as_mut()
            .unwrap()
            .fs
            .read(".obsidian/appearance.json")
            .unwrap();
        assert!(saved.contains("\"theme\": \"moonstone\""), "{saved}");
        // A text setting is typed in.
        a.click(Hit::SettingsNav(SettingsPage::DailyNotes), NONE);
        a.click(Hit::SettingsRow(1), NONE);
        a.text("Journal");
        a.key("Enter", NONE);
        assert_eq!(a.settings.daily_folder(), "Journal");
        // Core plugins switch off and their commands go from the palette.
        a.click(Hit::SettingsNav(SettingsPage::CorePlugins), NONE);
        let graph = a
            .settings_rows()
            .iter()
            .position(|r| r.name == "Graph view")
            .unwrap();
        a.click(Hit::SettingsRow(graph), NONE);
        assert!(!a.settings.plugin_on("graph"));
        a.key("Escape", NONE);
        a.key("p", CMD);
        a.text("graph view");
        match &a.overlay {
            Some(Overlay::List(p)) => {
                assert!(p.items.iter().all(|i| !i.title.starts_with("Graph view")))
            }
            _ => panic!("palette"),
        }
    }

    #[test]
    fn recovery_keeps_copies_and_restores() {
        let mut a = app();
        a.open("Plan.md", false);
        let p = String::from("Plan.md");
        a.recovery.note(&p, "old text", 0, 0);
        a.run(Cmd::Recovery);
        assert!(matches!(a.view(), Some(View::Recovery(_))));
        a.click(Hit::RecoveryRestore, NONE);
        a.click(Hit::ConfirmYes, NONE);
        assert_eq!(a.doc().unwrap().text(), "old text");
        assert_eq!(
            a.recovery.snapshots("Plan.md").len(),
            2,
            "the text before is kept"
        );
    }

    #[test]
    fn mention_words_not_parts_or_links() {
        assert!(mentions("Go home now", "home"));
        assert!(!mentions("homework", "home"));
        assert!(!mentions("see [[home]]", "home"));
        assert_eq!(mention_count("home, home and [[home]] at home2", "home"), 2);
        assert_eq!(
            link_first("the Home page", "home", "Home"),
            "the [[Home]] page"
        );
        assert_eq!(link_first("go home", "home", "Home"), "go [[Home|home]]");
    }
}
