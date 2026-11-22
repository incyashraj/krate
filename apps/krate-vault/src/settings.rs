//! The vault's settings, kept where the well-known notes app keeps them:
//! `<vault>/.obsidian/*.json`, in its own format. The same vault opens with
//! the same settings in both apps. Each file is held as the JSON it was read
//! as, so keys Vault does not use are written back untouched.

use crate::json::{self, Value};
use crate::vfs::Vfs;
use alloc::string::String;
use alloc::vec::Vec;

pub const DIR: &str = ".obsidian";

/// The core plugins, in the order the settings list them, with whether
/// each is on in a new vault.
pub const CORE_PLUGINS: &[(&str, &str, bool)] = &[
    ("file-explorer", "Files", true),
    ("global-search", "Search", true),
    ("switcher", "Quick switcher", true),
    ("graph", "Graph view", true),
    ("backlink", "Backlinks", true),
    ("canvas", "Canvas", true),
    ("outgoing-link", "Outgoing links", true),
    ("tag-pane", "Tags view", true),
    ("footnotes", "Footnotes view", false),
    ("properties", "Properties view", true),
    ("page-preview", "Page preview", true),
    ("daily-notes", "Daily notes", true),
    ("templates", "Templates", true),
    ("note-composer", "Note composer", true),
    ("command-palette", "Command palette", true),
    ("slash-command", "Slash commands", false),
    ("editor-status", "Editing toolbar", true),
    ("bookmarks", "Bookmarks", true),
    ("zk-prefixer", "Unique note creator", false),
    ("random-note", "Random note", false),
    ("outline", "Outline", true),
    ("word-count", "Word count", true),
    ("slides", "Slides", false),
    ("audio-recorder", "Audio recorder", false),
    ("workspaces", "Workspaces", false),
    ("file-recovery", "File recovery", true),
    ("publish", "Publish", false),
    ("sync", "Sync", true),
    ("bases", "Bases", true),
    ("webviewer", "Web viewer", false),
];

/// One settings file: its name and the JSON in it.
#[derive(Debug, Clone, PartialEq)]
pub struct File {
    pub name: &'static str,
    pub data: Value,
    pub dirty: bool,
}

impl File {
    fn new(name: &'static str) -> File {
        File {
            name,
            data: Value::obj(),
            dirty: false,
        }
    }

    pub fn str_or<'a>(&'a self, key: &str, or: &'a str) -> &'a str {
        self.data.get_str(key).unwrap_or(or)
    }

    pub fn bool_or(&self, key: &str, or: bool) -> bool {
        self.data.get_bool(key).unwrap_or(or)
    }

    pub fn num_or(&self, key: &str, or: f64) -> f64 {
        self.data.get_num(key).unwrap_or(or)
    }

    pub fn set(&mut self, key: &str, v: Value) {
        if self.data.get(key) != Some(&v) {
            self.data.set(key, v);
            self.dirty = true;
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// `app.json`: editor, files and links.
    pub app: File,
    /// `appearance.json`: theme, font size, ribbon.
    pub appearance: File,
    /// `core-plugins.json`: which core plugins are on.
    pub core: File,
    pub daily: File,
    pub templates: File,
    pub graph: File,
    pub bookmarks: File,
    pub recovery: File,
    pub hotkeys: File,
}

impl Default for Settings {
    fn default() -> Self {
        Settings::new()
    }
}

impl Settings {
    pub fn new() -> Settings {
        Settings {
            app: File::new("app.json"),
            appearance: File::new("appearance.json"),
            core: File::new("core-plugins.json"),
            daily: File::new("daily-notes.json"),
            templates: File::new("templates.json"),
            graph: File::new("graph.json"),
            bookmarks: File::new("bookmarks.json"),
            recovery: File::new("file-recovery.json"),
            hotkeys: File::new("hotkeys.json"),
        }
    }

    fn files_mut(&mut self) -> [&mut File; 9] {
        [
            &mut self.app,
            &mut self.appearance,
            &mut self.core,
            &mut self.daily,
            &mut self.templates,
            &mut self.graph,
            &mut self.bookmarks,
            &mut self.recovery,
            &mut self.hotkeys,
        ]
    }

    /// Read every settings file the vault has. A vault with none gets the
    /// files a new vault starts with (written on the first save).
    pub fn load(fs: &mut dyn Vfs) -> Settings {
        let mut s = Settings::new();
        let fresh = !fs.exists(DIR);
        for f in s.files_mut() {
            let mut p = String::from(DIR);
            p.push('/');
            p.push_str(f.name);
            if let Ok(text) = fs.read(&p) {
                if let Some(v) = json::parse(&text) {
                    f.data = v;
                }
            }
        }
        if fresh {
            // What a new vault writes at once.
            s.appearance.set("showRibbon", Value::Bool(false));
            s.app.dirty = true;
            let mut core = Value::obj();
            for (id, _, on) in CORE_PLUGINS {
                core.set(id, Value::Bool(*on));
            }
            s.core.data = core;
            s.core.dirty = true;
        }
        s
    }

    /// Write the files that changed. Errors are returned, one message.
    pub fn save(&mut self, fs: &mut dyn Vfs) -> Result<(), String> {
        let mut err = None;
        let any = self.files_mut().iter().any(|f| f.dirty);
        if any && !fs.exists(DIR) {
            fs.mkdir(DIR)?;
        }
        for f in self.files_mut() {
            if !f.dirty {
                continue;
            }
            let mut p = String::from(DIR);
            p.push('/');
            p.push_str(f.name);
            match fs.write(&p, &json::pretty(&f.data, "  ")) {
                Ok(()) => f.dirty = false,
                Err(e) => err = Some(e),
            }
        }
        err.map_or(Ok(()), Err)
    }

    pub fn dirty(&self) -> bool {
        [
            &self.app,
            &self.appearance,
            &self.core,
            &self.daily,
            &self.templates,
            &self.graph,
            &self.bookmarks,
            &self.recovery,
            &self.hotkeys,
        ]
        .iter()
        .any(|f| f.dirty)
    }

    // ---- what the settings say ------------------------------------------

    pub fn plugin_on(&self, id: &str) -> bool {
        let default = CORE_PLUGINS
            .iter()
            .find(|(i, _, _)| *i == id)
            .is_some_and(|(_, _, on)| *on);
        self.core.bool_or(id, default)
    }

    pub fn set_plugin(&mut self, id: &str, on: bool) {
        // A first change writes every plugin, the way the app does.
        if !matches!(self.core.data, Value::Obj(ref kv) if !kv.is_empty()) {
            let mut core = Value::obj();
            for (i, _, d) in CORE_PLUGINS {
                core.set(i, Value::Bool(*d));
            }
            self.core.data = core;
        }
        self.core.set(id, Value::Bool(on));
    }

    /// Dark unless the vault says `moonstone` (light). `system` follows the
    /// computer; Vault is told whether that is dark.
    pub fn dark(&self, system_dark: bool) -> bool {
        match self.appearance.str_or("theme", "obsidian") {
            "moonstone" => false,
            "system" => system_dark,
            _ => true,
        }
    }

    pub fn set_dark(&mut self, dark: bool) {
        self.appearance.set(
            "theme",
            Value::s(if dark { "obsidian" } else { "moonstone" }),
        );
    }

    pub fn font_size(&self) -> f32 {
        (self.appearance.num_or("baseFontSize", 16.0) as f32).clamp(10.0, 30.0)
    }

    pub fn show_ribbon(&self) -> bool {
        self.appearance.bool_or("showRibbon", false)
    }

    pub fn show_inline_title(&self) -> bool {
        self.appearance.bool_or("showInlineTitle", true)
    }

    pub fn show_view_header(&self) -> bool {
        self.appearance.bool_or("showViewHeader", true)
    }

    pub fn readable_line_length(&self) -> bool {
        self.app.bool_or("readableLineLength", true)
    }

    pub fn live_preview(&self) -> bool {
        self.app.bool_or("livePreview", true)
    }

    /// `source` (editing) or `preview` (reading), for notes as they open.
    pub fn default_view_reading(&self) -> bool {
        self.app.str_or("defaultViewMode", "source") == "preview"
    }

    pub fn always_update_links(&self) -> bool {
        self.app.bool_or("alwaysUpdateLinks", false)
    }

    pub fn prompt_delete(&self) -> bool {
        self.app.bool_or("promptDelete", true)
    }

    /// `root`, `current` or `folder` (then `new_file_folder`).
    pub fn new_file_location(&self) -> &str {
        self.app.str_or("newFileLocation", "root")
    }

    pub fn new_file_folder(&self) -> &str {
        self.app.str_or("newFileFolderPath", "")
    }

    pub fn auto_pair_brackets(&self) -> bool {
        self.app.bool_or("autoPairBrackets", true)
    }

    pub fn show_line_number(&self) -> bool {
        self.app.bool_or("showLineNumber", false)
    }

    pub fn spellcheck(&self) -> bool {
        self.app.bool_or("spellcheck", true)
    }

    pub fn daily_folder(&self) -> &str {
        self.daily.str_or("folder", "")
    }

    pub fn daily_format(&self) -> &str {
        let f = self.daily.str_or("format", "");
        if f.is_empty() {
            "YYYY-MM-DD"
        } else {
            f
        }
    }

    pub fn daily_template(&self) -> &str {
        self.daily.str_or("template", "")
    }

    pub fn templates_folder(&self) -> &str {
        self.templates.str_or("folder", "")
    }

    pub fn template_date_format(&self) -> &str {
        let f = self.templates.str_or("dateFormat", "");
        if f.is_empty() {
            "YYYY-MM-DD"
        } else {
            f
        }
    }

    pub fn template_time_format(&self) -> &str {
        let f = self.templates.str_or("timeFormat", "");
        if f.is_empty() {
            "HH:mm"
        } else {
            f
        }
    }

    pub fn recovery_interval_ms(&self) -> u64 {
        (self.recovery.num_or("intervalMinutes", 5.0).max(1.0) * 60_000.0) as u64
    }

    pub fn recovery_keep_days(&self) -> u64 {
        self.recovery.num_or("keepDays", 7.0).max(1.0) as u64
    }

    /// Hotkeys the person set, for a command id: `(primary, shift, alt, key)`.
    pub fn hotkeys_for(&self, id: &str) -> Option<Vec<(bool, bool, bool, String)>> {
        let list = self.hotkeys.data.get(id)?;
        Some(
            list.arr()
                .iter()
                .filter_map(|h| {
                    let key = String::from(h.get_str("key")?);
                    let mods: Vec<&str> = h
                        .get("modifiers")
                        .map(|m| m.arr().iter().filter_map(Value::str).collect())
                        .unwrap_or_default();
                    Some((
                        mods.iter()
                            .any(|m| *m == "Mod" || *m == "Meta" || *m == "Ctrl"),
                        mods.contains(&"Shift"),
                        mods.contains(&"Alt"),
                        key,
                    ))
                })
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::MemFs;

    #[test]
    fn a_new_vault_writes_the_files_a_new_vault_has() {
        let mut fs = MemFs::new();
        let mut s = Settings::load(&mut fs);
        assert!(s.plugin_on("graph"));
        assert!(!s.plugin_on("random-note"));
        assert!(!s.show_ribbon());
        assert!(s.dark(false));
        s.save(&mut fs).unwrap();
        let core = fs.read(".obsidian/core-plugins.json").unwrap();
        assert!(core.starts_with("{\n  \"file-explorer\": true,"), "{core}");
        assert_eq!(
            fs.read(".obsidian/appearance.json").unwrap(),
            "{\n  \"showRibbon\": false\n}"
        );
        assert_eq!(fs.read(".obsidian/app.json").unwrap(), "{}");
    }

    #[test]
    fn unknown_keys_survive_a_change() {
        let mut fs = MemFs::new();
        fs.put(
            ".obsidian/app.json",
            "{\n  \"vimMode\": true,\n  \"readableLineLength\": false\n}",
        );
        fs.put(".obsidian/appearance.json", "{\"theme\":\"moonstone\"}");
        fs.put(
            ".obsidian/daily-notes.json",
            "{\"folder\":\"Journal\",\"format\":\"DD.MM.YYYY\"}",
        );
        let mut s = Settings::load(&mut fs);
        assert!(!s.readable_line_length());
        assert!(!s.dark(true));
        assert_eq!(s.daily_folder(), "Journal");
        assert_eq!(s.daily_format(), "DD.MM.YYYY");
        s.app.set("readableLineLength", Value::Bool(true));
        s.set_dark(true);
        s.save(&mut fs).unwrap();
        assert_eq!(
            fs.read(".obsidian/app.json").unwrap(),
            "{\n  \"vimMode\": true,\n  \"readableLineLength\": true\n}"
        );
        assert!(fs
            .read(".obsidian/appearance.json")
            .unwrap()
            .contains("\"theme\": \"obsidian\""));
        assert!(!s.dirty());
    }

    #[test]
    fn hotkeys_read_in_the_apps_format() {
        let mut fs = MemFs::new();
        fs.put(
            ".obsidian/hotkeys.json",
            "{\"graph:open\":[{\"modifiers\":[\"Mod\",\"Shift\"],\"key\":\"G\"}]}",
        );
        let s = Settings::load(&mut fs);
        assert_eq!(
            s.hotkeys_for("graph:open"),
            Some(alloc::vec![(true, true, false, String::from("G"))])
        );
        assert_eq!(s.hotkeys_for("switcher:open"), None);
    }
}
