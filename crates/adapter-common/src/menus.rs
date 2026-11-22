//! Menus and tray icons (IC-1002, IC-1003): the parts every host shares.
//!
//! An app hands Krate a flat list of items -- each naming its parent -- and
//! this module turns it into a checked tree: ids unique, parents real
//! submenus that came first, shortcuts that parse, a bounded size. Every
//! host builds its native menu from the same tree, so a menu that is valid
//! on a Mac is valid on Windows and Linux too.
//!
//! What the person chooses comes back through one process-wide queue of
//! [`MenuSignal`]s: a Mac menu fires on the main thread, a Linux tray on its
//! D-Bus thread, and the runtime drains them all in its event loop and hands
//! the app an ordinary `action(id)` event.

use std::collections::{BTreeSet, VecDeque};
use std::sync::Mutex;

use crate::ui::Modifiers;

/// The most items one menu (bar, popup or tray) may hold.
pub const MAX_ITEMS: usize = 2000;
/// How deep submenus may nest.
pub const MAX_DEPTH: usize = 8;
/// The longest label, in characters.
pub const MAX_LABEL: usize = 200;

/// What kind of item a menu entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    /// An ordinary command.
    Normal,
    /// A command with a checkmark that turns on and off.
    Check,
    /// One of a group of choices; next-door radio items in one menu are a
    /// group and exactly one of them is marked.
    Radio,
    /// A line between groups of items.
    Separator,
    /// A menu of its own; items name it as their parent.
    Submenu,
}

/// A standard item that does the system's own thing, in the system's words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Not a standard item: choosing it sends the app its id.
    None,
    /// Undo the last change.
    Undo,
    /// Redo what was undone.
    Redo,
    /// Cut the selection.
    Cut,
    /// Copy the selection.
    Copy,
    /// Paste.
    Paste,
    /// Select everything.
    SelectAll,
    /// Quit the app.
    Quit,
    /// About this app.
    About,
    /// Hide the app (a Mac); minimize elsewhere.
    Hide,
    /// Minimize the window.
    Minimize,
    /// Close the window, as its close button does.
    CloseWindow,
}

impl Role {
    /// Whether this role is an edit command a text field or drawn editor
    /// answers.
    pub fn is_edit(self) -> bool {
        matches!(
            self,
            Role::Undo | Role::Redo | Role::Cut | Role::Copy | Role::Paste | Role::SelectAll
        )
    }

    /// The system's own words for the role, with the app's name where a
    /// Mac uses it.
    pub fn label(self, app: &str) -> String {
        match self {
            Role::None => String::new(),
            Role::Undo => "Undo".into(),
            Role::Redo => "Redo".into(),
            Role::Cut => "Cut".into(),
            Role::Copy => "Copy".into(),
            Role::Paste => "Paste".into(),
            Role::SelectAll => "Select All".into(),
            Role::Quit => format!("Quit {app}").trim().to_string(),
            Role::About => format!("About {app}").trim().to_string(),
            Role::Hide => format!("Hide {app}").trim().to_string(),
            Role::Minimize => "Minimize".into(),
            Role::CloseWindow => "Close Window".into(),
        }
    }

    /// The system's shortcut for the role, as a shortcut string.
    pub fn shortcut(self) -> &'static str {
        match self {
            Role::Undo => "CmdOrCtrl+Z",
            Role::Redo => "CmdOrCtrl+Shift+Z",
            Role::Cut => "CmdOrCtrl+X",
            Role::Copy => "CmdOrCtrl+C",
            Role::Paste => "CmdOrCtrl+V",
            Role::SelectAll => "CmdOrCtrl+A",
            Role::Quit => "CmdOrCtrl+Q",
            Role::Hide => "Cmd+H",
            Role::Minimize => "Cmd+M",
            Role::CloseWindow => "CmdOrCtrl+W",
            Role::None | Role::About => "",
        }
    }

    /// The key an edit role presses in a drawn editor (with the primary
    /// modifier, and Shift for redo).
    pub fn edit_key(self) -> Option<(&'static str, bool)> {
        match self {
            Role::Undo => Some(("z", false)),
            Role::Redo => Some(("z", true)),
            Role::Cut => Some(("x", false)),
            Role::Copy => Some(("c", false)),
            Role::Paste => Some(("v", false)),
            Role::SelectAll => Some(("a", false)),
            _ => None,
        }
    }
}

/// One entry as the app gave it.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuItem {
    /// The id the `action` event carries; 0 for a separator or a role item
    /// the app does not need to hear about.
    pub id: u64,
    /// The submenu this item sits in; 0 for the top level.
    pub parent: u64,
    /// What kind of item it is.
    pub kind: ItemKind,
    /// What the person reads. A role item may leave it empty.
    pub label: String,
    /// Whether it can be chosen.
    pub enabled: bool,
    /// Whether a check or radio item is marked.
    pub checked: bool,
    /// A shortcut such as `CmdOrCtrl+Shift+S`, or empty.
    pub shortcut: String,
    /// A standard item, or `Role::None`.
    pub role: Role,
}

impl MenuItem {
    /// An ordinary command, enabled, with no shortcut.
    pub fn command(id: u64, parent: u64, label: &str) -> Self {
        Self {
            id,
            parent,
            kind: ItemKind::Normal,
            label: label.to_string(),
            enabled: true,
            checked: false,
            shortcut: String::new(),
            role: Role::None,
        }
    }
}

/// A parsed keyboard shortcut.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shortcut {
    /// Command on a Mac, Ctrl elsewhere (`CmdOrCtrl`).
    pub primary: bool,
    /// Command on a Mac; the Windows key elsewhere (`Cmd`).
    pub command: bool,
    /// Control everywhere (`Ctrl`).
    pub control: bool,
    /// Option on a Mac, Alt elsewhere.
    pub alt: bool,
    /// Shift.
    pub shift: bool,
    /// The key, as Krate names keys: `s`, `5`, `,`, `F5`, `Enter`,
    /// `ArrowUp`.
    pub key: String,
}

/// Keys a shortcut may name besides one printable character.
const NAMED_KEYS: &[(&str, &str)] = &[
    ("enter", "Enter"),
    ("return", "Enter"),
    ("escape", "Escape"),
    ("esc", "Escape"),
    ("tab", "Tab"),
    ("space", "Space"),
    ("backspace", "Backspace"),
    ("delete", "Delete"),
    ("del", "Delete"),
    ("insert", "Insert"),
    ("home", "Home"),
    ("end", "End"),
    ("pageup", "PageUp"),
    ("pagedown", "PageDown"),
    ("up", "ArrowUp"),
    ("arrowup", "ArrowUp"),
    ("down", "ArrowDown"),
    ("arrowdown", "ArrowDown"),
    ("left", "ArrowLeft"),
    ("arrowleft", "ArrowLeft"),
    ("right", "ArrowRight"),
    ("arrowright", "ArrowRight"),
    ("plus", "+"),
];

/// Printable characters a shortcut may name, besides letters and digits.
const PUNCTUATION: &str = ",./;'[]-=`\\";

impl Shortcut {
    /// Read `CmdOrCtrl+Shift+S`-style text. Modifiers in any order and any
    /// case; the key comes last. A printable key needs a modifier other
    /// than Shift, or typing that letter would choose the item.
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("an empty shortcut".to_string());
        }
        let parts: Vec<&str> = text.split('+').map(str::trim).collect();
        // "CmdOrCtrl++" names the plus key.
        let (mods, key) = if text.ends_with("++") {
            (&parts[..parts.len() - 2], "+")
        } else {
            let (key, mods) = parts.split_last().ok_or("an empty shortcut")?;
            (mods, *key)
        };
        let mut s = Shortcut {
            primary: false,
            command: false,
            control: false,
            alt: false,
            shift: false,
            key: String::new(),
        };
        for m in mods {
            let flag = match m.to_ascii_lowercase().as_str() {
                "cmdorctrl" | "commandorcontrol" | "primary" | "mod" => &mut s.primary,
                "cmd" | "command" | "meta" | "super" | "win" => &mut s.command,
                "ctrl" | "control" => &mut s.control,
                "alt" | "option" | "opt" => &mut s.alt,
                "shift" => &mut s.shift,
                "" => return Err(format!("`{text}` has an empty part")),
                other => {
                    return Err(format!(
                        "`{other}` is not a modifier (use CmdOrCtrl, Cmd, Ctrl, Alt or Shift)"
                    ))
                }
            };
            if *flag {
                return Err(format!("`{text}` names a modifier twice"));
            }
            *flag = true;
        }
        s.key = Self::key_name(key)
            .ok_or_else(|| format!("`{key}` is not a key a shortcut can use"))?;
        let printable = s.key.chars().count() == 1;
        if printable && !(s.primary || s.command || s.control || s.alt) {
            return Err(format!(
                "`{text}` would fire while the person types: add CmdOrCtrl, Ctrl or Alt"
            ));
        }
        Ok(s)
    }

    fn key_name(key: &str) -> Option<String> {
        let mut chars = key.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            if c.is_ascii_alphanumeric() {
                return Some(c.to_ascii_lowercase().to_string());
            }
            if PUNCTUATION.contains(c) || c == '+' {
                return Some(c.to_string());
            }
            return None;
        }
        let lower = key.to_ascii_lowercase();
        if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
            if (1..=24).contains(&n) {
                return Some(format!("F{n}"));
            }
            return None;
        }
        NAMED_KEYS
            .iter()
            .find(|(name, _)| *name == lower)
            .map(|(_, canonical)| canonical.to_string())
    }

    /// The modifiers as they are on this system: (Command or the Windows
    /// key, Control, Alt, Shift).
    pub fn resolved(&self, mac: bool) -> Modifiers {
        Modifiers {
            meta: self.command || (self.primary && mac),
            control: self.control || (self.primary && !mac),
            alt: self.alt,
            shift: self.shift,
        }
    }

    /// Whether a key press is this shortcut. Letters compare without case,
    /// since Shift is a modifier of its own.
    pub fn matches(&self, key: &str, modifiers: Modifiers, mac: bool) -> bool {
        let want = self.resolved(mac);
        let same_key = if self.key.chars().count() == 1 {
            key.eq_ignore_ascii_case(&self.key)
        } else {
            key == self.key || (self.key == "Space" && key == " ")
        };
        same_key
            && want.meta == modifiers.meta
            && want.control == modifiers.control
            && want.alt == modifiers.alt
            && want.shift == modifiers.shift
    }

    /// How Windows and Linux write it beside the label: `Ctrl+Shift+S`.
    pub fn display(&self) -> String {
        let m = self.resolved(false);
        let mut out = String::new();
        for (on, name) in [
            (m.control, "Ctrl"),
            (m.meta, "Win"),
            (m.alt, "Alt"),
            (m.shift, "Shift"),
        ] {
            if on {
                out.push_str(name);
                out.push('+');
            }
        }
        let key = match self.key.as_str() {
            "ArrowUp" => "Up".to_string(),
            "ArrowDown" => "Down".to_string(),
            "ArrowLeft" => "Left".to_string(),
            "ArrowRight" => "Right".to_string(),
            k if k.chars().count() == 1 => k.to_ascii_uppercase(),
            k => k.to_string(),
        };
        out.push_str(&key);
        out
    }

    /// The key equivalent an `NSMenuItem` takes: the character AppKit
    /// matches, lowercase (Shift is in the mask).
    pub fn mac_key_equivalent(&self) -> String {
        let code = |c: u32| char::from_u32(c).map(String::from).unwrap_or_default();
        match self.key.as_str() {
            "Enter" => "\r".into(),
            "Escape" => "\u{1b}".into(),
            "Tab" => "\t".into(),
            "Space" => " ".into(),
            "Backspace" => "\u{8}".into(),
            "Delete" => code(0xF728),
            "Insert" => code(0xF727),
            "Home" => code(0xF729),
            "End" => code(0xF72B),
            "PageUp" => code(0xF72C),
            "PageDown" => code(0xF72D),
            "ArrowUp" => code(0xF700),
            "ArrowDown" => code(0xF701),
            "ArrowLeft" => code(0xF702),
            "ArrowRight" => code(0xF703),
            k => match k.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()) {
                Some(n) => code(0xF704 + n - 1),
                None => k.to_string(),
            },
        }
    }
}

/// One item with everything checked, and what sits under it.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuNode {
    /// The item as given, its label filled in for a role.
    pub item: MenuItem,
    /// Its shortcut, parsed; a role's own when the app gave none.
    pub shortcut: Option<Shortcut>,
    /// A submenu's items.
    pub children: Vec<MenuNode>,
}

/// A whole menu, checked.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MenuTree {
    /// The top-level items: a bar's menus, or a popup's items.
    pub roots: Vec<MenuNode>,
}

impl MenuTree {
    /// Check a flat list and make it a tree. `app` names the app in role
    /// labels ("Quit Notes").
    pub fn build(items: &[MenuItem], app: &str) -> Result<Self, String> {
        if items.len() > MAX_ITEMS {
            return Err(format!(
                "a menu holds at most {MAX_ITEMS} items; this one has {}",
                items.len()
            ));
        }
        let mut ids = BTreeSet::new();
        // Submenus seen so far, and how deep each sits.
        let mut submenus: Vec<(u64, usize)> = Vec::new();
        let mut nodes: Vec<(usize, MenuNode)> = Vec::with_capacity(items.len());
        for (at, item) in items.iter().enumerate() {
            let place = || format!("item {at} (\"{}\")", item.label);
            if item.label.chars().count() > MAX_LABEL {
                return Err(format!(
                    "{}: a label is at most {MAX_LABEL} characters",
                    place()
                ));
            }
            let needs_id = !matches!(item.kind, ItemKind::Separator) && item.role == Role::None;
            if item.id == 0 && needs_id {
                return Err(format!("{}: needs an id that is not 0", place()));
            }
            if item.id != 0 && !ids.insert(item.id) {
                return Err(format!("{}: id {} is used twice", place(), item.id));
            }
            let depth = if item.parent == 0 {
                0
            } else {
                match submenus.iter().find(|(id, _)| *id == item.parent) {
                    Some((_, d)) => d + 1,
                    None => {
                        return Err(format!(
                            "{}: parent {} is not a submenu listed before it",
                            place(),
                            item.parent
                        ))
                    }
                }
            };
            if depth >= MAX_DEPTH {
                return Err(format!(
                    "{}: submenus nest at most {MAX_DEPTH} deep",
                    place()
                ));
            }
            let mut item = item.clone();
            if item.kind == ItemKind::Submenu {
                if item.id == 0 {
                    return Err(format!(
                        "{}: a submenu needs an id its items can name",
                        place()
                    ));
                }
                submenus.push((item.id, depth));
            }
            if item.label.trim().is_empty() && item.kind != ItemKind::Separator {
                if item.role == Role::None {
                    return Err(format!("item {at}: needs a label"));
                }
                item.label = item.role.label(app);
            }
            let shortcut = if !item.shortcut.trim().is_empty() {
                Some(Shortcut::parse(&item.shortcut).map_err(|why| format!("{}: {why}", place()))?)
            } else if !item.role.shortcut().is_empty() {
                Shortcut::parse(item.role.shortcut()).ok()
            } else {
                None
            };
            if shortcut.is_some() && matches!(item.kind, ItemKind::Separator | ItemKind::Submenu) {
                return Err(format!(
                    "{}: a separator or submenu has no shortcut",
                    place()
                ));
            }
            nodes.push((
                at,
                MenuNode {
                    item,
                    shortcut,
                    children: Vec::new(),
                },
            ));
        }
        // Assemble from the bottom up: a child always comes after its parent,
        // so walking backwards moves each finished node into its parent.
        let mut finished: Vec<Option<MenuNode>> = nodes.into_iter().map(|(_, n)| Some(n)).collect();
        for i in (0..finished.len()).rev() {
            let parent = finished[i].as_ref().map(|n| n.item.parent).unwrap_or(0);
            if parent == 0 {
                continue;
            }
            let node = finished[i].take();
            if let Some(p) = finished[..i]
                .iter_mut()
                .rev()
                .flatten()
                .find(|n| n.item.id == parent && n.item.kind == ItemKind::Submenu)
            {
                if let Some(node) = node {
                    p.children.insert(0, node);
                }
            }
        }
        let mut tree = MenuTree {
            roots: finished.into_iter().flatten().collect(),
        };
        tree.normalize_radios();
        Ok(tree)
    }

    /// Every group of radio items has exactly one marked: the first marked,
    /// or the first when none is.
    fn normalize_radios(&mut self) {
        fn walk(nodes: &mut [MenuNode]) {
            let mut i = 0;
            while i < nodes.len() {
                if nodes[i].item.kind == ItemKind::Radio {
                    let start = i;
                    while i < nodes.len() && nodes[i].item.kind == ItemKind::Radio {
                        i += 1;
                    }
                    let group = &mut nodes[start..i];
                    let marked = group.iter().position(|n| n.item.checked).unwrap_or(0);
                    for (k, n) in group.iter_mut().enumerate() {
                        n.item.checked = k == marked;
                    }
                } else {
                    i += 1;
                }
            }
            for n in nodes.iter_mut() {
                walk(&mut n.children);
            }
        }
        walk(&mut self.roots);
    }

    /// Every node, depth first.
    pub fn walk(&self) -> Vec<&MenuNode> {
        fn go<'a>(nodes: &'a [MenuNode], out: &mut Vec<&'a MenuNode>) {
            for n in nodes {
                out.push(n);
                go(&n.children, out);
            }
        }
        let mut out = Vec::new();
        go(&self.roots, &mut out);
        out
    }

    /// The node with this id.
    pub fn find(&self, id: u64) -> Option<&MenuNode> {
        if id == 0 {
            return None;
        }
        self.walk().into_iter().find(|n| n.item.id == id)
    }

    fn find_mut(&mut self, id: u64) -> Option<&mut MenuNode> {
        fn go(nodes: &mut [MenuNode], id: u64) -> Option<&mut MenuNode> {
            for n in nodes {
                if n.item.id == id {
                    return Some(n);
                }
                if let Some(found) = go(&mut n.children, id) {
                    return Some(found);
                }
            }
            None
        }
        if id == 0 {
            return None;
        }
        go(&mut self.roots, id)
    }

    /// The person chose `id`: a check item flips, a radio item becomes its
    /// group's one mark. The item, after the change; None when there is no
    /// such item or it cannot be chosen.
    pub fn choose(&mut self, id: u64) -> Option<MenuItem> {
        let kind = {
            let node = self.find(id)?;
            if !node.item.enabled
                || matches!(node.item.kind, ItemKind::Separator | ItemKind::Submenu)
            {
                return None;
            }
            node.item.kind
        };
        match kind {
            ItemKind::Check => {
                let node = self.find_mut(id)?;
                node.item.checked = !node.item.checked;
            }
            ItemKind::Radio => {
                fn mark(nodes: &mut [MenuNode], id: u64) -> bool {
                    if let Some(at) = nodes.iter().position(|n| n.item.id == id) {
                        let (mut start, mut end) = (at, at);
                        while start > 0 && nodes[start - 1].item.kind == ItemKind::Radio {
                            start -= 1;
                        }
                        while end + 1 < nodes.len() && nodes[end + 1].item.kind == ItemKind::Radio {
                            end += 1;
                        }
                        for n in &mut nodes[start..=end] {
                            n.item.checked = n.item.id == id;
                        }
                        return true;
                    }
                    nodes.iter_mut().any(|n| mark(&mut n.children, id))
                }
                mark(&mut self.roots, id);
            }
            _ => {}
        }
        self.find(id).map(|n| n.item.clone())
    }

    /// Turn an item on or off; false when there is no such item.
    pub fn set_enabled(&mut self, id: u64, enabled: bool) -> bool {
        match self.find_mut(id) {
            Some(n) => {
                n.item.enabled = enabled;
                true
            }
            None => false,
        }
    }

    /// Mark or unmark a check or radio item (a radio item marked clears its
    /// group); false when there is no such item.
    pub fn set_checked(&mut self, id: u64, checked: bool) -> bool {
        let Some(kind) = self.find(id).map(|n| n.item.kind) else {
            return false;
        };
        if kind == ItemKind::Radio && checked {
            let enabled = self.find(id).is_some_and(|n| n.item.enabled);
            self.set_enabled(id, true);
            self.choose(id);
            self.set_enabled(id, enabled);
        } else if let Some(n) = self.find_mut(id) {
            n.item.checked = checked;
        }
        true
    }

    /// The enabled item whose shortcut this key press is.
    pub fn match_key(&self, key: &str, modifiers: Modifiers, mac: bool) -> Option<&MenuNode> {
        self.walk().into_iter().find(|n| {
            n.item.enabled
                && n.shortcut
                    .as_ref()
                    .is_some_and(|s| s.matches(key, modifiers, mac))
        })
    }
}

/// What came back from a menu or a tray icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuSignal {
    /// The person chose the item with this id (from the bar, a popup or a
    /// tray's menu).
    Chosen(u64),
    /// The person chose a standard item the runtime carries out.
    Role(Role),
    /// The person clicked a tray icon: which one, and whether with the
    /// right button.
    TrayClick {
        /// The tray icon's id.
        tray: u64,
        /// The right (secondary) button.
        right: bool,
    },
}

/// The most signals kept before the oldest are dropped: a person cannot
/// choose 256 items between two looks for input.
const MAX_SIGNALS: usize = 256;

static SIGNALS: Mutex<VecDeque<MenuSignal>> = Mutex::new(VecDeque::new());

/// Hand a signal to the runtime. Any thread.
pub fn push_signal(signal: MenuSignal) {
    if let Ok(mut q) = SIGNALS.lock() {
        if q.len() >= MAX_SIGNALS {
            q.pop_front();
        }
        q.push_back(signal);
    }
}

/// Everything signalled since the last call, oldest first.
pub fn take_signals() -> Vec<MenuSignal> {
    SIGNALS
        .lock()
        .map(|mut q| q.drain(..).collect())
        .unwrap_or_default()
}

/// An icon for the tray, drawn by the app.
#[derive(Debug, Clone, PartialEq)]
pub struct TrayIcon {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Straight (not premultiplied) RGBA, row by row from the top.
    pub rgba: Vec<u8>,
    /// On a Mac: draw it as a template (its shape only, tinted to match
    /// the menu bar in light and dark mode).
    pub template: bool,
}

/// The largest tray icon side, in pixels.
pub const MAX_TRAY_ICON: u32 = 256;
/// The longest tooltip, in characters (Windows keeps 127).
pub const MAX_TOOLTIP: usize = 127;
/// The longest title beside a Mac's tray icon, in characters.
pub const MAX_TRAY_TITLE: usize = 64;
/// How many tray icons one app may show.
pub const MAX_TRAY_ICONS: usize = 4;

impl TrayIcon {
    /// Check the size and the pixel count.
    pub fn validate(&self) -> Result<(), String> {
        if self.width == 0
            || self.height == 0
            || self.width > MAX_TRAY_ICON
            || self.height > MAX_TRAY_ICON
        {
            return Err(format!(
                "a tray icon is 1 to {MAX_TRAY_ICON} pixels a side; this one is {}x{}",
                self.width, self.height
            ));
        }
        let want = self.width as usize * self.height as usize * 4;
        if self.rgba.len() != want {
            return Err(format!(
                "a {}x{} icon is {want} bytes of RGBA; this one is {}",
                self.width,
                self.height,
                self.rgba.len()
            ));
        }
        Ok(())
    }
}

/// Cut text to at most `max` characters.
pub fn clip(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: u64, parent: u64, kind: ItemKind, label: &str) -> MenuItem {
        MenuItem {
            kind,
            ..MenuItem::command(id, parent, label)
        }
    }

    fn mods(meta: bool, control: bool, alt: bool, shift: bool) -> Modifiers {
        Modifiers {
            meta,
            control,
            alt,
            shift,
        }
    }

    #[test]
    fn a_flat_list_becomes_the_tree_it_names() {
        let items = vec![
            item(1, 0, ItemKind::Submenu, "File"),
            item(2, 1, ItemKind::Normal, "New"),
            item(3, 1, ItemKind::Submenu, "Recent"),
            item(4, 3, ItemKind::Normal, "a.txt"),
            item(0, 1, ItemKind::Separator, ""),
            item(5, 1, ItemKind::Normal, "Close"),
            item(6, 0, ItemKind::Submenu, "View"),
            item(7, 6, ItemKind::Check, "Sidebar"),
        ];
        let tree = MenuTree::build(&items, "Notes").unwrap();
        assert_eq!(tree.roots.len(), 2);
        let file = &tree.roots[0];
        let labels: Vec<_> = file
            .children
            .iter()
            .map(|n| n.item.label.as_str())
            .collect();
        assert_eq!(labels, ["New", "Recent", "", "Close"]);
        assert_eq!(file.children[1].children[0].item.label, "a.txt");
        assert_eq!(tree.roots[1].children[0].item.kind, ItemKind::Check);
        assert_eq!(tree.walk().len(), 8);
    }

    #[test]
    fn broken_lists_are_refused_with_the_item_named() {
        let cases: Vec<(Vec<MenuItem>, &str)> = vec![
            (vec![item(0, 0, ItemKind::Normal, "Open")], "not 0"),
            (
                vec![
                    item(1, 0, ItemKind::Normal, "A"),
                    item(1, 0, ItemKind::Normal, "B"),
                ],
                "used twice",
            ),
            (
                vec![item(2, 9, ItemKind::Normal, "Orphan")],
                "not a submenu",
            ),
            (
                // A parent that is not a submenu.
                vec![
                    item(1, 0, ItemKind::Normal, "A"),
                    item(2, 1, ItemKind::Normal, "B"),
                ],
                "not a submenu",
            ),
            (
                // A parent listed after its child.
                vec![
                    item(2, 1, ItemKind::Normal, "B"),
                    item(1, 0, ItemKind::Submenu, "A"),
                ],
                "not a submenu",
            ),
            (vec![item(3, 0, ItemKind::Normal, "")], "needs a label"),
            (
                vec![item(3, 0, ItemKind::Normal, &"x".repeat(201))],
                "at most 200",
            ),
        ];
        for (items, why) in cases {
            let err = MenuTree::build(&items, "App").unwrap_err();
            assert!(err.contains(why), "{err} should say {why}");
        }
        let too_many: Vec<_> = (1..=2001)
            .map(|i| item(i, 0, ItemKind::Normal, "x"))
            .collect();
        assert!(MenuTree::build(&too_many, "App").is_err());
        let mut deep = vec![item(1, 0, ItemKind::Submenu, "1")];
        for i in 2..=9 {
            deep.push(item(i, i - 1, ItemKind::Submenu, "n"));
        }
        assert!(MenuTree::build(&deep, "App").unwrap_err().contains("nest"));
    }

    #[test]
    fn roles_get_the_system_words_and_shortcuts() {
        let mut copy = item(0, 0, ItemKind::Normal, "");
        copy.role = Role::Copy;
        let mut quit = item(0, 0, ItemKind::Normal, "");
        quit.role = Role::Quit;
        let tree = MenuTree::build(&[copy, quit], "Notes").unwrap();
        assert_eq!(tree.roots[0].item.label, "Copy");
        assert_eq!(tree.roots[0].shortcut.as_ref().unwrap().key, "c");
        assert!(tree.roots[0].shortcut.as_ref().unwrap().primary);
        assert_eq!(tree.roots[1].item.label, "Quit Notes");
    }

    #[test]
    fn shortcuts_parse_in_any_order_and_case() {
        let s = Shortcut::parse("shift+CmdOrCtrl+S").unwrap();
        assert!(s.primary && s.shift && !s.alt);
        assert_eq!(s.key, "s");
        assert_eq!(Shortcut::parse("Alt+F4").unwrap().key, "F4");
        assert_eq!(Shortcut::parse("F5").unwrap().key, "F5");
        assert_eq!(Shortcut::parse("Ctrl+Up").unwrap().key, "ArrowUp");
        assert_eq!(Shortcut::parse("CmdOrCtrl++").unwrap().key, "+");
        assert_eq!(Shortcut::parse("CmdOrCtrl+,").unwrap().key, ",");
        assert_eq!(Shortcut::parse("Delete").unwrap().key, "Delete");
        for bad in [
            "",
            "S",
            "Shift+S",
            "Ctrl+Ctrl+S",
            "Hyper+S",
            "Ctrl+",
            "Ctrl+F25",
            "Ctrl+é",
            "Ctrl+Banana",
        ] {
            assert!(Shortcut::parse(bad).is_err(), "{bad} should be refused");
        }
    }

    #[test]
    fn cmd_or_ctrl_is_command_on_a_mac_and_control_elsewhere() {
        let s = Shortcut::parse("CmdOrCtrl+Shift+S").unwrap();
        assert!(s.matches("S", mods(true, false, false, true), true));
        assert!(!s.matches("s", mods(false, true, false, true), true));
        assert!(s.matches("s", mods(false, true, false, true), false));
        assert!(!s.matches("s", mods(false, true, false, false), false));
        assert!(!s.matches("d", mods(false, true, false, true), false));
        assert_eq!(s.display(), "Ctrl+Shift+S");
        assert_eq!(Shortcut::parse("Alt+Left").unwrap().display(), "Alt+Left");
        let space = Shortcut::parse("Ctrl+Space").unwrap();
        assert!(space.matches("Space", mods(false, true, false, false), false));
    }

    #[test]
    fn mac_key_equivalents_are_appkits_characters() {
        assert_eq!(Shortcut::parse("Cmd+S").unwrap().mac_key_equivalent(), "s");
        assert_eq!(
            Shortcut::parse("Cmd+F1").unwrap().mac_key_equivalent(),
            "\u{F704}"
        );
        assert_eq!(
            Shortcut::parse("Cmd+Up").unwrap().mac_key_equivalent(),
            "\u{F700}"
        );
        assert_eq!(
            Shortcut::parse("Cmd+Enter").unwrap().mac_key_equivalent(),
            "\r"
        );
    }

    #[test]
    fn checks_flip_and_radios_keep_one_mark() {
        let mut a = item(1, 0, ItemKind::Radio, "Small");
        a.checked = true;
        let mut b = item(2, 0, ItemKind::Radio, "Large");
        b.checked = true;
        let c = item(3, 0, ItemKind::Radio, "Huge");
        let sep = item(0, 0, ItemKind::Separator, "");
        let d = item(4, 0, ItemKind::Radio, "Other group");
        let e = item(5, 0, ItemKind::Check, "Wrap");
        let mut off = item(6, 0, ItemKind::Normal, "Off");
        off.enabled = false;
        let mut tree = MenuTree::build(&[a, b, c, sep, d, e, off], "App").unwrap();
        // Two were marked: the first keeps it.
        let marks: Vec<_> = tree.roots.iter().map(|n| n.item.checked).collect();
        assert_eq!(marks, [true, false, false, false, true, false, false]);
        tree.choose(3);
        let marks: Vec<_> = tree.roots.iter().map(|n| n.item.checked).collect();
        assert_eq!(marks, [false, false, true, false, true, false, false]);
        assert!(tree.choose(5).unwrap().checked);
        assert!(!tree.choose(5).unwrap().checked);
        assert!(tree.choose(6).is_none(), "a disabled item cannot be chosen");
        assert!(tree.choose(99).is_none());
        tree.set_checked(1, true);
        assert!(tree.find(1).unwrap().item.checked && !tree.find(3).unwrap().item.checked);
        assert!(tree.set_enabled(6, true));
        assert!(tree.choose(6).is_some());
    }

    #[test]
    fn a_key_press_finds_the_enabled_item_it_is_the_shortcut_of() {
        let mut save = item(1, 0, ItemKind::Normal, "Save");
        save.shortcut = "CmdOrCtrl+S".into();
        let mut print = item(2, 0, ItemKind::Normal, "Print");
        print.shortcut = "CmdOrCtrl+P".into();
        print.enabled = false;
        let tree = MenuTree::build(&[save, print], "App").unwrap();
        let ctrl = mods(false, true, false, false);
        assert_eq!(tree.match_key("s", ctrl, false).unwrap().item.id, 1);
        assert!(tree.match_key("p", ctrl, false).is_none());
        assert!(tree.match_key("s", Modifiers::default(), false).is_none());
    }

    #[test]
    fn signals_queue_in_order_and_drop_the_oldest_past_the_cap() {
        let _ = take_signals();
        for i in 0..300 {
            push_signal(MenuSignal::Chosen(i));
        }
        let got = take_signals();
        assert_eq!(got.len(), MAX_SIGNALS);
        assert_eq!(got[0], MenuSignal::Chosen(300 - MAX_SIGNALS as u64));
        assert!(take_signals().is_empty());
    }

    #[test]
    fn tray_icons_are_checked_for_size_and_bytes() {
        let ok = TrayIcon {
            width: 2,
            height: 2,
            rgba: vec![0; 16],
            template: true,
        };
        assert!(ok.validate().is_ok());
        assert!(TrayIcon {
            rgba: vec![0; 15],
            ..ok.clone()
        }
        .validate()
        .is_err());
        assert!(TrayIcon {
            width: 0,
            ..ok.clone()
        }
        .validate()
        .is_err());
        assert!(TrayIcon {
            width: 257,
            rgba: vec![0; 257 * 2 * 4],
            ..ok
        }
        .validate()
        .is_err());
    }
}
