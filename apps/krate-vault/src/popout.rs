//! Tabs in windows of their own, as the notes app opens them: "Open in new
//! window" copies the tab there, "Move to new window" takes it there. Each
//! such window holds one tab group of its own, outside the main window's
//! layout (it can be split again inside); closing it closes its tabs (nothing goes back to the main
//! window). The windows and the main window's groups are kept in the
//! vault's `.obsidian/workspace.json`, under `main` and `floating`, as the
//! notes app keeps them, so the layout comes back next time.

use crate::app::{App, Pane, Tab, View};
use crate::json::{self, Value};
use crate::panes::Layout;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

const WORKSPACE: &str = ".obsidian/workspace.json";

/// A window of its own holding tab groups.
#[derive(Debug, Clone, PartialEq)]
pub struct Floating {
    /// Which window this is, for the life of the run.
    pub id: u64,
    /// Its tab groups, by index in the app's groups, as they are split.
    pub layout: Layout,
    /// Where the window was and how big, in points. Krate cannot place a
    /// window yet (K-1352): the place is kept so the file round-trips.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl App {
    /// The window of its own a group is in, if it is in one.
    pub fn float_of(&self, pane: usize) -> Option<&Floating> {
        self.floating.iter().find(|f| f.layout.contains(pane))
    }

    /// The groups of the window a group is in, in their order.
    pub fn window_panes(&self, pane: usize) -> Vec<usize> {
        let mut v = Vec::new();
        match self.float_of(pane) {
            Some(f) => f.layout.panes(&mut v),
            None => self.layout.panes(&mut v),
        }
        v
    }

    /// The split of the window a group is in, to change.
    pub fn layout_with_mut(&mut self, pane: usize) -> &mut Layout {
        match self.floating.iter().position(|f| f.layout.contains(pane)) {
            Some(i) => &mut self.floating[i].layout,
            None => &mut self.layout,
        }
    }

    /// The split of a window by its id (0: the main window), to change.
    pub fn window_layout_mut(&mut self, id: u64) -> &mut Layout {
        match self.floating.iter().position(|f| f.id == id) {
            Some(i) => &mut self.floating[i].layout,
            None => &mut self.layout,
        }
    }

    /// The groups after `gone` shift down one, in every window's split.
    pub(crate) fn renumber_all(&mut self, gone: usize) {
        self.layout.renumber_after(gone);
        for f in &mut self.floating {
            f.layout.renumber_after(gone);
        }
    }

    pub fn is_floating(&self, pane: usize) -> bool {
        self.float_of(pane).is_some()
    }

    /// A first group of the main window.
    pub fn main_pane(&self) -> usize {
        self.pane_order().first().copied().unwrap_or(0)
    }

    /// A new window holding `tab`, and the focus in it.
    fn pop_out(&mut self, tab: Tab) -> u64 {
        let new = self.panes.len();
        self.panes.push(Pane {
            tabs: alloc::vec![tab],
            active: 0,
            stacked: false,
        });
        self.next_float += 1;
        let id = self.next_float;
        let n = self.floating.len() as f32;
        self.floating.push(Floating {
            id,
            layout: Layout::Pane(new),
            x: 120.0 + 30.0 * n,
            y: 120.0 + 30.0 * n,
            w: 800.0,
            h: 600.0,
        });
        self.focus_pane(new);
        self.save_workspace();
        id
    }

    /// "Open in new window": the tab in focus, again, in a window of its own.
    pub fn open_in_new_window(&mut self) {
        let path = self.active_path();
        let graph = matches!(self.view(), Some(View::Graph(g)) if g.local.is_none());
        self.pop_out(Tab::empty());
        if let Some(p) = path {
            self.open_file(&p, false);
        } else if graph {
            self.open_graph(None);
        }
        self.save_workspace();
    }

    /// A file opened straight into a window of its own (the file menu's
    /// "Open in new window", or Cmd+Option+Shift+click).
    pub fn open_path_in_new_window(&mut self, path: &str) {
        self.pop_out(Tab::empty());
        self.open_file(path, false);
        self.save_workspace();
    }

    /// "Move to new window": the tab in focus leaves its group for a window
    /// of its own.
    pub fn move_to_new_window(&mut self) {
        if self.tabs.len() <= 1
            && self.is_floating(self.pane)
            && self.window_panes(self.pane).len() <= 1
        {
            // Already alone in a window of its own.
            return;
        }
        self.save_active();
        let i = self.active.min(self.tabs.len().saturating_sub(1));
        if i >= self.tabs.len() {
            return;
        }
        let tab = self.tabs.remove(i);
        self.active = self.active.min(self.tabs.len().saturating_sub(1));
        if self.tabs.is_empty() {
            self.drop_pane_if_empty();
        }
        self.pop_out(tab);
    }

    /// A tab dragged out of every window: a window of its own for it, as
    /// the notes app does with a tab let go outside its windows.
    pub fn move_tab_to_new_window(&mut self, from_pane: usize, tab: usize) {
        self.focus_pane(from_pane);
        self.select_tab(tab);
        self.move_to_new_window();
    }

    /// The window `id` was closed: its tabs close with it.
    pub fn close_floating(&mut self, id: u64) {
        let Some(at) = self.floating.iter().position(|f| f.id == id) else {
            return;
        };
        let mut gone = Vec::new();
        self.floating[at].layout.panes(&mut gone);
        if gone.contains(&self.pane) {
            let main = self.main_pane();
            self.focus_pane(main);
        }
        self.save_all();
        for g in &gone {
            if let Some(p) = self.panes.get(*g) {
                for t in &p.tabs {
                    if let Some(path) = t.path() {
                        self.closed.push(String::from(path));
                    }
                }
            }
        }
        self.floating.remove(at);
        gone.sort_unstable();
        for g in gone.into_iter().rev() {
            self.remove_pane_slot(g);
        }
        self.save_workspace();
    }

    /// Take away group `gone`'s slot (not the one in focus): the groups
    /// after it shift down one, in the layout and the windows alike.
    pub(crate) fn remove_pane_slot(&mut self, gone: usize) {
        if gone >= self.panes.len() || gone == self.pane {
            return;
        }
        self.panes.remove(gone);
        self.renumber_all(gone);
        if self.pane > gone {
            self.pane -= 1;
        }
    }

    /// A window of its own moved or was resized.
    pub fn floating_moved(&mut self, id: u64, x: f32, y: f32, w: f32, h: f32) {
        if let Some(f) = self.floating.iter_mut().find(|f| f.id == id) {
            f.x = x;
            f.y = y;
            f.w = w;
            f.h = h;
        }
    }

    // ---- workspace.json -----------------------------------------------------------

    fn leaf(&self, n: &mut u64, t: &Tab) -> Option<Value> {
        let (kind, state) = match (&t.doc, &t.view) {
            (Some(d), _) => {
                let mut st = Value::obj();
                st.set("file", Value::s(&d.path));
                let reading = d.mode == crate::editor::Mode::Reading;
                st.set("mode", Value::s(if reading { "preview" } else { "source" }));
                st.set("source", Value::Bool(d.mode == crate::editor::Mode::Source));
                ("markdown", st)
            }
            (None, Some(v)) => {
                let mut st = Value::obj();
                let kind = match v {
                    View::Graph(g) => match &g.local {
                        Some(p) => {
                            // As the notes app keeps a local graph: its note
                            // and every control of its panel.
                            st.set("file", Value::s(p));
                            st.set("options", local_options(g));
                            "localgraph"
                        }
                        None => "graph",
                    },
                    View::Canvas(c) => {
                        st.set("file", Value::s(&c.path));
                        "canvas"
                    }
                    View::Base(b) => {
                        st.set("file", Value::s(&b.path));
                        "bases"
                    }
                    View::File(f) => {
                        st.set("file", Value::s(&f.path));
                        match f.kind {
                            crate::vault::FileKind::Image => "image",
                            crate::vault::FileKind::Pdf => "pdf",
                            crate::vault::FileKind::Media if is_sound(&f.path) => "audio",
                            crate::vault::FileKind::Media => "video",
                            _ => return None,
                        }
                    }
                    View::Recovery(_) | View::Side(_) => return None,
                };
                (kind, st)
            }
            (None, None) => ("empty", Value::obj()),
        };
        *n += 1;
        let mut state_box = Value::obj();
        state_box.set("type", Value::s(kind));
        state_box.set("state", state);
        if t.pinned {
            state_box.set("pinned", Value::Bool(true));
        }
        let mut leaf = Value::obj();
        leaf.set("id", Value::s(&node_id(*n)));
        leaf.set("type", Value::s("leaf"));
        if let Some(g) = t.group {
            // Linked tabs share a group, as the notes app writes it.
            leaf.set("group", Value::s(&node_id(1_000_000 + u64::from(g))));
        }
        leaf.set("state", state_box);
        Some(leaf)
    }

    fn tabs_node(&self, n: &mut u64, pane: usize) -> Value {
        let (tabs, active) = self.pane_tabs(pane);
        let mut kids = Vec::new();
        let mut current = 0;
        for (i, t) in tabs.iter().enumerate() {
            if let Some(l) = self.leaf(n, t) {
                if i == active {
                    current = kids.len();
                }
                kids.push(l);
            }
        }
        *n += 1;
        let mut v = Value::obj();
        v.set("id", Value::s(&node_id(*n)));
        v.set("type", Value::s("tabs"));
        if self.pane_stacked(pane) {
            v.set("stacked", Value::Bool(true));
        }
        if current > 0 {
            v.set("currentTab", Value::Num(current as f64));
        }
        v.set("children", Value::Arr(kids));
        v
    }

    fn split_node(&self, n: &mut u64, l: &Layout) -> Value {
        match l {
            Layout::Pane(p) => self.tabs_node(n, *p),
            Layout::Split { down, kids, sizes } => {
                let children: Vec<Value> = kids
                    .iter()
                    .zip(sizes.iter())
                    .map(|(k, s)| {
                        let mut c = self.split_node(n, k);
                        c.set("dimension", Value::Num((*s as f64 * 100.0).round()));
                        c
                    })
                    .collect();
                *n += 1;
                let mut v = Value::obj();
                v.set("id", Value::s(&node_id(*n)));
                v.set("type", Value::s("split"));
                v.set("children", Value::Arr(children));
                // The notes app's "vertical" split puts its children side by
                // side.
                v.set(
                    "direction",
                    Value::s(if *down { "horizontal" } else { "vertical" }),
                );
                v
            }
        }
    }

    /// The main window's groups and the windows of their own, written into
    /// the vault's workspace file. What else the file holds (the sidebars as
    /// the notes app left them) is kept.
    pub fn save_workspace(&mut self) {
        if self.vault.is_none() || self.restoring {
            return;
        }
        let mut n = 0u64;
        let mut main = match &self.layout {
            Layout::Pane(_) => {
                let tabs = self.split_node(&mut n, &self.layout);
                n += 1;
                let mut v = Value::obj();
                v.set("id", Value::s(&node_id(n)));
                v.set("type", Value::s("split"));
                v.set("children", Value::Arr(alloc::vec![tabs]));
                v.set("direction", Value::s("vertical"));
                v
            }
            l => self.split_node(&mut n, l),
        };
        let _ = &mut main;
        let windows: Vec<Value> = self
            .floating
            .iter()
            .map(|f| {
                // A window is a split of its own: its groups are its
                // children, side by side or one above the other.
                let root = self.split_node(&mut n, &f.layout);
                let (children, direction) = match &f.layout {
                    Layout::Pane(_) => (alloc::vec![root], "vertical"),
                    Layout::Split { down, .. } => (
                        root.get("children").map_or(&[][..], Value::arr).to_vec(),
                        if *down { "horizontal" } else { "vertical" },
                    ),
                };
                n += 1;
                let mut w = Value::obj();
                w.set("id", Value::s(&node_id(n)));
                w.set("type", Value::s("window"));
                w.set("children", Value::Arr(children));
                w.set("direction", Value::s(direction));
                w.set("x", Value::Num(f.x.round() as f64));
                w.set("y", Value::Num(f.y.round() as f64));
                w.set("width", Value::Num(f.w.round() as f64));
                w.set("height", Value::Num(f.h.round() as f64));
                w.set("maximize", Value::Bool(false));
                w.set("zoom", Value::Num(0.0));
                w
            })
            .collect();
        n += 1;
        let mut floating = Value::obj();
        floating.set("id", Value::s(&node_id(n)));
        floating.set("type", Value::s("split"));
        floating.set("children", Value::Arr(windows));
        let Some(v) = self.vault.as_mut() else {
            return;
        };
        let mut doc =
            v.fs.read(WORKSPACE)
                .ok()
                .and_then(|t| json::parse(&t))
                .filter(|d| matches!(d, Value::Obj(_)))
                .unwrap_or_else(Value::obj);
        doc.set("main", main);
        if self.floating.is_empty() {
            doc.remove("floating");
        } else {
            doc.set("floating", floating);
        }
        let text = json::pretty(&doc, "  ");
        if v.fs.read(WORKSPACE).ok().as_deref() != Some(text.as_str()) {
            let _ = v.fs.write(WORKSPACE, &text);
        }
    }

    /// Bring back the groups and windows the workspace file keeps.
    pub fn restore_workspace(&mut self) {
        let Some(doc) = self
            .vault
            .as_mut()
            .and_then(|v| v.fs.read(WORKSPACE).ok())
            .and_then(|t| json::parse(&t))
        else {
            return;
        };
        self.restoring = true;
        if let Some(main) = doc.get("main") {
            let mut first = true;
            if let Some(l) = self.restore_split(main, &mut first) {
                self.layout = l;
            }
        }
        let windows: Vec<Value> = doc
            .get("floating")
            .map(|f| f.get("children").map_or(&[][..], Value::arr).to_vec())
            .unwrap_or_default();
        let main = self.pane;
        for w in &windows {
            if w.get("children").map_or(&[][..], Value::arr).is_empty() {
                continue;
            }
            let new = self.panes.len();
            self.panes.push(Pane {
                tabs: Vec::new(),
                active: 0,
                stacked: false,
            });
            self.focus_pane(new);
            // The window node is a split: read it as one.
            let mut as_split = w.clone();
            as_split.set("type", Value::s("split"));
            let mut first = true;
            let layout = self
                .restore_split(&as_split, &mut first)
                .unwrap_or(Layout::Pane(new));
            self.next_float += 1;
            let num = |k: &str, d: f32| w.get_num(k).map_or(d, |v| v as f32);
            self.floating.push(Floating {
                id: self.next_float,
                layout,
                x: num("x", 120.0),
                y: num("y", 120.0),
                w: num("width", 800.0).max(200.0),
                h: num("height", 600.0).max(150.0),
            });
            let mut ps = Vec::new();
            if let Some(f) = self.floating.last() {
                f.layout.panes(&mut ps);
            }
            let nothing = ps.iter().all(|p| {
                self.pane_tabs(*p)
                    .0
                    .iter()
                    .all(|t| t.path().is_none() && t.view.is_none())
            });
            if nothing {
                // Nothing in it could be opened: no window for it.
                let id = self.next_float;
                self.restoring = false;
                self.close_floating(id);
                self.restoring = true;
            }
        }
        if main < self.panes.len() && !self.is_floating(main) {
            self.focus_pane(main);
        }
        self.next_group = self.next_group.max(1000 + self.group_ids.len() as u32);
        self.restoring = false;
    }

    fn restore_split(&mut self, node: &Value, first: &mut bool) -> Option<Layout> {
        match node.get_str("type")? {
            "tabs" => {
                let pane = if *first {
                    *first = false;
                    self.pane
                } else {
                    let new = self.panes.len();
                    self.panes.push(Pane {
                        tabs: Vec::new(),
                        active: 0,
                        stacked: false,
                    });
                    self.focus_pane(new);
                    new
                };
                self.restore_tabs(node);
                Some(Layout::Pane(pane))
            }
            "split" => {
                let mut kids = Vec::new();
                let mut sizes = Vec::new();
                for k in node.get("children").map_or(&[][..], Value::arr) {
                    if let Some(l) = self.restore_split(k, first) {
                        kids.push(l);
                        sizes.push(k.get_num("dimension").unwrap_or(0.0) as f32);
                    }
                }
                match kids.len() {
                    0 => None,
                    1 => kids.pop(),
                    n => {
                        let total: f32 = sizes.iter().sum();
                        let sizes = if total > 0.0 && sizes.iter().all(|s| *s > 0.0) {
                            sizes.iter().map(|s| s / total).collect()
                        } else {
                            alloc::vec![1.0 / n as f32; n]
                        };
                        Some(Layout::Split {
                            down: node.get_str("direction") == Some("horizontal"),
                            kids,
                            sizes,
                        })
                    }
                }
            }
            _ => None,
        }
    }

    /// Open a "tabs" node's leaves in the group in focus.
    fn restore_tabs(&mut self, node: &Value) {
        let current = node.get_num("currentTab").unwrap_or(0.0) as usize;
        self.stacked = node.get_bool("stacked").unwrap_or(false);
        let mut opened = 0;
        let mut active = 0;
        for (i, leaf) in node
            .get("children")
            .map_or(&[][..], Value::arr)
            .iter()
            .enumerate()
        {
            let Some(st) = leaf.get("state") else {
                continue;
            };
            let file = st.get("state").and_then(|s| s.get_str("file"));
            let exists = |me: &App, p: &str| me.vault.as_ref().is_some_and(|v| v.note(p).is_some());
            let before = self.tabs.len();
            match (st.get_str("type"), file) {
                (Some("graph"), _) => self.open_graph(None),
                (Some("localgraph"), Some(f)) if exists(self, f) => {
                    // Made in place, not beside: the layout is being read.
                    let opts = st.get("state").and_then(|s| s.get("options"));
                    let view = local_graph(f, opts);
                    let blank = self.tabs.len() == 1
                        && self.tabs[0].path().is_none()
                        && self.tabs[0].view.is_none();
                    if !blank {
                        self.new_tab();
                    }
                    if let Some(t) = self.tab_mut() {
                        t.view = Some(view);
                    }
                }
                (
                    Some("markdown" | "canvas" | "bases" | "pdf" | "image" | "video" | "audio"),
                    Some(f),
                ) if exists(self, f) => {
                    let blank = self.tabs.len() == 1
                        && self.tabs[0].path().is_none()
                        && self.tabs[0].view.is_none();
                    self.open_file(f, !blank);
                    if st.get("state").and_then(|s| s.get_str("mode")) == Some("preview") {
                        if let Some(d) = self.doc_mut() {
                            d.mode = crate::editor::Mode::Reading;
                        }
                    }
                }
                _ => continue,
            }
            if self.tabs.len() >= before {
                if st.get_bool("pinned") == Some(true) {
                    if let Some(t) = self.tab_mut() {
                        t.pinned = true;
                    }
                }
                if let Some(g) = leaf.get_str("group") {
                    let n = self
                        .group_ids
                        .iter()
                        .position(|x| x == g)
                        .unwrap_or_else(|| {
                            self.group_ids.push(String::from(g));
                            self.group_ids.len() - 1
                        });
                    if let Some(t) = self.tab_mut() {
                        t.group = Some(1000 + n as u32);
                    }
                }
                if i <= current {
                    active = self.active;
                }
                opened += 1;
            }
        }
        if opened > 0 {
            self.select_tab(active);
        }
    }
}

/// A local graph's panel as the notes app writes it in workspace.json.
fn local_options(g: &crate::app::GraphTab) -> Value {
    let mut o = Value::obj();
    o.set("collapse-filter", Value::Bool(!g.opt.open_filters));
    o.set("search", Value::s(&g.opt.search));
    o.set("localJumps", Value::Num(f64::from(g.depth)));
    o.set("localBacklinks", Value::Bool(g.opt.incoming));
    o.set("localForelinks", Value::Bool(g.opt.outgoing));
    o.set("localInterlinks", Value::Bool(g.opt.neighbors));
    let mut rest = Value::obj();
    g.opt.to_json(&mut rest);
    if let Value::Obj(kv) = rest {
        for (k, v) in kv {
            if k != "collapse-filter" && k != "search" && k != "showOrphans" {
                o.set(&k, v);
            }
        }
    }
    o.set("scale", Value::Num(f64::from(g.zoom)));
    o.set("close", Value::Bool(!g.controls));
    o
}

/// A local graph made from what workspace.json kept of it.
fn local_graph(file: &str, opts: Option<&Value>) -> View {
    let empty = Value::obj();
    let o = opts.unwrap_or(&empty);
    let mut opt = crate::graph::Options::from_json(o);
    opt.incoming = o.get_bool("localBacklinks").unwrap_or(true);
    opt.outgoing = o.get_bool("localForelinks").unwrap_or(true);
    opt.neighbors = o.get_bool("localInterlinks").unwrap_or(false);
    View::Graph(alloc::boxed::Box::new(crate::app::GraphTab {
        local: Some(String::from(file)),
        depth: o
            .get_num("localJumps")
            .map_or(1, |d| (d as u32).clamp(1, 5)),
        graph: None,
        opt,
        cam: (0.0, 0.0),
        zoom: o.get_num("scale").map_or(1.0, |z| z as f32),
        fitted: false,
        controls: !o.get_bool("close").unwrap_or(false),
        hover: None,
        stale: true,
        animate: None,
    }))
}

/// A sound file, played with the sound player rather than shown.
pub fn is_sound(path: &str) -> bool {
    matches!(
        crate::vault::ext_of(path).to_ascii_lowercase().as_str(),
        "mp3" | "wav" | "m4a" | "flac" | "ogg" | "3gp" | "oga" | "opus" | "aac"
    )
}

/// A node id as the notes app writes them: sixteen hex digits.
fn node_id(n: u64) -> String {
    format!(
        "{:016x}",
        n.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0x6b72_6174_6500_0000
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::Vault;
    use crate::vfs::{MemFs, Vfs};
    use alloc::boxed::Box;

    fn first_pane(f: &Floating) -> usize {
        let mut v = Vec::new();
        f.layout.panes(&mut v);
        v[0]
    }

    #[test]
    fn a_window_of_its_own_splits_and_keeps_its_split() {
        let mut a = app();
        a.move_to_new_window();
        a.split(true);
        a.open_file("Third.md", false);
        assert_eq!(a.floating.len(), 1);
        assert_eq!(a.window_panes(a.pane).len(), 2);
        // The main window is untouched.
        assert_eq!(a.pane_order().len(), 1);
        a.save_workspace();
        let text = a.vault.as_mut().unwrap().fs.read(WORKSPACE).unwrap();
        let doc = json::parse(&text).unwrap();
        let win = &doc.get("floating").unwrap().get("children").unwrap().arr()[0];
        assert_eq!(win.get_str("direction"), Some("horizontal"));
        assert_eq!(win.get("children").unwrap().arr().len(), 2);
        // Closing one of its groups keeps the window with the other.
        let i = a.active;
        a.close_tab(i);
        assert_eq!(a.floating.len(), 1);
        assert_eq!(a.window_panes(a.pane).len(), 1);
        assert_eq!(a.active_path().as_deref(), Some("Other.md"));
        // And it comes back split from the file.
        a.split(false);
        a.save_workspace();
        let fs = core::mem::replace(&mut a.vault.as_mut().unwrap().fs, Box::new(MemFs::new()));
        let mut b = App::new();
        b.open_vault(Vault::open("v", fs));
        assert_eq!(b.floating.len(), 1);
        let mut ps = Vec::new();
        b.floating[0].layout.panes(&mut ps);
        assert_eq!(ps.len(), 2);
        assert!(ps
            .iter()
            .all(|p| b.pane_tabs(*p).0[0].path() == Some("Other.md")));
    }

    #[test]
    fn a_tab_let_go_outside_the_windows_gets_a_window() {
        let mut a = app();
        let main = a.pane;
        a.move_tab_to_new_window(main, 0);
        assert_eq!(a.floating.len(), 1);
        assert_eq!(a.active_path().as_deref(), Some("Home.md"));
    }

    #[test]
    fn a_local_graph_keeps_its_panel_and_its_link_in_the_workspace_file() {
        let mut a = app();
        a.open_graph(Some(String::from("Home.md")));
        if let Some(View::Graph(g)) = a.view_mut() {
            g.depth = 3;
            g.opt.neighbors = true;
            g.opt.show_tags = true;
            g.opt.repel = 12.5;
        }
        a.save_workspace();
        let text = a.vault.as_mut().unwrap().fs.read(WORKSPACE).unwrap();
        assert!(text.contains("\"localJumps\": 3"), "{text}");
        assert!(text.contains("\"localInterlinks\": true"));
        assert!(text.contains("\"group\""));
        let fs = core::mem::replace(&mut a.vault.as_mut().unwrap().fs, Box::new(MemFs::new()));
        let mut b = App::new();
        b.open_vault(Vault::open("v", fs));
        let g = b
            .all_tabs()
            .find_map(|t| match &t.view {
                Some(View::Graph(g)) => Some((
                    g.depth,
                    g.opt.neighbors,
                    g.opt.show_tags,
                    g.opt.repel,
                    t.group,
                )),
                _ => None,
            })
            .expect("the local graph came back");
        assert_eq!((g.0, g.1, g.2, g.3), (3, true, true, 12.5));
        // Linked again to the note it follows.
        let note_group = b
            .all_tabs()
            .find(|t| t.path() == Some("Other.md") && t.group.is_some())
            .and_then(|t| t.group);
        assert_eq!(g.4, note_group);
    }

    fn app() -> App {
        let mut fs = MemFs::new();
        fs.write("Home.md", "# Home\n").unwrap();
        fs.write("Other.md", "Other\n").unwrap();
        fs.write("Third.md", "Third\n").unwrap();
        let mut a = App::new();
        a.open_vault(Vault::open("v", Box::new(fs)));
        a.open_file("Home.md", false);
        a.open_file("Other.md", true);
        a
    }

    #[test]
    fn open_in_new_window_copies_the_tab() {
        let mut a = app();
        a.open_in_new_window();
        assert_eq!(a.floating.len(), 1);
        assert!(a.is_floating(a.pane));
        assert_eq!(a.active_path().as_deref(), Some("Other.md"));
        // The main window still has both tabs.
        let main = a.main_pane();
        assert_eq!(a.pane_tabs(main).0.len(), 2);
    }

    #[test]
    fn move_to_new_window_takes_the_tab() {
        let mut a = app();
        a.move_to_new_window();
        assert_eq!(a.floating.len(), 1);
        assert_eq!(a.active_path().as_deref(), Some("Other.md"));
        let main = a.main_pane();
        let left: Vec<_> = a.pane_tabs(main).0.iter().filter_map(Tab::path).collect();
        assert_eq!(left, alloc::vec!["Home.md"]);
    }

    #[test]
    fn closing_the_window_returns_nothing_to_the_main_window() {
        let mut a = app();
        a.move_to_new_window();
        let id = a.floating[0].id;
        a.close_floating(id);
        assert!(a.floating.is_empty());
        assert!(!a.is_floating(a.pane));
        let paths: Vec<_> = a.all_tabs().filter_map(Tab::path).collect();
        assert_eq!(paths, alloc::vec!["Home.md"]);
        assert_eq!(a.panes.len(), 1);
    }

    #[test]
    fn closing_the_last_tab_of_a_window_closes_it() {
        let mut a = app();
        a.move_to_new_window();
        let i = a.active;
        a.close_tab(i);
        assert!(a.floating.is_empty());
        assert_eq!(a.panes.len(), 1);
        assert_eq!(a.active_path().as_deref(), Some("Home.md"));
    }

    #[test]
    fn a_split_group_closing_keeps_the_windows_pointing_at_their_groups() {
        let mut a = app();
        a.split(false);
        a.open_file("Third.md", false);
        let split_pane = a.pane;
        a.move_to_new_window();
        // The split group was left empty and went; the window's group moved
        // down one and still holds Third.
        assert_eq!(a.floating.len(), 1);
        let f = first_pane(&a.floating[0]);
        assert!(f < a.panes.len());
        assert_eq!(a.pane_tabs(f).0[0].path(), Some("Third.md"));
        let _ = split_pane;
    }

    #[test]
    fn the_workspace_file_keeps_main_and_floating_and_brings_them_back() {
        let mut a = app();
        a.move_to_new_window();
        a.save_workspace();
        let text = a.vault.as_mut().unwrap().fs.read(WORKSPACE).unwrap();
        let doc = json::parse(&text).unwrap();
        let win = &doc.get("floating").unwrap().get("children").unwrap().arr()[0];
        assert_eq!(win.get_str("type"), Some("window"));
        let leaf = &win.get("children").unwrap().arr()[0]
            .get("children")
            .unwrap()
            .arr()[0];
        let file = leaf
            .get("state")
            .unwrap()
            .get("state")
            .unwrap()
            .get_str("file");
        assert_eq!(file, Some("Other.md"));
        assert!(doc.get("main").is_some());
        // A sidebar the notes app wrote stays as it was.
        let mut d2 = doc.clone();
        d2.set("left", Value::s("kept"));
        a.vault
            .as_mut()
            .unwrap()
            .fs
            .write(WORKSPACE, &json::pretty(&d2, "  "))
            .unwrap();
        a.save_workspace();
        let text = a.vault.as_mut().unwrap().fs.read(WORKSPACE).unwrap();
        assert!(text.contains("\"kept\""));

        // Open the vault again: the window comes back with its tab.
        let fs = core::mem::replace(
            &mut a.vault.as_mut().unwrap().fs,
            Box::new(MemFs::default()),
        );
        let mut b = App::new();
        // Opening the vault brings the workspace back.
        b.open_vault(Vault::open("v", fs));
        assert_eq!(b.floating.len(), 1);
        let f = first_pane(&b.floating[0]);
        assert_eq!(b.pane_tabs(f).0[0].path(), Some("Other.md"));
        assert!(!b.is_floating(b.pane));
        assert_eq!(b.active_path().as_deref(), Some("Home.md"));
    }
}
