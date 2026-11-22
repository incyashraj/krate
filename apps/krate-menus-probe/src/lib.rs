// Menus Probe: every kind of menu item (krate:ui/menus, IC-1002) in one
// window -- a menu bar with submenus, separators, a check item, a radio
// group, a disabled item, shortcuts and the standard Edit and Quit items; a
// right-click menu; and a "More" button whose menu opens under it. The
// window says what was chosen last and how the marks stand, and every
// choice is printed as `chosen:<id> <label>`.
//
//   krate run menus-probe.krate -- quick   set the bar and both menus, print
//                                          `menus:bar=ok` (or why not) and exit
#![no_std]
extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use krate::gfx::{canvas2d, types as gfx};
use krate::layout::{Look, Rect, Said, Screen};
use krate::ui::menus::{self, Item, ItemKind, Role};
use krate::{
    io::{args, stdio},
    ui::{
        events, tree,
        types::{self, Event},
        window,
    },
    Guest,
};

const FILE: u64 = 100;
const NEW: u64 = 101;
const OPEN: u64 = 102;
const SAVE: u64 = 103;
const PRINT: u64 = 104;
const RECENT: u64 = 110;
const RECENT_A: u64 = 111;
const RECENT_B: u64 = 112;
const EDIT: u64 = 120;
const FIND: u64 = 121;
const VIEW: u64 = 130;
const SIDEBAR: u64 = 131;
const SMALL: u64 = 132;
const MEDIUM: u64 = 133;
const LARGE: u64 = 134;
const ZOOM: u64 = 140;
const ZOOM_IN: u64 = 141;
const ZOOM_OUT: u64 = 142;
const HELP: u64 = 150;
const WEBSITE: u64 = 151;
const RENAME: u64 = 201;
const DELETE: u64 = 202;
const DUPLICATE: u64 = 301;
const ARCHIVE: u64 = 302;

fn item(id: u64, parent: u64, kind: ItemKind, label: &str, shortcut: &str) -> Item {
    Item {
        id,
        parent,
        kind,
        label: String::from(label),
        enabled: true,
        checked: false,
        shortcut: String::from(shortcut),
        role: Role::None,
    }
}

fn role(parent: u64, role: Role) -> Item {
    Item { role, ..item(0, parent, ItemKind::Normal, "", "") }
}

fn sep(parent: u64) -> Item {
    item(0, parent, ItemKind::Separator, "", "")
}

fn bar() -> Vec<Item> {
    let mut print = item(PRINT, FILE, ItemKind::Normal, "Print...", "CmdOrCtrl+P");
    print.enabled = false;
    let mut sidebar = item(SIDEBAR, VIEW, ItemKind::Check, "Show Sidebar", "CmdOrCtrl+Shift+B");
    sidebar.checked = true;
    let mut medium = item(MEDIUM, VIEW, ItemKind::Radio, "Medium", "CmdOrCtrl+2");
    medium.checked = true;
    alloc::vec![
        item(FILE, 0, ItemKind::Submenu, "File", ""),
        item(NEW, FILE, ItemKind::Normal, "New Note", "CmdOrCtrl+N"),
        item(OPEN, FILE, ItemKind::Normal, "Open...", "CmdOrCtrl+O"),
        item(RECENT, FILE, ItemKind::Submenu, "Open Recent", ""),
        item(RECENT_A, RECENT, ItemKind::Normal, "groceries.md", ""),
        item(RECENT_B, RECENT, ItemKind::Normal, "trip plan.md", ""),
        sep(FILE),
        item(SAVE, FILE, ItemKind::Normal, "Save", "CmdOrCtrl+S"),
        print,
        sep(FILE),
        role(FILE, Role::Quit),
        item(EDIT, 0, ItemKind::Submenu, "Edit", ""),
        role(EDIT, Role::Undo),
        role(EDIT, Role::Redo),
        sep(EDIT),
        role(EDIT, Role::Cut),
        role(EDIT, Role::Copy),
        role(EDIT, Role::Paste),
        role(EDIT, Role::SelectAll),
        sep(EDIT),
        item(FIND, EDIT, ItemKind::Normal, "Find...", "CmdOrCtrl+F"),
        item(VIEW, 0, ItemKind::Submenu, "View", ""),
        sidebar,
        sep(VIEW),
        item(SMALL, VIEW, ItemKind::Radio, "Small", "CmdOrCtrl+1"),
        medium,
        item(LARGE, VIEW, ItemKind::Radio, "Large", "CmdOrCtrl+3"),
        sep(VIEW),
        item(ZOOM, VIEW, ItemKind::Submenu, "Zoom", ""),
        item(ZOOM_IN, ZOOM, ItemKind::Normal, "Zoom In", "CmdOrCtrl+="),
        item(ZOOM_OUT, ZOOM, ItemKind::Normal, "Zoom Out", "CmdOrCtrl+-"),
        item(HELP, 0, ItemKind::Submenu, "Help", ""),
        role(HELP, Role::About),
        item(WEBSITE, HELP, ItemKind::Normal, "Krate Website", "F1"),
    ]
}

fn context() -> Vec<Item> {
    alloc::vec![
        role(0, Role::Cut),
        role(0, Role::Copy),
        role(0, Role::Paste),
        sep(0),
        item(RENAME, 0, ItemKind::Normal, "Rename...", ""),
        item(DELETE, 0, ItemKind::Normal, "Delete Note", ""),
    ]
}

fn more() -> Vec<Item> {
    alloc::vec![
        item(DUPLICATE, 0, ItemKind::Normal, "Duplicate", ""),
        item(ARCHIVE, 0, ItemKind::Normal, "Archive", ""),
    ]
}

fn label(id: u64) -> &'static str {
    match id {
        NEW => "New Note",
        OPEN => "Open...",
        SAVE => "Save",
        PRINT => "Print...",
        RECENT_A => "groceries.md",
        RECENT_B => "trip plan.md",
        FIND => "Find...",
        SIDEBAR => "Show Sidebar",
        SMALL => "Small",
        MEDIUM => "Medium",
        LARGE => "Large",
        ZOOM_IN => "Zoom In",
        ZOOM_OUT => "Zoom Out",
        WEBSITE => "Krate Website",
        RENAME => "Rename...",
        DELETE => "Delete Note",
        DUPLICATE => "Duplicate",
        ARCHIVE => "Archive",
        _ => "?",
    }
}

fn style(weight: u16) -> gfx::TextStyle {
    gfx::TextStyle { weight, italic: false, letter_spacing: 0.0, family: gfx::FontFamily::Sans }
}

fn say(line: &str) {
    let out = stdio::stdout();
    let _ = out.write(line.as_bytes());
    let _ = out.write(b"\n");
}

fn node(id: u64, parent: Option<u64>, kind: types::WidgetKind) -> types::WidgetNode {
    types::WidgetNode {
        id,
        parent,
        kind,
        label: None,
        role: None,
        style: types::Style {
            width: None,
            height: None,
            grow: 1.0,
            padding: 0.0,
            text: None,
            place: None,
            box_: None,
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

struct State {
    last: String,
    keys: String,
    point: String,
}

fn more_button(screen: &Screen) -> Rect {
    // Bottom right, where it follows the window as it grows.
    let area = screen.safe_area().inset(20.0, 24.0, 20.0, 24.0);
    Rect::new(area.x + area.w - 160.0, area.y + area.h - 40.0, 160.0, 40.0)
}

fn draw(win: u64, canvas: u64, screen: &Screen, s: &State) -> Result<(), gfx::GfxError> {
    let look: Look = screen.look;
    let colors = look.palette();
    let area = screen.safe_area().inset(20.0, 24.0, 20.0, 24.0);
    canvas2d::clear(canvas, colors.background)?;
    let text = |t: &str, y: f32, size: f32, weight: u16, color| {
        canvas2d::draw_text_styled(
            canvas,
            t,
            gfx::Point { x: area.x, y: area.y + y },
            look.text(size),
            color,
            style(weight),
        )
    };
    text("Menus Probe", 30.0, 26.0, 800, colors.ink)?;
    text("Use the menu bar, its shortcuts, or right-click anywhere.", 60.0, 14.0, 500, colors.muted)?;
    text(&format!("Last chosen: {}", s.last), 110.0, 18.0, 700, colors.ink)?;
    let sidebar = if menus::checked(SIDEBAR) { "on" } else { "off" };
    let size = if menus::checked(SMALL) {
        "Small"
    } else if menus::checked(LARGE) {
        "Large"
    } else {
        "Medium"
    };
    text(&format!("Show Sidebar: {sidebar}    Text size: {size}"), 145.0, 16.0, 500, colors.ink)?;
    text(&format!("Keys the app saw: {}", s.keys), 180.0, 14.0, 500, colors.muted)?;
    text(&s.point, 210.0, 14.0, 500, colors.muted)?;
    // A status strip across the whole width, above the button.
    let strip = gfx::Rect { x: area.x, y: area.y + area.h - 56.0, width: area.w, height: 1.0 };
    canvas2d::fill_rect(canvas, strip, colors.line)?;
    let b = more_button(screen);
    let radii = gfx::CornerRadii { top_left: 10.0, top_right: 10.0, bottom_right: 10.0, bottom_left: 10.0 };
    canvas2d::fill_round_rect(canvas, gfx::Rect { x: b.x, y: b.y, width: b.w, height: b.h }, radii, colors.accent)?;
    canvas2d::draw_text_styled(
        canvas,
        "More...",
        gfx::Point { x: b.x + 52.0, y: b.y + 26.0 },
        look.text(15.0),
        colors.on_accent,
        style(600),
    )?;
    canvas2d::present(canvas)?;
    let mut said = Said::new();
    said.heading(Rect::new(area.x, area.y, area.w, 40.0), "Menus Probe");
    said.text(Rect::new(area.x, area.y + 90.0, area.w, 24.0), &format!("Last chosen: {}", s.last));
    said.button(b, "More...");
    said.send(win);
    Ok(())
}

struct Component;

impl Guest for Component {
    fn run() -> i32 {
        let Ok(win) = window::create("Menus Probe", types::WindowSize { width: 560, height: 380 }) else {
            return 1;
        };
        let bar_set = menus::set_bar(win, &bar());
        let context_set = menus::set_context(win, &context());
        if args::raw().split('\n').any(|a| a == "quick") {
            say(&match bar_set {
                Ok(()) => String::from("menus:bar=ok"),
                Err(e) => format!("menus:bar=failed {e:?}"),
            });
            say(&match context_set {
                Ok(()) => String::from("menus:context=ok"),
                Err(e) => format!("menus:context=failed {e:?}"),
            });
            // A shortcut that would fire while typing is refused, item named.
            let mut bad = bar();
            bad.push(item(999, FILE, ItemKind::Normal, "Typing trap", "S"));
            say(&match menus::set_bar(win, &bad) {
                Err(e) => format!("menus:bad-shortcut=refused {e:?}"),
                Ok(()) => String::from("menus:bad-shortcut=allowed"),
            });
            let _ = menus::set_bar(win, &bar());
            say(&format!("menus:sidebar-checked={}", menus::checked(SIDEBAR)));
            return 0;
        }
        if tree::set_root(win, &node(1, None, types::WidgetKind::Stack)).is_err()
            || tree::upsert_node(win, &node(2, Some(1), types::WidgetKind::Canvas)).is_err()
        {
            return 1;
        }
        let Ok(canvas) = canvas2d::bind(win, 2) else {
            return 1;
        };
        let mut state = State {
            last: String::from(match bar_set {
                Ok(()) => "(nothing yet)",
                Err(_) => "(the menu bar was refused)",
            }),
            keys: String::new(),
            point: String::new(),
        };
        loop {
            let screen = Screen::of(win);
            if draw(win, canvas, &screen, &state).is_err() {
                return 1;
            }
            let mut next = events::wait(None);
            while let Some(event) = next {
                match event {
                    Event::CloseRequested(_) => return 0,
                    Event::Action(id) => {
                        state.last = format!("{} (id {id})", label(id));
                        if id == RENAME || id == DELETE {
                            if let Some((x, y)) = krate::ui::pointer::position(win) {
                                state.point = format!("Right-clicked at {x:.0}, {y:.0}");
                            }
                        }
                        say(&format!("chosen:{id} {}", label(id)));
                    }
                    Event::Key(k) if k.pressed => {
                        let mut chord = String::new();
                        if k.modifiers.meta {
                            chord.push_str("Cmd+");
                        }
                        if k.modifiers.control {
                            chord.push_str("Ctrl+");
                        }
                        if k.modifiers.shift {
                            chord.push_str("Shift+");
                        }
                        chord.push_str(&k.key);
                        say(&format!("key:{chord}"));
                        if state.keys.len() > 60 {
                            state.keys.clear();
                        }
                        state.keys.push_str(&chord);
                        state.keys.push(' ');
                    }
                    Event::Pointer(p) if p.button.is_some() && !p.pressed => {
                        if more_button(&screen).contains(p.x, p.y) {
                            let b = more_button(&screen);
                            let _ = menus::popup(win, b.x, b.y + b.h + 4.0, &more());
                        }
                    }
                    _ => {}
                }
                next = events::poll();
            }
        }
    }
}

krate::export!(Component);
