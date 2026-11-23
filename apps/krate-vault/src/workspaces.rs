//! Workspaces (a core plugin): layouts saved by name in the vault's
//! `.obsidian/workspaces.json`, in the notes app's shape -- each a
//! workspace.json's `main` (and its windows, sidebars and ribbon), with the
//! `active` leaf and an `mtime` -- and the one in use named at `active`.
//! Saving keeps the others; loading puts the saved groups and tabs back.

use crate::app::{App, Ask, AskFor, Input, ListKind, Overlay, Tab};
use crate::json::{self, Value};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

const FILE: &str = ".obsidian/workspaces.json";

fn read(app: &mut App, path: &str) -> Value {
    app.vault
        .as_mut()
        .and_then(|v| v.fs.read(path).ok())
        .and_then(|t| json::parse(&t))
        .filter(|d| matches!(d, Value::Obj(_)))
        .unwrap_or_else(Value::obj)
}

/// The id of the leaf showing `file` in a layout, for `active`.
fn leaf_for(node: &Value, file: &str) -> Option<String> {
    if node.get_str("type") == Some("leaf") {
        let f = node
            .get("state")
            .and_then(|s| s.get("state"))
            .and_then(|s| s.get_str("file"));
        return (f == Some(file)).then(|| String::from(node.get_str("id").unwrap_or("")));
    }
    node.get("children")?
        .arr()
        .iter()
        .find_map(|c| leaf_for(c, file))
}

impl App {
    /// The saved layouts' names, in the file's order.
    pub fn workspace_names(&mut self) -> Vec<String> {
        match read(self, FILE).get("workspaces") {
            Some(Value::Obj(kv)) => kv.iter().map(|(k, _)| k.clone()).collect(),
            _ => Vec::new(),
        }
    }

    /// The layout in use, as the file names it.
    pub fn workspace_active(&mut self) -> String {
        String::from(read(self, FILE).get_str("active").unwrap_or(""))
    }

    /// Save the layout as it is now under `name`.
    pub fn save_layout(&mut self, name: &str) {
        if self.vault.is_none() || name.trim().is_empty() {
            return;
        }
        let (main, floating) = self.layout_values();
        let current = read(self, ".obsidian/workspace.json");
        let mut rec = Value::obj();
        rec.set("main", main.clone());
        if !self.floating.is_empty() {
            rec.set("floating", floating);
        }
        // The sidebars and the ribbon as the notes app last wrote them.
        for k in ["left", "right", "left-ribbon"] {
            if let Some(v) = current.get(k) {
                rec.set(k, v.clone());
            }
        }
        if let Some(id) = self.active_path().and_then(|p| leaf_for(&main, &p)) {
            rec.set("active", Value::s(&id));
        }
        let off = self.utc_offset_min();
        rec.set("mtime", Value::s(&stamp(self.local, off)));
        let mut doc = read(self, FILE);
        let mut all = match doc.get("workspaces") {
            Some(Value::Obj(kv)) => Value::Obj(kv.clone()),
            _ => Value::obj(),
        };
        all.set(name.trim(), rec);
        doc.set("workspaces", all);
        doc.set("active", Value::s(name.trim()));
        let text = json::pretty(&doc, "  ");
        if let Some(v) = self.vault.as_mut() {
            match v.fs.write(FILE, &text) {
                Ok(()) => self.say(&format!(
                    "Successfully saved layout \u{201C}{}\u{201D}",
                    name.trim()
                )),
                Err(e) => self.say(&e),
            }
        }
    }

    /// Put a saved layout back: its groups, tabs and windows.
    pub fn load_layout(&mut self, name: &str) {
        let doc = read(self, FILE);
        let Some(rec) = doc.get("workspaces").and_then(|w| w.get(name)).cloned() else {
            return;
        };
        self.save_all();
        // Start again from one empty group, as the layout is read into it.
        self.tabs = alloc::vec![Tab::empty()];
        self.active = 0;
        self.panes = alloc::vec![crate::app::Pane {
            tabs: Vec::new(),
            active: 0,
            stacked: false,
        }];
        self.pane = 0;
        self.stacked = false;
        self.layout = crate::panes::Layout::Pane(0);
        self.floating.clear();
        self.restore_from(&rec);
        let mut doc = doc;
        doc.set("active", Value::s(name));
        if let Some(v) = self.vault.as_mut() {
            let _ = v.fs.write(FILE, &json::pretty(&doc, "  "));
        }
        self.save_workspace();
    }

    pub fn delete_layout(&mut self, name: &str) {
        let mut doc = read(self, FILE);
        if let Some(Value::Obj(kv)) = doc.get_mut("workspaces") {
            kv.retain(|(k, _)| k != name);
        }
        if doc.get_str("active") == Some(name) {
            doc.remove("active");
        }
        if let Some(v) = self.vault.as_mut() {
            let _ = v.fs.write(FILE, &json::pretty(&doc, "  "));
        }
    }

    /// "Workspaces: Save layout": the layout in use saved again, or the
    /// Manage dialog when none is.
    pub fn save_active_layout(&mut self) {
        let cur = self.workspace_active();
        if !cur.is_empty() && self.workspace_names().contains(&cur) {
            self.save_layout(&cur);
        } else {
            self.manage_layouts();
        }
    }

    /// "Manage workspace layouts": a name to save the layout as, then each
    /// saved one with Load and delete.
    pub fn manage_layouts(&mut self) {
        self.workspace_list = self.layout_rows();
        self.overlay = Some(Overlay::Ask(Ask {
            title: String::from("Manage workspace layouts"),
            input: Input::with(""),
            purpose: AskFor::WorkspaceName,
        }));
    }

    /// The Manage dialog's Save: the layout saved under the name, made the
    /// one in use, and the dialog left open with the list redrawn.
    pub fn manage_save(&mut self, name: &str) {
        if name.trim().is_empty() {
            self.say("Please enter a name for the new layout.");
        } else {
            self.save_layout(name);
        }
        self.manage_layouts();
    }

    /// "Load workspace layout" and "Save and load another layout": the
    /// saved layouts to choose from.
    pub fn pick_layout(&mut self, save_first: bool) {
        self.workspace_list = self.layout_rows();
        self.layout_save_first = save_first;
        self.open_list(ListKind::Workspaces);
    }

    /// A layout chosen in the picker.
    pub fn pick_layout_done(&mut self, name: &str) {
        if core::mem::take(&mut self.layout_save_first) {
            let cur = self.workspace_active();
            if !cur.is_empty() && self.workspace_names().contains(&cur) {
                self.save_layout(&cur);
            }
        }
        self.load_layout(name);
    }

    /// Each saved layout: its name, when it was saved, and whether it is
    /// the one in use.
    pub fn layout_rows(&mut self) -> Vec<(String, String, bool)> {
        let doc = read(self, FILE);
        let active = String::from(doc.get_str("active").unwrap_or(""));
        let now = self.now / 1000;
        match doc.get("workspaces") {
            Some(Value::Obj(kv)) => kv
                .iter()
                .map(|(k, v)| {
                    let when = v
                        .get_str("mtime")
                        .and_then(epoch_of)
                        .map(|t| format!("Modified {}", from_now(now as i64 - t)))
                        .unwrap_or_default();
                    (k.clone(), when, *k == active)
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The local time's offset from UTC, in minutes, to the quarter hour.
    fn utc_offset_min(&self) -> i64 {
        if self.now == 0 {
            return 0;
        }
        let w = self.local;
        let local = crate::dates::days_from_civil(w.year, w.month, w.day) * 86_400
            + i64::from(w.hour) * 3600
            + i64::from(w.minute) * 60
            + i64::from(w.second);
        let diff = local - (self.now / 1000) as i64;
        (diff as f64 / 900.0).round() as i64 * 15
    }
}

/// `moment().format()`: the local time with its offset.
fn stamp(w: crate::dates::When, off: i64) -> String {
    let sign = if off < 0 { '-' } else { '+' };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{}{:02}:{:02}",
        w.year,
        w.month,
        w.day,
        w.hour,
        w.minute,
        w.second,
        sign,
        off.abs() / 60,
        off.abs() % 60
    )
}

/// Seconds since 1970 of a `moment().format()` time.
fn epoch_of(s: &str) -> Option<i64> {
    let n = |a: usize, b: usize| s.get(a..b)?.parse::<i64>().ok();
    let days = crate::dates::days_from_civil(n(0, 4)? as i32, n(5, 7)? as u8, n(8, 10)? as u8);
    let mut t = days * 86_400 + n(11, 13)? * 3600 + n(14, 16)? * 60 + n(17, 19)?;
    if let Some(rest) = s.get(19..).filter(|r| r.len() >= 6) {
        let off = n(20, 22)? * 60 + n(23, 25)?;
        t -= if rest.starts_with('-') { -off } else { off } * 60;
    }
    Some(t)
}

/// moment's `fromNow()`, for a time `secs` ago.
fn from_now(secs: i64) -> String {
    let s = secs.max(0) as f64;
    let (m, h, d) = (s / 60.0, s / 3600.0, s / 86_400.0);
    let r = |x: f64| x.round() as i64;
    if s < 45.0 {
        String::from("a few seconds ago")
    } else if s < 90.0 {
        String::from("a minute ago")
    } else if m < 45.0 {
        format!("{} minutes ago", r(m))
    } else if m < 90.0 {
        String::from("an hour ago")
    } else if h < 22.0 {
        format!("{} hours ago", r(h))
    } else if h < 36.0 {
        String::from("a day ago")
    } else if d < 26.0 {
        format!("{} days ago", r(d))
    } else if d < 45.0 {
        String::from("a month ago")
    } else if d < 320.0 {
        format!("{} months ago", r(d / 30.4))
    } else if d < 548.0 {
        String::from("a year ago")
    } else {
        format!("{} years ago", r(d / 365.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::Vault;
    use crate::vfs::{MemFs, Vfs};
    use alloc::boxed::Box;

    #[test]
    fn layouts_save_and_load_by_name_beside_the_notes_apps_own() {
        let mut fs = MemFs::new();
        fs.write("Home.md", "# Home\n").unwrap();
        fs.write("Other.md", "Other\n").unwrap();
        // A file the notes app 1.15.1 wrote itself (its Workspaces plugin,
        // through its developer tools): its layout is read back.
        fs.write(FILE, include_str!("../fixtures/obsidian-workspaces.json"))
            .unwrap();
        let mut a = App::new();
        a.open_vault(Vault::open("v", Box::new(fs)));
        assert_eq!(a.workspace_names(), alloc::vec![String::from("Writing")]);
        a.open_file("Other.md", false);
        a.save_layout("Reading");
        assert_eq!(
            a.workspace_names(),
            alloc::vec![String::from("Writing"), String::from("Reading")]
        );
        assert_eq!(a.workspace_active(), "Reading");
        a.load_layout("Writing");
        assert_eq!(a.active_path().as_deref(), Some("Home.md"));
        assert_eq!(a.all_tabs().filter(|t| t.path().is_some()).count(), 1);
        a.load_layout("Reading");
        assert_eq!(a.active_path().as_deref(), Some("Other.md"));
        // The notes app's layout is kept as it wrote it.
        let text = a.vault.as_mut().unwrap().fs.read(FILE).unwrap();
        assert!(text.contains("\"left-ribbon\""));
        assert_eq!(epoch_of("1970-01-01T05:30:00+05:30"), Some(0));
        assert_eq!(from_now(22 * 60), "22 minutes ago");
        assert_eq!(stamp(a.local, 330).len(), "2026-11-23T10:45:42+05:30".len());
        a.delete_layout("Writing");
        assert_eq!(a.workspace_names(), alloc::vec![String::from("Reading")]);
    }
}
