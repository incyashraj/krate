//! The guest: draws the [`App`] on one canvas and turns window events into
//! calls on it. Wasm only; everything it draws comes from the plain modules.
//!
//! The window is laid out the way the well-known notes app lays out its
//! own: the title bar is part of the window's content (the system's window
//! buttons sit over the top-left corner), a 300 px sidebar on each side
//! with its tab icons in that top row, tab groups in the middle, each with
//! a 40 px tab bar and a 40 px view header, and a small status bar in the
//! bottom-right corner.

mod basemenus;
mod boards;
mod carddoc;
mod disk;
mod editor;
mod look;
mod media;
mod pdftext;
mod slides;
mod views;
mod widgets;
mod windows;

use crate::app::{
    App, Cmd, Focus, Hit, HostRequest, LeftPane, ListKind, Mods, Overlay, RightPane, RowKind, View,
};
use crate::editor::Mode;
use crate::panes::{Area, Divider};
use crate::text::{count, grouped};
use crate::vault::Vault;
use crate::vfs::parent;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use editor::{LayCache, Shown, View as EdView};
use krate::bindings::krate::store::kv as store_kv;
use krate::gfx::canvas2d;
use krate::layout::{Look, Rect, Said};
#[allow(unused_imports)]
use krate::prelude::*;
use krate::ui::{events, pointer, tree, types, window};
use look::{fill, fit, icon, r, round, text, width, Icon, Sizes, Theme, DARK, LIGHT};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;
const START_W: u32 = 1024;
const START_H: u32 = 800;
const BLINK_MS: u64 = 600;
/// The room the system's window buttons take in the top-left corner.
const LIGHTS: f32 = 80.0;

/// A search result as drawn: name, folder, matches, and its lines with
/// where the match is in each.
type ShownResult = (String, String, usize, Vec<(String, (usize, usize))>);

/// Where a press landed, as the frame was drawn.
#[derive(Clone, PartialEq)]
enum Zone {
    App(Hit),
    Editor,
    LeftDivider,
    RightDivider,
    Thumb,
    Track,
    TreeRow(usize),
    Tree,
    /// A tab: its group and place.
    Tab(usize, usize),
    /// The body of a tab group (focuses it).
    Pane(usize),
    PaneDivider(usize),
    Graph(usize),
    GraphPanel,
    GraphSlider(u8),
    Canvas(usize),
    Settings,
    /// A PDF's or a player's control, by group.
    File(usize, media::FileCtl),
    /// A card on a base's board, by its row.
    BoardCard(usize),
    /// A row of a base menu that can be dragged: (0 sort keys, 1
    /// properties) and its place.
    PopRow(u8, usize),
}

#[derive(Clone, PartialEq)]
enum Drag {
    None,
    Select,
    Thumb {
        grab: f32,
    },
    Left,
    Right,
    Row {
        row: usize,
        x: f32,
        y: f32,
        moved: bool,
    },
    Tab {
        pane: usize,
        tab: usize,
        x: f32,
        y: f32,
        moved: bool,
    },
    Divider(usize),
    /// Panning a graph, or dragging one of its dots.
    Graph {
        x: f32,
        y: f32,
        node: Option<usize>,
        moved: bool,
    },
    Slider(u8),
    /// On a canvas: panning, moving a card, resizing it, or drawing an
    /// arrow from one of its sides.
    Canvas {
        x: f32,
        y: f32,
        what: views::CanvasDrag,
        moved: bool,
        before: String,
    },
    /// Dragging a player's scrubber, in a group.
    Scrub(usize),
    /// Selecting a PDF's text, in a group.
    PdfSel(usize),
    /// A base menu's row being carried up or down.
    PopRow {
        list: u8,
        from: usize,
        y: f32,
        moved: bool,
    },
    /// A board card being carried to another column.
    Card {
        row: usize,
        x: f32,
        y: f32,
        moved: bool,
    },
}

struct Ui {
    win: u64,
    c: u64,
    app: App,
    cache: LayCache,
    shown: Shown,
    hits: Vec<(Rect, Zone)>,
    said: Said,
    sz: Sizes,
    view: Option<EdView>,
    tree_area: Option<Rect>,
    search_area: Option<Rect>,
    right_area: Option<Rect>,
    list_area: Option<Rect>,
    settings_area: Option<Rect>,
    /// Each tab group's box and its tab strip, as drawn.
    pane_areas: Vec<(usize, Rect, Rect)>,
    dividers: Vec<Divider>,
    /// Each tab's box, by group and place, for dropping a dragged tab.
    tab_rects: Vec<(usize, usize, Rect)>,
    /// The graph or canvas body of each group as drawn.
    view_areas: Vec<(usize, Rect)>,
    row_h: f32,
    hover: Option<Zone>,
    drag: Drag,
    drag_mods: Mods,
    last_click: (u64, f32, f32, u8),
    last_input: u64,
    caret_xy: Option<(f32, f32)>,
    title: String,
    frames: u64,
    w: f32,
    h: f32,
    drop_text: Option<char>,
    paste_next: bool,
    trace: bool,
    /// On a phone, the sidebar shown over the note.
    sheet: bool,
    t0: u64,
    first_frame_done: bool,
    index_done_said: bool,
    graph_img: views::GraphImage,
    files: media::Files,
    /// Laying a frame out without showing it (to fit a view first).
    unseen: bool,
    /// Where the pointer is, as last reported.
    hover_point: Option<(f32, f32)>,
    /// The count the next result block shows, when not its line count.
    block_count: Option<usize>,
    /// Web pages being read for link cards: address and handle.
    fetches: Vec<(String, u64)>,
    /// The windows of their own (src/guest/windows.rs).
    extra: Vec<windows::WinState>,
    /// The window a menu or list was opened in: 0 the main window,
    /// otherwise a floating group's id.
    overlay_in: u64,
    /// The window whose state is swapped in now, the same way.
    in_window: u64,
    /// The base toolbar's menu buttons as drawn: which menu, left, bottom.
    base_anchors: Vec<(crate::basemenu::Panel, f32, f32)>,
    /// The open base menu's box, to close it on a press outside.
    base_pop: Option<Rect>,
    /// A board's columns as drawn, each with its group.
    board_cols: Vec<(Rect, usize)>,
    /// Canvas cards' text drawn with the note editor, by card: the vault
    /// generation, the text and its document (src/guest/carddoc.rs).
    card_docs: alloc::collections::BTreeMap<String, (u64, String, crate::editor::Doc)>,
    /// A presentation is filling the window (src/guest/slides.rs).
    slides_on: bool,
    /// A base menu's draggable rows as drawn, and the properties list.
    pop_rows: Vec<(Rect, usize)>,
    pop_props: Vec<String>,
}

fn ms() -> u64 {
    krate::time::monotonic_nanos() / 1_000_000
}

fn widget(id: u64, parent: Option<u64>, kind: types::WidgetKind) -> types::WidgetNode {
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

fn mods(m: types::Modifiers) -> Mods {
    Mods {
        shift: m.shift,
        primary: m.meta || m.control,
        alt: m.alt,
        ctrl: m.control,
    }
}

/// What a tab shows, as a number the drawing code switches on.
fn view_kind(tb: &crate::app::Tab) -> u8 {
    match &tb.view {
        Some(View::Graph(_)) => 1,
        Some(View::Canvas(_)) => 2,
        Some(View::Base(_)) => 3,
        Some(View::Recovery(_)) => 4,
        Some(View::File(_)) => 5,
        Some(View::Side(_)) => 6,
        None => 0,
    }
}

/// A tab's kind for its icon: as [`view_kind`], with a file told apart as
/// a picture (5), a PDF (7), a video (8), a sound (9) or other (10).
fn tab_kind(tb: &crate::app::Tab) -> u8 {
    match &tb.view {
        Some(View::File(f)) => match f.kind {
            crate::vault::FileKind::Image => 5,
            crate::vault::FileKind::Pdf => 7,
            crate::vault::FileKind::Media if media::is_sound(&f.path) => 9,
            crate::vault::FileKind::Media => 8,
            _ => 10,
        },
        // A base's tab shows its view's kind, as the notes app marks it.
        Some(View::Base(b)) => match b.base.views.get(b.view).map(|v| v.kind.as_str()) {
            Some("list") => 14,
            Some("kanban") => 15,
            Some("cards") => 16,
            _ => 3,
        },
        _ => view_kind(tb),
    }
}

fn area(rc: Rect) -> Area {
    Area::new(rc.x, rc.y, rc.w, rc.h)
}

fn rect(a: Area) -> Rect {
    r(a.x, a.y, a.w, a.h)
}

impl Ui {
    fn t(&self) -> &'static Theme {
        if self.app.dark {
            &DARK
        } else {
            &LIGHT
        }
    }

    fn put(&self, k: &str, v: &str) {
        let mut s = String::from(k);
        s.push(':');
        s.push_str(v);
        s.push('\n');
        let _ = krate::io::stdio::write(s.as_bytes());
    }

    fn mark(&self, k: &str) {
        if self.trace {
            self.put(
                k,
                &crate::text::num((ms().saturating_sub(self.t0)) as usize),
            );
        }
    }

    fn hit(&mut self, rc: Rect, z: Zone) {
        self.hits.push((rc, z));
    }

    fn button(&mut self, rc: Rect, label: &str, h: Hit) {
        self.said.button(rc, label);
        self.hit(rc, Zone::App(h));
    }

    /// A clickable icon in a 26 px box, with the hover square behind it.
    fn icon_button(&mut self, ic: Icon, cx: f32, cy: f32, label: &str, h: Hit, on: bool) {
        let t = self.t();
        let k = self.sz.scale;
        let b = r(cx - 13.0 * k, cy - 12.0 * k, 26.0 * k, 24.0 * k);
        let z = Zone::App(h.clone());
        if on || self.hovered(&z) {
            round(self.c, b, 4.0, t.hover);
        }
        let col = if on || self.hovered(&z) {
            t.text
        } else {
            t.muted
        };
        icon(self.c, ic, cx, cy, k, col);
        self.button(b, label, h);
    }

    fn zone_at(&self, x: f32, y: f32) -> Option<Zone> {
        self.hits
            .iter()
            .rev()
            .find(|(rc, _)| rc.contains(x, y))
            .map(|(_, z)| z.clone())
    }

    fn hovered(&self, z: &Zone) -> bool {
        self.hover.as_ref() == Some(z)
    }

    // ---- the frame ---------------------------------------------------------

    fn draw(&mut self) {
        // A graph or canvas seen for the first time is fitted to its view,
        // which needs the view's size: lay the frame out once unseen first.
        let unfitted = match self.app.view() {
            Some(View::Graph(g)) => !g.fitted && g.graph.is_some(),
            Some(View::Canvas(c)) => !c.fitted,
            _ => false,
        };
        if unfitted {
            self.unseen = true;
            self.draw_frame(false);
            self.unseen = false;
            self.after_draw();
        }
        self.draw_frame(true);
    }

    fn draw_frame(&mut self, show: bool) {
        self.files_sweep();
        let size = canvas2d::canvas_size(self.c).unwrap_or(krate::gfx::types::Size {
            width: START_W as f32,
            height: START_H as f32,
        });
        let (w, h) = (size.width.max(320.0), size.height.max(240.0));
        self.w = w;
        self.h = h;
        let look = Look::now();
        self.app.system_dark = look.dark;
        let font = self.app.settings.font_size();
        self.sz = Sizes::with_body(look.text_scale, font);
        let t = self.t();
        self.hits.clear();
        self.said = Said::new();
        self.caret_xy = None;
        self.tree_area = None;
        self.search_area = None;
        self.right_area = None;
        self.list_area = None;
        self.settings_area = None;
        self.pane_areas.clear();
        self.dividers.clear();
        self.tab_rects.clear();
        self.view_areas.clear();
        self.base_pop = None;
        self.board_cols.clear();
        self.pop_rows.clear();
        self.pop_props.clear();
        if self.app.is_floating(self.app.pane) {
            self.view = None;
            self.shown.area = None;
        }
        let _ = canvas2d::clear(
            self.c,
            if self.app.vault.is_none() {
                t.bg
            } else {
                t.side
            },
        );
        self.follow_slides();
        if self.showing_fullscreen() {
            self.draw_fullscreen(w, h);
        } else if self.app.slides.is_some() {
            self.draw_slides(w, h);
        } else if self.app.vault.is_none() {
            self.draw_welcome(w, h);
        } else {
            self.draw_workspace(w, h);
        }
        // The note in focus may be in a window of its own: its extras and
        // the menus opened there are drawn there.
        if !self.app.is_floating(self.app.pane) {
            self.draw_editor_extras(w, h);
        }
        if self.overlay_in == 0 {
            self.draw_overlay(w, h);
        }
        // A video filling the window covers everything (src/guest/widgets.rs).
        let t = self.t();
        widgets::draw_fullscreen(
            self.c,
            &self.cache.env,
            look::r(0.0, 0.0, w, h),
            t,
            &mut self.said,
        );
        widgets::say_media_menu(&self.cache.env, &mut self.said);
        if !show {
            return;
        }
        let _ = canvas2d::present(self.c);
        self.said.send(self.win);
        self.frames += 1;
        self.set_title();
        if !self.first_frame_done {
            self.first_frame_done = true;
            self.mark("t_first_frame_ms");
        }
    }

    fn set_title(&mut self) {
        let mut want = String::new();
        let main = if self.app.is_floating(self.app.pane) {
            self.app.main_pane()
        } else {
            self.app.pane
        };
        let (tabs, act) = self.app.pane_tabs(main);
        if let Some(tb) = tabs.get(act) {
            want.push_str(tb.title());
            want.push_str(" - ");
        }
        if let Some(v) = &self.app.vault {
            want.push_str(&v.name);
            want.push_str(" - ");
        }
        want.push_str("Vault");
        if want != self.title {
            let _ = window::set_title(self.win, &want);
            self.title = want;
        }
    }

    /// The vault picker: who the app is on the left, the ways in on the
    /// right, each a row with what it does and a button.
    fn draw_welcome(&mut self, w: f32, h: f32) {
        let t = self.t();
        let k = self.sz.scale;
        let narrow = w < 720.0;
        let left_w = if narrow {
            0.0
        } else {
            (w * 0.3).clamp(240.0, 340.0)
        };
        if left_w > 0.0 {
            fill(self.c, r(0.0, 0.0, left_w, h), t.side);
            fill(self.c, r(left_w - 1.0, 0.0, 1.0, h), t.border);
            let mx = 28.0 * k;
            let my = 60.0 * k;
            for i in 0..3 {
                let o = i as f32 * 5.0 * k;
                round(
                    self.c,
                    r(mx + o, my - o, 24.0 * k, 30.0 * k),
                    4.0,
                    if i == 2 { t.accent } else { t.border_hover },
                );
            }
            text(
                self.c,
                "Vault",
                mx + 46.0 * k,
                my + 22.0 * k,
                24.0 * k,
                700,
                t.text,
            );
            self.said
                .heading(r(mx + 46.0 * k, my, 160.0, 30.0 * k), "Vault");
            let lines = self.wrap(
                "A vault is a folder of plain Markdown notes. Vault reads and saves only inside the folder you pick, and only while it is open.",
                self.sz.ui,
                left_w - 56.0 * k,
            );
            let mut y = my + 70.0 * k;
            for l in &lines {
                text(self.c, l, mx, y, self.sz.ui, 400, t.muted);
                y += self.sz.ui * 1.55;
            }
            let ver = "Vault 0.1 \u{b7} made with Krate";
            text(self.c, ver, mx, h - 24.0 * k, self.sz.small, 400, t.faint);
            self.said
                .text(r(mx, h - 40.0 * k, left_w - mx, 20.0 * k), ver);
        }
        let area = r(left_w, 0.0, w - left_w, h);
        let col_w = (520.0 * k).min(area.w - 48.0);
        let x = area.x + (area.w - col_w) * 0.5;
        let row_h = 78.0 * k;
        let block_h = row_h * 3.0;
        let y0 = (area.h - block_h) * 0.45;
        let rows: [(&str, &str, &str, &str); 3] = [
            (
                "Create new vault",
                "Make a new vault in a folder you choose.",
                "Create",
                "Create new vault",
            ),
            (
                "Open folder as vault",
                "Choose a folder of Markdown files you already have.",
                "Open",
                "Open folder as vault",
            ),
            (
                "Explore the sample vault",
                "A few notes that show every feature, kept in memory.",
                "Explore",
                "Explore the sample vault",
            ),
        ];
        for (i, (title, desc, btn, label)) in rows.iter().enumerate() {
            let ry = y0 + i as f32 * row_h;
            if i > 0 {
                fill(self.c, r(x, ry, col_w, 1.0), t.border);
            }
            text(self.c, title, x, ry + 32.0 * k, 15.0 * k, 600, t.text);
            fit(
                self.c,
                desc,
                x,
                ry + 54.0 * k,
                self.sz.ui,
                400,
                t.muted,
                col_w - 130.0 * k,
            );
            let bw = 100.0 * k;
            let b = r(x + col_w - bw, ry + (row_h - 32.0 * k) * 0.5, bw, 32.0 * k);
            let z = Zone::App(Hit::Welcome(i as u8));
            let primary = i == 1;
            let fillc = if primary {
                t.accent
            } else if self.hovered(&z) {
                t.border_hover
            } else {
                t.button
            };
            round(self.c, b, 6.0, fillc);
            let bt = width(self.c, btn, self.sz.ui, 500);
            text(
                self.c,
                btn,
                b.x + (b.w - bt) * 0.5,
                b.y + b.h * 0.5 + self.sz.ui * 0.36,
                self.sz.ui,
                500,
                if primary { t.on_accent } else { t.text },
            );
            self.button(b, label, Hit::Welcome(i as u8));
        }
        if let Some((msg, _)) = self.app.status.clone() {
            let mw = width(self.c, &msg, self.sz.ui, 400);
            text(
                self.c,
                &msg,
                x,
                (y0 - 20.0).max(18.0),
                self.sz.ui,
                400,
                t.danger,
            );
            self.said.text(r(x, (y0 - 38.0).max(0.0), mw, 20.0), &msg);
        }
    }

    fn draw_workspace(&mut self, w: f32, h: f32) {
        let t = self.t();
        let sz = self.sz;
        let k = sz.scale;
        let head = sz.head;
        let phone = w < 600.0;
        let ribbon = if self.app.settings.show_ribbon() && !phone {
            sz.ribbon
        } else {
            0.0
        };
        let left_open = self.app.left_open && !phone;
        let right_open = self.app.right_open && w >= 760.0;
        let left_w = if left_open {
            self.app.left_w.clamp(200.0, (w * 0.4).max(200.0))
        } else {
            0.0
        };
        let right_w = if right_open {
            self.app.right_w.clamp(200.0, (w * 0.4).max(200.0))
        } else {
            0.0
        };
        // The ribbon, under the top row on the far left.
        if ribbon > 0.0 {
            fill(self.c, r(0.0, head, ribbon, h - head), t.side);
            fill(self.c, r(ribbon - 1.0, head, 1.0, h - head), t.border);
            let items: [(Icon, Cmd, &str, &str); 9] = [
                (
                    Icon::Switcher,
                    Cmd::QuickSwitcher,
                    "Open quick switcher",
                    "switcher",
                ),
                (Icon::Graph, Cmd::Graph, "Open graph view", "graph"),
                (Icon::Canvas, Cmd::NewCanvas, "Create new canvas", "canvas"),
                (
                    Icon::Calendar,
                    Cmd::DailyNote,
                    "Open today's daily note",
                    "daily-notes",
                ),
                (
                    Icon::Template,
                    Cmd::InsertTemplate,
                    "Insert template",
                    "templates",
                ),
                (
                    Icon::Terminal,
                    Cmd::Palette,
                    "Open command palette",
                    "command-palette",
                ),
                (Icon::Table, Cmd::NewBase, "Create new base", "bases"),
                (
                    Icon::Dice,
                    Cmd::RandomNote,
                    "Open random note",
                    "random-note",
                ),
                (
                    Icon::FilePlus,
                    Cmd::UniqueNote,
                    "Create new unique note",
                    "zk-prefixer",
                ),
            ];
            let mut y = head + 22.0 * k;
            for (ic, cmd, label, plugin) in items {
                if !self.app.settings.plugin_on(plugin) {
                    continue;
                }
                self.icon_button(ic, ribbon * 0.5, y, label, Hit::Ribbon(cmd), false);
                y += 34.0 * k;
            }
        }
        // The left sidebar.
        let lx = ribbon;
        if left_open {
            let a = r(lx, 0.0, left_w, h);
            self.draw_left(a, ribbon > 0.0);
            fill(self.c, r(lx + left_w - 1.0, 0.0, 1.0, h), t.border);
            self.hit(r(lx + left_w - 4.0, head, 8.0, h - head), Zone::LeftDivider);
        }
        // The top row's left end when the sidebar is closed: the window
        // buttons, then the sidebar toggle.
        let main_x = lx + left_w;
        let main_w = (w - main_x - right_w).max(160.0);
        let lead = if left_open {
            0.0
        } else {
            (LIGHTS + 44.0 * k).max(ribbon)
        };
        if !left_open && !phone {
            fill(self.c, r(0.0, 0.0, lead.max(ribbon), head), t.side);
            self.icon_button(
                Icon::SidebarLeft,
                LIGHTS + 22.0 * k,
                head * 0.5,
                "Expand",
                Hit::Ribbon(Cmd::ToggleLeft),
                false,
            );
        }
        let main = r(main_x, 0.0, main_w, h);
        if phone && self.sheet {
            self.draw_left(r(0.0, 0.0, w, h), false);
        } else {
            self.draw_panes(main, lead.max(if left_open { 0.0 } else { ribbon }));
        }
        // The right end of the top row: the tab list and the right sidebar's
        // toggle when it is closed.
        let top_right = main.right();
        if !right_open {
            self.icon_button(
                Icon::SidebarRight,
                top_right - 22.0 * k,
                head * 0.5,
                "Expand",
                Hit::Ribbon(Cmd::ToggleRight),
                false,
            );
        }
        if right_open {
            let rx = main_x + main_w;
            fill(self.c, r(rx, 0.0, 1.0, h), t.border);
            self.draw_right(r(rx + 1.0, 0.0, right_w - 1.0, h));
            self.hit(r(rx - 4.0, head, 8.0, h - head), Zone::RightDivider);
        }
        self.draw_status(w, h, main_x);
    }

    // ---- the left sidebar ------------------------------------------------------

    fn draw_left(&mut self, area: Rect, after_ribbon: bool) {
        let t = self.t();
        let sz = self.sz;
        let k = sz.scale;
        fill(self.c, area, t.side);
        let head = sz.head;
        let tabs: [(Icon, LeftPane, &str, &str); 3] = [
            (Icon::Folder, LeftPane::Files, "Files", "file-explorer"),
            (Icon::Search, LeftPane::Search, "Search", "global-search"),
            (
                Icon::Bookmark,
                LeftPane::Bookmarks,
                "Bookmarks",
                "bookmarks",
            ),
        ];
        let start = if after_ribbon {
            area.x + 22.0 * k
        } else {
            area.x + LIGHTS + 22.0 * k
        };
        let mut x = start;
        for (ic, pane, label, plugin) in tabs {
            if !self.app.settings.plugin_on(plugin) {
                continue;
            }
            let on = self.app.left == pane;
            self.icon_button(ic, x, head * 0.5, label, Hit::LeftTab(pane), on);
            x += 31.0 * k;
        }
        self.icon_button(
            Icon::SidebarLeft,
            area.right() - 22.0 * k,
            head * 0.5,
            "Collapse",
            Hit::Ribbon(Cmd::ToggleLeft),
            false,
        );
        fill(self.c, r(area.x, head - 1.0, area.w, 1.0), t.border);
        // The vault's name, help and settings at the bottom.
        let foot = 42.0 * k;
        let fy = area.bottom() - foot;
        fill(self.c, r(area.x, fy, area.w, 1.0), t.border);
        let name = self
            .app
            .vault
            .as_ref()
            .map(|v| v.name.clone())
            .unwrap_or_default();
        let vb = r(
            area.x + 10.0 * k,
            fy + 7.0 * k,
            (area.w * 0.6).min(200.0),
            foot - 14.0 * k,
        );
        let vz = Zone::App(Hit::VaultMenu);
        if self.hovered(&vz) {
            round(self.c, vb, 8.0, t.hover);
        }
        icon(
            self.c,
            Icon::ChevronsUpDown,
            area.x + 24.0 * k,
            fy + foot * 0.5,
            k,
            t.muted,
        );
        fit(
            self.c,
            &name,
            area.x + 40.0 * k,
            fy + foot * 0.5 + sz.ui * 0.36,
            sz.ui,
            600,
            t.text,
            vb.right() - area.x - 44.0 * k,
        );
        self.button(vb, &name, Hit::VaultMenu);
        self.icon_button(
            Icon::Help,
            area.right() - 50.0 * k,
            fy + foot * 0.5,
            "Help",
            Hit::Command(Cmd::Help),
            false,
        );
        self.icon_button(
            Icon::Gear,
            area.right() - 22.0 * k,
            fy + foot * 0.5,
            "Settings",
            Hit::Command(Cmd::Settings),
            false,
        );
        let body = r(area.x, head, area.w, fy - head);
        match self.app.left {
            LeftPane::Files => self.draw_files(body),
            LeftPane::Search => self.draw_search(body),
            LeftPane::Bookmarks => self.draw_bookmarks(body),
        }
    }

    /// A row of small icon buttons, centred, under a sidebar's top row.
    fn nav_buttons(&mut self, area: Rect, acts: &[(Icon, Hit, &str)]) {
        let k = self.sz.scale;
        let step = 30.0 * k;
        let total = acts.len() as f32 * step;
        let mut x = area.x + (area.w - total) * 0.5 + step * 0.5;
        let cy = area.y + 20.0 * k;
        for (ic, hit, label) in acts {
            self.icon_button(*ic, x, cy, label, hit.clone(), false);
            x += step;
        }
    }

    fn draw_files(&mut self, area: Rect) {
        let t = self.t();
        let sz = self.sz;
        let k = sz.scale;
        self.nav_buttons(
            area,
            &[
                (Icon::SquarePen, Hit::Command(Cmd::NewNote), "New note"),
                (Icon::FolderPlus, Hit::Command(Cmd::NewFolder), "New folder"),
                (
                    Icon::Sort,
                    Hit::Command(Cmd::CollapseAll),
                    "Change sort order",
                ),
                (
                    Icon::Reveal,
                    Hit::Command(Cmd::Reveal),
                    "Auto-reveal current file",
                ),
                (
                    Icon::ChevronsUpDown,
                    Hit::Command(Cmd::CollapseAll),
                    "Collapse all",
                ),
            ],
        );
        let top = area.y + 42.0 * k;
        let list = r(area.x, top, area.w, area.bottom() - top);
        self.tree_area = Some(list);
        self.hit(list, Zone::Tree);
        let row_h = sz.row;
        self.row_h = row_h;
        let rows = self.app.rows().to_vec();
        let content = rows.len() as f32 * row_h + 12.0;
        let max = (content - list.h).max(0.0);
        self.app.explorer.scroll = self.app.explorer.scroll.clamp(0.0, max);
        let scroll = self.app.explorer.scroll;
        let first = ((scroll / row_h) as usize).min(rows.len());
        let active_path = self.app.active_path();
        let selected = self.app.explorer.selected.clone();
        let rename = self.app.explorer.rename.clone();
        let drag_row = match self.drag {
            Drag::Row {
                row, moved: true, ..
            } => Some(row),
            _ => None,
        };
        let drop_row = if drag_row.is_some() {
            pointer::position(self.win).and_then(|(px, py)| self.zone_at_rows(list, px, py))
        } else {
            None
        };
        let _ = canvas2d::set_clip(self.c, list.x, list.y, list.w, list.h);
        let mut y = list.y + first as f32 * row_h - scroll;
        let names: Vec<(String, String, bool)> = {
            let v = self.app.vault.as_ref();
            rows.iter()
                .skip(first)
                .take(((list.h / row_h) as usize) + 2)
                .map(|row| match (row.kind, v) {
                    (RowKind::Folder(f), Some(v)) => v
                        .folders
                        .get(f)
                        .map_or((String::new(), String::new(), true), |fo| {
                            (fo.name.clone(), fo.path.clone(), true)
                        }),
                    (RowKind::Note(n), Some(v)) => v
                        .notes
                        .get(n)
                        .map_or((String::new(), String::new(), false), |no| {
                            (no.name.clone(), no.path.clone(), false)
                        }),
                    _ => (String::new(), String::new(), false),
                })
                .collect()
        };
        let indent = 17.0 * k;
        for (n, (name, path, is_folder)) in names.iter().enumerate() {
            let i = first + n;
            if y > list.bottom() {
                break;
            }
            let Some(row) = rows.get(i) else {
                break;
            };
            let depth = row.depth as f32;
            let rr = r(list.x + 12.0 * k, y + 1.0, list.w - 24.0 * k, row_h - 2.0);
            let z = Zone::TreeRow(i);
            let is_active = active_path.as_deref() == Some(path.as_str());
            let is_sel = selected.as_deref() == Some(path.as_str());
            let is_drop = drop_row == Some(i) && *is_folder;
            if is_active || is_drop {
                round(self.c, rr, 6.0, t.active);
            } else if self.hovered(&z) || (is_sel && self.app.focus == Focus::Tree) {
                round(self.c, rr, 6.0, t.hover);
            }
            // Indent guides.
            for d in 0..row.depth {
                fill(
                    self.c,
                    r(list.x + 24.0 * k + d as f32 * indent, y, 1.0, row_h),
                    look::alpha(t.text, 0.12),
                );
            }
            let tx = list.x + 36.0 * k + depth * indent;
            let base = y + row_h * 0.5 + sz.ui * 0.36;
            if *is_folder {
                let open = self.app.explorer.expanded.contains(path);
                look::chevron(self.c, tx - 12.0 * k, y + row_h * 0.5, open, k, t.faint);
            }
            let mut room = rr.right() - tx - 8.0 * k;
            // A canvas or a base says so on the right.
            let tag = if *is_folder {
                None
            } else if path.ends_with(".canvas") {
                Some("CANVAS")
            } else if path.ends_with(".base") {
                Some("BASE")
            } else {
                None
            };
            if let Some(tag) = tag {
                let tw = look::caps_width(self.c, tag, 10.0 * k);
                look::caps(
                    self.c,
                    tag,
                    rr.right() - 12.0 * k - tw,
                    base - 0.5,
                    10.0 * k,
                    t.faint,
                );
                room -= tw + 12.0 * k;
            }
            if rename.as_ref().is_some_and(|rn| rn.path == *path) {
                if let Some(rn) = rename.as_ref() {
                    let fbox = r(tx - 4.0, y + 2.0, rr.right() - tx, row_h - 4.0);
                    round(self.c, fbox, 4.0, t.field);
                    look::outline(self.c, fbox, 4.0, 1.0, t.accent);
                    self.draw_input(&rn.input, tx, base, sz.ui, t.text, fbox.right() - 4.0, true);
                    self.said.field(fbox, "Name", &rn.input.text);
                }
            } else {
                let col = if is_active || self.hovered(&z) {
                    t.text
                } else {
                    t.muted
                };
                fit(self.c, name, tx, base, sz.ui, 400, col, room);
                self.said.button(rr, name);
            }
            self.hit(rr, z);
            y += row_h;
        }
        let _ = canvas2d::clear_clip(self.c);
        if rows.is_empty() {
            let unreadable = self
                .app
                .vault
                .as_ref()
                .is_some_and(|v| !v.problems.is_empty());
            let msg = if unreadable {
                "This folder could not be read"
            } else {
                "No notes yet"
            };
            text(
                self.c,
                msg,
                list.x + 24.0,
                list.y + 20.0,
                sz.ui,
                400,
                t.faint,
            );
            self.said.text(r(list.x, list.y + 4.0, list.w, 22.0), msg);
        }
    }

    fn zone_at_rows(&self, list: Rect, px: f32, py: f32) -> Option<usize> {
        if !list.contains(px, py) {
            return None;
        }
        let i = ((py - list.y + self.app.explorer.scroll) / self.row_h.max(1.0)) as usize;
        (i < self.app.explorer.rows.len()).then_some(i)
    }

    /// One line of input text with its caret and selection.
    #[allow(clippy::too_many_arguments)]
    fn draw_input(
        &mut self,
        inp: &crate::app::Input,
        x: f32,
        base: f32,
        size: f32,
        col: look::Rgba,
        right: f32,
        caret: bool,
    ) {
        let t = self.t();
        let s = inp.text.as_str();
        let before = s.get(..inp.caret).unwrap_or("");
        let cx = x + width(self.c, before, size, 400);
        // Keep the caret in the box by sliding long text left.
        let shift = (cx - right + 2.0).max(0.0);
        let _ = canvas2d::set_clip(
            self.c,
            x - 2.0,
            base - size * 1.2,
            (right - x + 4.0).max(0.0),
            size * 1.8,
        );
        if let Some((a, b)) = inp.selection() {
            let xa = x + width(self.c, s.get(..a).unwrap_or(""), size, 400) - shift;
            let xb = x + width(self.c, s.get(..b).unwrap_or(""), size, 400) - shift;
            fill(
                self.c,
                r(xa, base - size * 0.95, xb - xa, size * 1.3),
                t.sel,
            );
        }
        text(self.c, s, x - shift, base, size, 400, col);
        if caret {
            fill(
                self.c,
                r(cx - shift, base - size * 0.95, 1.5, size * 1.25),
                t.text,
            );
        }
        let _ = canvas2d::clear_clip(self.c);
    }

    fn draw_search(&mut self, area: Rect) {
        let t = self.t();
        let sz = self.sz;
        let k = sz.scale;
        self.nav_buttons(
            area,
            &[
                (Icon::Filter, Hit::SearchBox, "Search settings"),
                (Icon::ChevronsUpDown, Hit::SearchBox, "Collapse results"),
                (
                    Icon::Bookmark,
                    Hit::Command(Cmd::BookmarkSearch),
                    "Bookmark current search",
                ),
            ],
        );
        let boxr = r(
            area.x + 12.0 * k,
            area.y + 44.0 * k,
            area.w - 24.0 * k,
            30.0 * k,
        );
        round(self.c, boxr, 5.0, t.field);
        let focused = self.app.focus == Focus::Search;
        look::outline(
            self.c,
            boxr,
            5.0,
            1.0,
            if focused { t.border_focus } else { t.border },
        );
        icon(
            self.c,
            Icon::Search,
            boxr.x + 14.0 * k,
            boxr.y + boxr.h * 0.5,
            k * 0.85,
            t.faint,
        );
        let base = boxr.y + boxr.h * 0.5 + sz.ui * 0.36;
        let inp = self.app.search.clone();
        if inp.text.is_empty() && !focused {
            text(
                self.c,
                "Search...",
                boxr.x + 28.0 * k,
                base,
                sz.ui,
                400,
                t.faint,
            );
        }
        self.draw_input(
            &inp,
            boxr.x + 28.0 * k,
            base,
            sz.ui,
            t.text,
            boxr.right() - 8.0,
            focused,
        );
        self.said.field(boxr, "Search", &inp.text);
        self.hit(boxr, Zone::App(Hit::SearchBox));
        let mut y = boxr.bottom() + 8.0;
        let Some(job) = self.app.job.as_ref() else {
            return;
        };
        let summary = if job.done {
            let mut s = count(job.notes_matched, "result", "results");
            s.push_str(" \u{b7} ");
            s.push_str(&count(job.matches, "match", "matches"));
            s
        } else {
            let mut s = String::from("Searching ");
            s.push_str(&grouped(job.scanned()));
            s.push_str(" of ");
            s.push_str(&grouped(job.total));
            s.push_str("\u{2026}  ");
            s.push_str(&count(job.notes_matched, "result", "results"));
            s
        };
        text(
            self.c,
            &summary,
            area.x + 16.0 * k,
            y + 14.0,
            sz.small,
            400,
            t.faint,
        );
        self.said.text(r(area.x, y, area.w, 20.0), &summary);
        y += 24.0;
        let list = r(area.x, y, area.w, area.bottom() - y);
        self.search_area = Some(list);
        let row_h = sz.row;
        let results: Vec<ShownResult> = job
            .results
            .iter()
            .map(|r| {
                (
                    r.name.clone(),
                    String::from(parent(&r.path)),
                    r.count,
                    r.lines.iter().map(|l| (l.text.clone(), l.at)).collect(),
                )
            })
            .collect();
        let paths: Vec<String> = job.results.iter().map(|r| r.path.clone()).collect();
        let line_h = (sz.small * 1.5 + 12.0).round();
        let mut total = 0.0;
        for (i, res) in results.iter().enumerate() {
            total += row_h;
            if !paths
                .get(i)
                .is_some_and(|p| self.app.search_collapsed.contains(p))
            {
                total += res.3.len() as f32 * (line_h + 4.0) + 6.0;
            }
        }
        let max = (total - list.h + 12.0).max(0.0);
        self.app.search_scroll = self.app.search_scroll.clamp(0.0, max);
        let _ = canvas2d::set_clip(self.c, list.x, list.y, list.w, list.h);
        let mut yy = list.y - self.app.search_scroll;
        for (i, (name, _folder, n, lines)) in results.iter().enumerate() {
            if yy > list.bottom() {
                break;
            }
            let collapsed = paths
                .get(i)
                .is_some_and(|p| self.app.search_collapsed.contains(p));
            let block_h = row_h
                + if collapsed {
                    0.0
                } else {
                    lines.len() as f32 * (line_h + 4.0) + 6.0
                };
            if yy + block_h < list.y {
                yy += block_h;
                continue;
            }
            let hr = r(list.x + 12.0 * k, yy, list.w - 24.0 * k, row_h);
            let z = Zone::App(Hit::SearchFile(i));
            if self.hovered(&z) {
                round(self.c, hr, 6.0, t.hover);
            }
            let base = yy + row_h * 0.5 + sz.ui * 0.36;
            look::chevron(
                self.c,
                hr.x + 12.0 * k,
                yy + row_h * 0.5,
                !collapsed,
                k,
                t.faint,
            );
            let badge = grouped(*n);
            let bw = width(self.c, &badge, sz.small, 400);
            fit(
                self.c,
                name,
                hr.x + 24.0 * k,
                base,
                sz.ui,
                400,
                t.text,
                hr.w - 40.0 * k - bw,
            );
            text(
                self.c,
                &badge,
                hr.right() - bw - 6.0 * k,
                yy + row_h * 0.5 + sz.small * 0.36,
                sz.small,
                400,
                t.faint,
            );
            self.said.button(hr, name);
            self.hit(hr, z);
            yy += row_h;
            if !collapsed {
                for (l, (line, at)) in lines.iter().enumerate() {
                    let lr = r(list.x + 12.0 * k, yy, list.w - 24.0 * k, line_h);
                    let lz = Zone::App(Hit::SearchLine(i, l));
                    round(
                        self.c,
                        lr,
                        4.0,
                        if self.hovered(&lz) {
                            look::over(t.hover, t.bg)
                        } else {
                            t.bg
                        },
                    );
                    let lb = yy + line_h * 0.5 + sz.small * 0.36;
                    let lx = lr.x + 12.0 * k;
                    let pre = line.get(..at.0).unwrap_or("");
                    let mid = line.get(at.0..at.1).unwrap_or("");
                    let room = lr.w - 24.0 * k;
                    let pw = width(self.c, pre, sz.small, 400);
                    if pw < room {
                        let mw = width(self.c, mid, sz.small, 400);
                        fill(
                            self.c,
                            r(lx + pw, lb - sz.small, mw.min(room - pw), sz.small * 1.3),
                            t.find,
                        );
                    }
                    fit(self.c, line, lx, lb, sz.small, 400, t.text, room);
                    self.said.button(lr, line);
                    self.hit(lr, lz);
                    yy += line_h + 4.0;
                }
                yy += 6.0;
            }
        }
        let _ = canvas2d::clear_clip(self.c);
    }

    fn draw_bookmarks(&mut self, area: Rect) {
        let t = self.t();
        let sz = self.sz;
        let k = sz.scale;
        self.nav_buttons(
            area,
            &[
                (
                    Icon::Plus,
                    Hit::Command(Cmd::BookmarkFile),
                    "Bookmark the active tab",
                ),
                (
                    Icon::FolderPlus,
                    Hit::Command(Cmd::BookmarkGroup),
                    "New group",
                ),
                (
                    Icon::ChevronsUpDown,
                    Hit::Command(Cmd::ShowBookmarks),
                    "Collapse all",
                ),
            ],
        );
        let rows = self.app.bookmark_rows();
        let top = area.y + 42.0 * k;
        let list = r(area.x, top, area.w, area.bottom() - top);
        self.tree_area = None;
        if rows.is_empty() {
            let lines = self.wrap(
                "No bookmarks yet. Bookmark a note, heading, folder or search to keep it here.",
                sz.ui,
                list.w - 48.0 * k,
            );
            let mut y = list.y + 20.0 * k;
            for l in &lines {
                text(self.c, l, list.x + 24.0 * k, y, sz.ui, 400, t.faint);
                self.said.text(r(list.x, y - 14.0, list.w, 18.0), l);
                y += sz.ui * 1.5;
            }
            return;
        }
        let mut y = list.y;
        let row_h = sz.row;
        for (place, depth, label, kind) in rows {
            if y > list.bottom() {
                break;
            }
            let rr = r(list.x + 12.0 * k, y + 1.0, list.w - 24.0 * k, row_h - 2.0);
            let hit = Hit::Bookmark(place.clone());
            let z = Zone::App(hit.clone());
            if self.hovered(&z) {
                round(self.c, rr, 6.0, t.hover);
            }
            let tx = list.x + 36.0 * k + depth as f32 * 17.0 * k;
            let cy = y + row_h * 0.5;
            let ic = match kind {
                0 => Icon::File,
                1 => Icon::Hash,
                2 => Icon::Folder,
                3 => Icon::Search,
                _ => Icon::Folder,
            };
            if kind == 4 {
                let open = !self.app.bookmarks_closed.contains(&place);
                look::chevron(self.c, tx - 12.0 * k, cy, open, k, t.faint);
            } else {
                icon(self.c, ic, tx - 12.0 * k, cy, k * 0.85, t.faint);
            }
            fit(
                self.c,
                &label,
                tx + 4.0 * k,
                cy + sz.ui * 0.36,
                sz.ui,
                if kind == 4 { 600 } else { 400 },
                if self.hovered(&z) { t.text } else { t.muted },
                rr.right() - tx - 8.0,
            );
            self.button(rr, &label, hit);
            y += row_h;
        }
    }

    // ---- the middle: tab groups -------------------------------------------------

    fn draw_panes(&mut self, main: Rect, lead: f32) {
        let t = self.t();
        let mut placed = Vec::new();
        let mut divs = Vec::new();
        // The main window's split, or the window of its own drawn now.
        let id = self.in_window;
        let layout = self.app.window_layout_mut(id).clone();
        layout.place(area(main), &mut placed, &mut divs);
        let top_right = placed
            .iter()
            .filter(|(_, a)| a.y <= main.y + 0.5)
            .map(|(p, a)| (*p, a.x + a.w))
            .fold(None, |best: Option<(usize, f32)>, (p, x)| match best {
                Some((_, bx)) if bx >= x => best,
                _ => Some((p, x)),
            })
            .map(|(p, _)| p);
        for (p, a) in placed.iter() {
            let rc = rect(*a);
            let lead_here = if (a.x - main.x).abs() < 0.5 && a.y <= main.y + 0.5 {
                lead
            } else {
                0.0
            };
            let reserve = if Some(*p) == top_right && !self.app.right_open && id == 0 {
                44.0 * self.sz.scale
            } else {
                0.0
            };
            self.draw_pane(*p, rc, lead_here, reserve);
        }
        for (i, d) in divs.iter().enumerate() {
            let a = d.area;
            if d.down {
                fill(self.c, r(a.x, a.y + a.h * 0.5, a.w, 1.0), t.border);
            } else {
                fill(self.c, r(a.x + a.w * 0.5, a.y, 1.0, a.h), t.border);
            }
            self.hit(rect(a), Zone::PaneDivider(i));
        }
        self.dividers = divs;
    }

    /// One tab group: its tab bar, the view header, and what the tab shows.
    fn draw_pane(&mut self, pane: usize, a: Rect, lead: f32, reserve: f32) {
        let t = self.t();
        let sz = self.sz;
        let k = sz.scale;
        let focused = pane == self.app.pane;
        let strip = r(a.x, a.y, a.w, sz.tab);
        fill(self.c, strip, t.side);
        fill(self.c, r(a.x, strip.bottom() - 1.0, a.w, 1.0), t.border);
        self.pane_areas.push((pane, a, strip));
        self.hit(a, Zone::Pane(pane));
        // The tabs.
        let (titles, active, pins, links, kinds): (
            Vec<String>,
            usize,
            Vec<bool>,
            Vec<bool>,
            Vec<u8>,
        ) = {
            let (tabs, act) = self.app.pane_tabs(pane);
            (
                tabs.iter().map(|tb| String::from(tb.title())).collect(),
                act,
                tabs.iter().map(|tb| tb.pinned).collect(),
                tabs.iter().map(|tb| tb.group.is_some()).collect(),
                tabs.iter().map(tab_kind).collect(),
            )
        };
        let n = titles.len().max(1);
        let x0 = a.x + 8.0 * k + lead;
        let avail = a.w - lead - 16.0 * k - 36.0 * k - reserve - 30.0 * k;
        let tab_w = (avail / n as f32).clamp(48.0 * k, 200.0 * k);
        let mut x = x0;
        let ty = a.y + 5.0 * k;
        let th = strip.h - 5.0 * k;
        let dragging = match self.drag {
            Drag::Tab {
                pane: dp,
                tab,
                moved: true,
                ..
            } if dp == pane => Some(tab),
            _ => None,
        };
        // Stacked tabs show no chips: each tab is a column with its title
        // down a spine, drawn below.
        let stacked = self.app.pane_stacked(pane);
        let chips: &[String] = if stacked { &[] } else { &titles };
        for (i, title) in chips.iter().enumerate() {
            let tr = r(x, ty, tab_w, th);
            let is_active = i == active;
            let z = Zone::Tab(pane, i);
            self.tab_rects.push((pane, i, tr));
            if is_active {
                look::round_top(
                    self.c,
                    r(tr.x - 1.0, tr.y - 1.0, tr.w + 2.0, tr.h + 1.0),
                    10.0,
                    t.border,
                );
                look::round_top(self.c, r(tr.x, tr.y, tr.w, tr.h + 1.0), 10.0, t.bg);
            } else if self.hovered(&z) && dragging.is_none() {
                round(
                    self.c,
                    r(tr.x + 2.0, tr.y + 3.0, tr.w - 4.0, tr.h - 9.0),
                    6.0,
                    t.hover,
                );
            } else if i + 1 != active && i + 1 < n {
                fill(
                    self.c,
                    r(tr.right(), tr.y + 9.0 * k, 1.0, tr.h - 18.0 * k),
                    t.border_hover,
                );
            }
            if dragging == Some(i) {
                round(
                    self.c,
                    r(tr.x + 2.0, tr.y + 3.0, tr.w - 4.0, tr.h - 9.0),
                    6.0,
                    look::alpha(t.accent, 0.2),
                );
            }
            let mut tx = tr.x + 10.0 * k;
            let cy = tr.y + tr.h * 0.5;
            let tic = match kinds.get(i).copied().unwrap_or(0) {
                1 => Some(Icon::Graph),
                2 => Some(Icon::Canvas),
                3 => Some(Icon::Table),
                4 => Some(Icon::History),
                5 => Some(Icon::Image),
                6 => Some(Icon::Backlinks),
                7 => Some(Icon::FileText),
                9 => Some(Icon::Headphones),
                8 | 10 => Some(Icon::File),
                14 => Some(Icon::List),
                15 => Some(Icon::Kanban),
                16 => Some(Icon::LayoutGrid),
                _ => None,
            };
            let tcol = if is_active && focused {
                t.text
            } else if is_active {
                t.muted
            } else {
                t.faint
            };
            if let Some(ic) = tic {
                icon(self.c, ic, tx + 7.0 * k, cy, k * 0.95, tcol);
                tx += 20.0 * k;
            }
            if links.get(i).copied().unwrap_or(false) {
                icon(self.c, Icon::Link, tx + 6.0 * k, cy, k * 0.75, t.faint);
                tx += 16.0 * k;
            }
            let close_w = 22.0 * k;
            fit(
                self.c,
                title,
                tx,
                cy + sz.ui * 0.36,
                sz.ui,
                400,
                tcol,
                tr.right() - tx - close_w - 6.0 * k,
            );
            self.said.button(tr, title);
            self.hit(tr, z.clone());
            let cb = r(
                tr.right() - close_w - 4.0 * k,
                cy - close_w * 0.5,
                close_w,
                close_w,
            );
            if pins.get(i).copied().unwrap_or(false) {
                icon(
                    self.c,
                    Icon::Pin,
                    cb.x + cb.w * 0.5,
                    cb.y + cb.h * 0.5,
                    k * 0.8,
                    t.muted,
                );
                let mut lbl = String::from("Unpin ");
                lbl.push_str(title);
                if focused {
                    self.button(cb, &lbl, Hit::Pin(i));
                }
            } else if is_active || self.hovered(&z) || self.hovered(&Zone::App(Hit::TabClose(i))) {
                if focused && self.hovered(&Zone::App(Hit::TabClose(i))) {
                    round(self.c, cb, 4.0, t.hover);
                }
                icon(
                    self.c,
                    Icon::Close,
                    cb.x + cb.w * 0.5,
                    cb.y + cb.h * 0.5,
                    k * 0.9,
                    t.muted,
                );
                if focused {
                    let mut lbl = String::from("Close ");
                    lbl.push_str(title);
                    self.button(cb, &lbl, Hit::TabClose(i));
                }
            }
            x += tab_w;
        }
        // New tab, and the list of tabs at the end of the bar.
        let plus_x = x + 20.0 * k;
        if focused {
            self.icon_button(
                Icon::Plus,
                plus_x,
                a.y + sz.tab * 0.5,
                "New tab",
                Hit::NewTab,
                false,
            );
        } else {
            icon(self.c, Icon::Plus, plus_x, a.y + sz.tab * 0.5, k, t.muted);
        }
        icon(
            self.c,
            Icon::ChevronDown,
            a.right() - reserve - 22.0 * k,
            a.y + sz.tab * 0.5,
            k,
            t.muted,
        );
        // The view.
        let mut body = r(a.x, strip.bottom(), a.w, a.bottom() - strip.bottom());
        fill(self.c, body, t.bg);
        if stacked {
            body = self.draw_spines(pane, body, &titles, active, focused);
        }
        if !focused {
            // Drawn from the group's own tab, read-only until it is pressed.
            self.draw_pane_body(pane, body, false);
            return;
        }
        self.draw_pane_body(pane, body, true);
    }

    /// Stacked tabs: a spine for each tab not showing (those before it on
    /// the left, after it on the right), its title running down it. The
    /// room left is the showing tab's.
    fn draw_spines(
        &mut self,
        pane: usize,
        body: Rect,
        titles: &[String],
        active: usize,
        focused: bool,
    ) -> Rect {
        let t = self.t();
        let sz = self.sz;
        let k = sz.scale;
        let sw = 40.0 * k;
        let n = titles.len();
        let before = active.min(n);
        let after = n.saturating_sub(active + 1);
        let spine = |me: &mut Ui, i: usize, x: f32| {
            let rc = r(x, body.y, sw, body.h);
            let z = Zone::Tab(pane, i);
            fill(
                me.c,
                rc,
                if me.hovered(&z) {
                    look::over(t.hover, t.side)
                } else {
                    t.side
                },
            );
            fill(me.c, r(x + sw - 1.0, body.y, 1.0, body.h), t.border);
            // The title turned a quarter clockwise, reading down the spine,
            // as the notes app writes it (krate:gfx/canvas-text).
            let room = (body.h - 24.0 * k).max(0.0);
            let full = titles.get(i).map_or("", String::as_str);
            let mut title = String::from(full);
            if width(me.c, &title, sz.ui, 400) > room {
                let mut cut = String::new();
                for ch in full.chars() {
                    cut.push(ch);
                    let mut trial = cut.clone();
                    trial.push('\u{2026}');
                    if width(me.c, &trial, sz.ui, 400) > room {
                        cut.pop();
                        break;
                    }
                }
                cut.push('\u{2026}');
                title = cut;
            }
            let line = sz.ui * 1.3;
            let _ = krate::gfx::canvas_text::draw_turned(
                me.c,
                &title,
                krate::gfx::types::Point {
                    x: x + (sw - line) * 0.5,
                    y: body.y + 16.0 * k,
                },
                sz.ui,
                if focused { t.muted } else { t.faint },
                look::style(400, false, false),
                1,
            );
            me.tab_rects.push((pane, i, rc));
            me.said.button(rc, titles.get(i).map_or("", String::as_str));
            me.hit(rc, z);
        };
        for i in 0..before {
            spine(self, i, body.x + i as f32 * sw);
        }
        for j in 0..after {
            let i = active + 1 + j;
            spine(self, i, body.right() - (after - j) as f32 * sw);
        }
        let left = body.x + before as f32 * sw;
        let right = body.right() - after as f32 * sw;
        r(left, body.y, (right - left).max(80.0), body.h)
    }

    fn draw_pane_body(&mut self, pane: usize, body: Rect, focused: bool) {
        let t = self.t();
        let sz = self.sz;
        let k = sz.scale;
        let (title, path, kind, has_doc, mode, can_back, can_fwd) = {
            let (tabs, act) = self.app.pane_tabs(pane);
            let tb = tabs.get(act);
            (
                tb.map(|x| String::from(x.title())).unwrap_or_default(),
                tb.and_then(|x| x.path()).map(String::from),
                tb.map_or(0u8, view_kind),
                tb.is_some_and(|x| x.doc.is_some()),
                tb.and_then(|x| x.doc.as_ref())
                    .map_or(Mode::Live, |d| d.mode),
                focused && self.app.can_navigate(false),
                focused && self.app.can_navigate(true),
            )
        };
        let header = self.app.settings.show_view_header();
        let vh = r(
            body.x,
            body.y,
            body.w,
            if header { sz.view_head } else { 0.0 },
        );
        if header {
            // Back and forward, where the file is, and its actions.
            for (j, (fwd, ic, can, label)) in [
                (false, Icon::ArrowLeft, can_back, "Navigate back"),
                (true, Icon::ArrowRight, can_fwd, "Navigate forward"),
            ]
            .into_iter()
            .enumerate()
            {
                let cx = vh.x + 26.0 * k + j as f32 * 28.0 * k;
                let cy = vh.y + vh.h * 0.5;
                if focused && can {
                    self.icon_button(ic, cx, cy, label, Hit::Nav(fwd), false);
                } else {
                    icon(self.c, ic, cx, cy, k, if can { t.muted } else { t.faint });
                }
            }
            let mut crumb = String::new();
            if let Some(p) = &path {
                let folder = parent(p);
                if !folder.is_empty() {
                    crumb.push_str(&folder.replace('/', " / "));
                    crumb.push_str(" / ");
                }
            }
            let tx = vh.x + 76.0 * k;
            let base = vh.y + vh.h * 0.5 + sz.ui * 0.36;
            let room = vh.w - 76.0 * k - 70.0 * k;
            let used = fit(self.c, &crumb, tx, base, sz.ui, 400, t.faint, room * 0.6);
            fit(
                self.c,
                &title,
                tx + used,
                base,
                sz.ui,
                400,
                t.text,
                room - used,
            );
            self.said.text(r(tx, vh.y, room, vh.h), &title);
            if focused {
                self.icon_button(
                    Icon::More,
                    vh.right() - 26.0 * k,
                    vh.y + vh.h * 0.5,
                    "More options",
                    Hit::Command(Cmd::TogglePin),
                    false,
                );
                // The pin command is the menu's first item; the button opens
                // the tab's menu instead (see press()).
                if has_doc {
                    let reading = mode == Mode::Reading;
                    self.icon_button(
                        if reading {
                            Icon::Pencil
                        } else {
                            Icon::BookOpen
                        },
                        vh.right() - 54.0 * k,
                        vh.y + vh.h * 0.5,
                        if reading {
                            "Current view: reading. Click to edit"
                        } else {
                            "Current view: editing. Click to read"
                        },
                        Hit::ModeToggle,
                        false,
                    );
                }
            } else {
                icon(
                    self.c,
                    Icon::More,
                    vh.right() - 26.0 * k,
                    vh.y + vh.h * 0.5,
                    k,
                    t.muted,
                );
            }
        }
        // The status bar sits in the bottom-right corner: a note's text
        // stops above it rather than running under it.
        let foot = if (body.bottom() - self.h).abs() < 1.0 && kind == 0 {
            26.0 * k
        } else {
            0.0
        };
        let content = r(
            body.x,
            vh.bottom(),
            body.w,
            body.bottom() - vh.bottom() - foot,
        );
        match kind {
            1 => self.draw_graph(pane, content, focused),
            2 => self.draw_canvas(pane, content, focused),
            3 => self.draw_base(pane, content, focused),
            4 => self.draw_recovery(pane, content, focused),
            5 => self.draw_file(pane, content, focused),
            6 => self.draw_side(pane, content, focused),
            _ if has_doc => self.draw_note(pane, content, focused),
            _ => {
                if focused {
                    self.view = None;
                    self.shown.area = None;
                }
                self.draw_empty(content, focused);
            }
        }
    }

    fn draw_note(&mut self, pane: usize, ed: Rect, focused: bool) {
        let sz = self.sz;
        let t = self.t();
        let readable = self.app.settings.readable_line_length();
        let pad = (32.0 * sz.scale).min(ed.w * 0.06);
        let col_w = if readable {
            sz.measure.min(ed.w - pad * 2.0).max(80.0)
        } else {
            (ed.w - pad * 2.0).max(80.0)
        };
        let col_x = ed.x + (ed.w - col_w) * 0.5;
        let view = EdView {
            area: ed,
            x: col_x,
            width: col_w,
            sz,
        };
        if focused {
            self.hit(ed, Zone::Editor);
        }
        // Property types the vault keeps (src/propedit.rs).
        crate::propedit::share_types(&mut self.app);
        // Inline title and line numbers, as the settings say (editor.rs).
        self.cache.follow_settings(
            self.app.settings.show_inline_title(),
            self.app.settings.show_line_number(),
            self.app.settings.spellcheck(),
        );
        let caret_on = focused && self.caret_visible();
        let app = &self.app;
        let resolved = |name: &str| -> bool { app.resolved(name) };
        let (tabs, act) = app.pane_tabs(pane);
        let Some(doc) = tabs.get(act).and_then(|tb| tb.doc.as_ref()) else {
            return;
        };
        if focused {
            editor::draw(
                &mut self.cache,
                self.c,
                doc,
                &view,
                t,
                &mut self.shown,
                &resolved,
                caret_on,
                &mut self.said,
            );
            // Where the caret is, for the link completion card.
            let n = doc.cur.line + 1;
            if let Some((_, y, l)) = self.shown.rows.iter().find(|(vi, _, _)| *vi == n) {
                let (row, x) = l.x_of(self.c, doc.line(doc.cur.line), doc.cur.col);
                if let Some(rl) = l.rows.get(row) {
                    self.caret_xy = Some((col_x + x, y + rl.top + rl.h));
                }
            }
            if let Some(thumb) = self.shown.thumb {
                self.hit(self.shown.track.unwrap_or(thumb), Zone::Track);
                self.hit(thumb, Zone::Thumb);
            }
            self.view = Some(view);
        } else {
            let mut other = Shown::default();
            editor::draw(
                &mut self.cache,
                self.c,
                doc,
                &view,
                t,
                &mut other,
                &resolved,
                false,
                &mut self.said,
            );
        }
    }

    /// A press on the note that is not plain caret placing: a link, a tag,
    /// a checkbox, a fold, a callout, a property, a block drawn whole, the
    /// find bar. True when it was one (src/guest/editor.rs decides what).
    fn editor_press(&mut self, px: f32, py: f32, m: Mods) -> bool {
        // The title above the note renames it.
        let on_bar = self.shown.find.iter().any(|(_, rc)| rc.contains(px, py));
        let on_title = !on_bar
            && self
                .shown
                .rows
                .first()
                .is_some_and(|(vi, y, l)| *vi == 0 && py >= *y && py < y + l.h);
        if on_title && self.app.doc().is_some_and(|d| d.mode != Mode::Reading) {
            if let Some(path) = self.app.doc().map(|d| d.path.clone()) {
                self.app.start_rename(&path);
            }
            return true;
        }
        let act = {
            let Some(doc) = self.app.doc() else {
                return false;
            };
            let on_line = doc.mode != Mode::Reading
                && self
                    .shown
                    .rows
                    .iter()
                    .rev()
                    .find(|(_, y, l)| py >= *y && l.h > 0.0)
                    .is_some_and(|(vi, _, _)| *vi == doc.cur.line + 1);
            editor::press(self.c, doc, &self.shown, px, py, !on_line || m.primary)
        };
        let Some(act) = act else {
            return false;
        };
        // A sound or video player's controls (src/guest/widgets.rs).
        if let editor::Act::Media(key, op) = &act {
            self.cache.env.media_act(key, *op);
            return true;
        }
        if let editor::Act::Url(u) = &act {
            if krate::ui::launcher::open_url(u).is_err() {
                self.app.say("Krate did not let this app open web links");
            }
            return true;
        }
        crate::links::act_on(&mut self.app, act, m)
    }

    /// A right-click in the note: on a table cell, Obsidian's table menu
    /// (src/tables.rs).
    fn editor_context(&mut self, px: f32, py: f32) {
        // On a misspelled word: its suggestions, then Add to dictionary and
        // Ignore, as Obsidian's menu has them (src/spell.rs).
        let word = self.app.doc().and_then(|doc| {
            editor::misspelled_at(&mut self.cache, self.c, doc, &self.shown, px, py)
        });
        if let Some((line, start, len, w)) = word {
            if let Some(d) = self.app.doc_mut() {
                d.place(crate::buffer::Pos::new(line, start + len), false);
            }
            let mut items: Vec<(String, crate::app::MenuAction)> = krate::ui::spelling::suggest(&w)
                .into_iter()
                .take(5)
                .map(|s| {
                    let a = crate::spell::SpellAct::Replace {
                        line,
                        start,
                        len,
                        with: s.clone(),
                    };
                    (s, crate::app::MenuAction::Spell(a))
                })
                .collect();
            if items.is_empty() {
                items.push((
                    String::from("No suggestions"),
                    crate::app::MenuAction::Spell(crate::spell::SpellAct::Ignore(String::new())),
                ));
            }
            items.push((
                String::from("Add to dictionary"),
                crate::app::MenuAction::Spell(crate::spell::SpellAct::Learn(w.clone())),
            ));
            items.push((
                String::from("Ignore"),
                crate::app::MenuAction::Spell(crate::spell::SpellAct::Ignore(w)),
            ));
            self.app.overlay = Some(Overlay::Menu(crate::app::Menu {
                x: px,
                y: py,
                items,
            }));
            return;
        }
        let act = self
            .app
            .doc()
            .and_then(|doc| editor::press(self.c, doc, &self.shown, px, py, false));
        if let Some(editor::Act::Place(pos)) = act {
            if let Some(d) = self.app.doc_mut() {
                d.place(pos, false);
            }
        }
        let in_table = self
            .app
            .doc()
            .is_some_and(|d| crate::tables::table_at(d, d.cur.line).is_some());
        if in_table {
            let items = crate::tables::menu()
                .into_iter()
                .map(|(label, op)| (String::from(label), crate::app::MenuAction::Table(op)))
                .collect();
            self.app.overlay = Some(Overlay::Menu(crate::app::Menu {
                x: px,
                y: py,
                items,
            }));
        }
    }

    /// The pointer moved over the window: fold chevrons and page previews.
    fn editor_hover(&mut self, px: f32, py: f32, cmd: bool) -> bool {
        let doc = self.app.doc();
        widgets::hover(&mut self.cache, self.c, doc, &self.shown, px, py, cmd)
    }

    /// After the panes: the page preview, and reading in what embeds asked for.
    fn draw_editor_extras(&mut self, w: f32, h: f32) {
        let t = self.t();
        widgets::draw_preview(&mut self.cache, self.c, t, &self.sz, r(0.0, 0.0, w, h));
        if widgets::fetch(&mut self.cache, &mut self.app, self.c) {
            let _ = window::request_redraw(self.win);
        }
    }

    fn caret_visible(&self) -> bool {
        if self.app.focus != Focus::Editor
            || self.app.overlay.is_some()
            || self.app.explorer.rename.is_some()
        {
            return false;
        }
        let since = ms().saturating_sub(self.last_input);
        (since / BLINK_MS).is_multiple_of(2)
    }

    /// A new tab: what can be done from here, as round buttons.
    fn draw_empty(&mut self, area: Rect, focused: bool) {
        let t = self.t();
        let sz = self.sz;
        let k = sz.scale;
        let mut items: Vec<(Icon, &str, &str, Cmd)> =
            alloc::vec![(Icon::SquarePen, "New note", "\u{2318} N", Cmd::NewNote)];
        if self.app.settings.plugin_on("bases") {
            items.push((Icon::LayoutList, "New base", "", Cmd::NewBase));
        }
        if self.app.settings.plugin_on("canvas") {
            items.push((Icon::Canvas, "New canvas", "", Cmd::NewCanvas));
        }
        items.push((Icon::Search, "Go to file", "\u{2318} O", Cmd::QuickSwitcher));
        let pill_h = 37.0 * k;
        let step = 46.0 * k;
        let pw = 145.0 * k;
        let block = step * (items.len() as f32 - 1.0) + pill_h;
        let x = area.x + (area.w - pw) * 0.5;
        let mut y = area.y + (area.h - block) * 0.5;
        for (ic, label, keys, cmd) in items {
            let b = r(x, y, pw, pill_h);
            let z = Zone::App(Hit::Command(cmd));
            if focused && self.hovered(&z) {
                round(self.c, b, pill_h * 0.5, t.hover);
            }
            look::outline(self.c, b, pill_h * 0.5, 1.0, t.border);
            icon(
                self.c,
                ic,
                b.x + 20.0 * k,
                b.y + pill_h * 0.5,
                k * 1.05,
                t.text,
            );
            let ps = 14.0 * k;
            text(
                self.c,
                label,
                b.x + 37.0 * k,
                b.y + pill_h * 0.5 + ps * 0.36,
                ps,
                400,
                t.text,
            );
            if !keys.is_empty() {
                let kw = width(self.c, keys, sz.ui, 400) + 14.0 * k;
                let kb = r(
                    b.right() - kw - 6.0 * k,
                    b.y + (pill_h - 24.0 * k) * 0.5,
                    kw,
                    24.0 * k,
                );
                round(self.c, kb, 12.0 * k, t.hover);
                text(
                    self.c,
                    keys,
                    kb.x + 7.0 * k,
                    kb.y + kb.h * 0.5 + sz.ui * 0.36,
                    sz.ui,
                    400,
                    t.text,
                );
            }
            if focused {
                self.button(b, label, Hit::Command(cmd));
            }
            y += step;
        }
    }

    // ---- the right sidebar ---------------------------------------------------------

    fn draw_right(&mut self, area: Rect) {
        let t = self.t();
        let sz = self.sz;
        let k = sz.scale;
        fill(self.c, area, t.side);
        let head = sz.head;
        let panes: [(Icon, RightPane, &str, &str); 5] = [
            (
                Icon::Backlinks,
                RightPane::Backlinks,
                "Backlinks",
                "backlink",
            ),
            (
                Icon::Outgoing,
                RightPane::Outgoing,
                "Outgoing links",
                "outgoing-link",
            ),
            (Icon::Tags, RightPane::Tags, "Tags", "tag-pane"),
            (
                Icon::Archive,
                RightPane::Properties,
                "All properties",
                "properties",
            ),
            (Icon::List, RightPane::Outline, "Outline", "outline"),
        ];
        let mut x = area.x + 21.0 * k;
        for (ic, pane, label, plugin) in panes {
            if !self.app.settings.plugin_on(plugin) {
                continue;
            }
            let on = self.app.right == pane;
            self.icon_button(ic, x, head * 0.5, label, Hit::RightTab(pane), on);
            x += 31.0 * k;
        }
        self.icon_button(
            Icon::SidebarRight,
            area.right() - 22.0 * k,
            head * 0.5,
            "Collapse",
            Hit::Ribbon(Cmd::ToggleRight),
            false,
        );
        fill(self.c, r(area.x, head - 1.0, area.w, 1.0), t.border);
        let body = r(area.x, head, area.w, area.h - head);
        self.nav_buttons(
            body,
            &[
                (Icon::List, Hit::Section(0), "Collapse results"),
                (Icon::ChevronsUpDown, Hit::Section(0), "Show more context"),
                (Icon::Sort, Hit::Section(0), "Change sort order"),
                (Icon::Search, Hit::Section(0), "Show search filter"),
            ],
        );
        let list = r(body.x, body.y + 42.0 * k, body.w, body.h - 42.0 * k);
        self.right_area = Some(list);
        let _ = canvas2d::set_clip(self.c, list.x, list.y, list.w, list.h);
        match self.app.right {
            RightPane::Backlinks => self.draw_backlinks(list),
            RightPane::Outgoing => self.draw_outgoing(list),
            RightPane::Tags => self.draw_tags(list),
            RightPane::Properties => self.draw_properties(list),
            RightPane::Outline => self.draw_outline(list),
        }
        let _ = canvas2d::clear_clip(self.c);
    }

    /// A section heading in a side pane ("Linked mentions  5").
    fn section(
        &mut self,
        list: Rect,
        y: f32,
        title: &str,
        n: Option<usize>,
        hit: Hit,
        open: bool,
    ) -> f32 {
        let t = self.t();
        let sz = self.sz;
        let k = sz.scale;
        let hr = r(list.x + 12.0 * k, y, list.w - 24.0 * k, sz.row);
        if self.hovered(&Zone::App(hit.clone())) {
            round(self.c, hr, 6.0, t.hover);
        }
        let base = y + sz.row * 0.5 + sz.ui * 0.36;
        text(
            self.c,
            title,
            list.x + 19.0 * k,
            base,
            sz.ui,
            600,
            if open { t.text } else { t.faint },
        );
        if let Some(n) = n {
            let s = grouped(n);
            let nw = width(self.c, &s, sz.small, 400);
            text(
                self.c,
                &s,
                hr.right() - nw - 8.0 * k,
                base,
                sz.small,
                400,
                t.faint,
            );
        }
        self.button(hr, title, hit);
        y + sz.row + 4.0 * k
    }

    /// A note's name with its chevron and count, then its lines in boxes.
    #[allow(clippy::too_many_arguments)]
    fn result_block(
        &mut self,
        list: Rect,
        mut y: f32,
        name: &str,
        lines: &[String],
        key: &str,
        open_hit: Hit,
        line_hit: &dyn Fn(usize) -> Hit,
    ) -> f32 {
        let t = self.t();
        let sz = self.sz;
        let k = sz.scale;
        let nr = r(list.x + 12.0 * k, y, list.w - 24.0 * k, sz.row);
        let z = Zone::App(open_hit.clone());
        if self.hovered(&z) {
            round(self.c, nr, 6.0, t.hover);
        }
        let cy = y + sz.row * 0.5;
        let n = lines.iter().filter(|l| !l.is_empty()).count();
        let told = self.block_count.take();
        // A file with no lines to show (a canvas) has no fold and no count.
        if n > 0 {
            look::chevron(self.c, list.x + 24.0 * k, cy, true, k, t.faint);
        }
        let ns = match told {
            Some(c) => grouped(c),
            None if n > 0 => grouped(n),
            None => String::new(),
        };
        let nw = width(self.c, &ns, sz.small, 400);
        fit(
            self.c,
            name,
            list.x + 36.0 * k,
            cy + sz.ui * 0.36,
            sz.ui,
            400,
            t.text,
            nr.w - 40.0 * k - nw,
        );
        text(
            self.c,
            &ns,
            nr.right() - nw - 8.0 * k,
            cy + sz.small * 0.36,
            sz.small,
            400,
            t.faint,
        );
        self.button(nr, name, open_hit);
        y += sz.row + 2.0 * k;
        for (l, line) in lines.iter().enumerate().take(6) {
            if line.is_empty() {
                continue;
            }
            // The line as written, links and all, as the notes app shows it.
            let shown = String::from(line.as_str());
            let room = list.w - 24.0 * k - 26.0 * k;
            let wrapped = self.wrap(&shown, sz.ui, room);
            let wrapped: Vec<String> = wrapped.into_iter().take(4).collect();
            let lh = sz.ui * 1.5;
            let bh = wrapped.len().max(1) as f32 * lh + 16.0 * k;
            let lr = r(list.x + 12.0 * k, y, list.w - 24.0 * k, bh);
            let lz = Zone::App(line_hit(l));
            round(
                self.c,
                lr,
                4.0,
                if self.hovered(&lz) {
                    look::over(t.hover, t.bg)
                } else {
                    t.bg
                },
            );
            let mut ly = y + 8.0 * k + lh * 0.5 + sz.ui * 0.36;
            for wl in &wrapped {
                // The name, marked where it appears.
                if !key.is_empty() {
                    let low = crate::text::lower(wl);
                    // A link is marked whole, brackets and all.
                    let mut link = String::from("[[");
                    link.push_str(key);
                    let span = match low.find(&link) {
                        Some(i) => low[i..].find("]]").map(|e| (i, i + e + 2)),
                        None => None,
                    }
                    .or_else(|| low.find(key).map(|i| (i, i + key.len())));
                    if let Some((i, j)) = span {
                        if wl.is_char_boundary(i) && wl.is_char_boundary(j) {
                            let pre = width(self.c, &wl[..i], sz.ui, 400);
                            let mw = width(self.c, &wl[i..j], sz.ui, 400);
                            fill(
                                self.c,
                                r(lr.x + 13.0 * k + pre, ly - sz.ui, mw, sz.ui * 1.3),
                                t.find,
                            );
                        }
                    }
                }
                text(self.c, wl, lr.x + 13.0 * k, ly, sz.ui, 400, t.text);
                ly += lh;
            }
            self.said.button(lr, &shown);
            self.hit(lr, lz);
            y += bh + 6.0 * k;
        }
        y + 6.0 * k
    }

    fn draw_backlinks(&mut self, list: Rect) {
        let t = self.t();
        let sz = self.sz;
        let k = sz.scale;
        let mut y = list.y - self.app.right_scroll;
        if self.app.subject_path().is_none() {
            text(
                self.c,
                "No backlinks found.",
                list.x + 19.0 * k,
                y + 16.0,
                sz.ui,
                400,
                t.faint,
            );
            return;
        }
        let pending = self.app.vault.as_ref().map_or(0, |v| v.index_pending());
        let key = self
            .app
            .subject_path()
            .map(|p| crate::text::lower(crate::app::strip_ext(crate::vfs::file_name(&p))))
            .unwrap_or_default();
        let data: Vec<(String, Vec<String>)> = self
            .app
            .backlinks()
            .iter()
            .map(|b| {
                let file = crate::vfs::file_name(&b.path);
                let name = String::from(if b.path.ends_with(".md") {
                    crate::app::strip_ext(file)
                } else {
                    file
                });
                let ctx = if b.context.is_empty() {
                    b.lines.iter().map(|_| String::new()).collect()
                } else {
                    b.context.clone()
                };
                (name, ctx)
            })
            .collect();
        let open = self.app.linked_open;
        // The count is of notes whose lines link (a canvas has none).
        let counted = data
            .iter()
            .filter(|(_, ctx)| ctx.iter().any(|l| !l.is_empty()))
            .count();
        y = self.section(
            list,
            y,
            "Linked mentions",
            Some(counted),
            Hit::Section(0),
            open,
        );
        if open {
            if data.is_empty() {
                let msg = if pending > 0 {
                    "Still reading the vault\u{2026}"
                } else {
                    "No backlinks found."
                };
                text(
                    self.c,
                    msg,
                    list.x + 19.0 * k,
                    y + 14.0,
                    sz.ui,
                    400,
                    t.faint,
                );
                self.said.text(r(list.x, y, list.w, 22.0), msg);
                y += 30.0 * k;
            }
            for (i, (name, ctx)) in data.iter().enumerate() {
                if y > list.bottom() {
                    break;
                }
                y = self.result_block(list, y, name, ctx, &key, Hit::Backlink(i, 0), &|l| {
                    Hit::Backlink(i, l)
                });
            }
        }
        y += 14.0 * k;
        // Unlinked mentions: found between frames once the section is open.
        // Each note with its lines and how many mentions (the notes app
        // counts mentions, not notes); a canvas shows its file name only.
        let unl: Option<Vec<(String, Vec<String>, usize)>> = self.app.unlinked.as_ref().map(|u| {
            u.found
                .iter()
                .map(|(p, ls)| {
                    let n: usize = ls
                        .iter()
                        .map(|(_, s)| crate::workspace::mention_count(s, &key).max(1))
                        .sum();
                    if p.ends_with(".canvas") {
                        (String::from(crate::vfs::file_name(p)), Vec::new(), n)
                    } else {
                        (
                            String::from(crate::app::strip_ext(crate::vfs::file_name(p))),
                            ls.iter().map(|(_, s)| s.clone()).collect(),
                            n,
                        )
                    }
                })
                .collect()
        });
        let busy = self
            .app
            .unlinked
            .as_ref()
            .is_some_and(|u| !u.queue.is_empty());
        let uopen = self.app.unlinked_open;
        y = self.section(
            list,
            y,
            "Unlinked mentions",
            unl.as_ref()
                .map(|v| v.iter().map(|x| x.2).sum())
                .filter(|_| uopen),
            Hit::Section(1),
            uopen,
        );
        if uopen {
            let found = unl.unwrap_or_default();
            if found.is_empty() {
                let msg = if busy {
                    "Searching\u{2026}"
                } else {
                    "No unlinked mentions found."
                };
                text(
                    self.c,
                    msg,
                    list.x + 19.0 * k,
                    y + 14.0,
                    sz.ui,
                    400,
                    t.faint,
                );
            }
            for (i, (name, lines, count)) in found.iter().enumerate() {
                if y > list.bottom() {
                    break;
                }
                let top = y;
                self.block_count = Some(*count);
                y = self.result_block(list, y, name, lines, &key, Hit::Unlinked(i, 0), &|l| {
                    Hit::Unlinked(i, l)
                });
                // "Link" turns these mentions into links; it shows while
                // the pointer is on the note's row, as in the notes app.
                let row_hit = r(list.x, top, list.w, y - top);
                let over = self
                    .hover_point
                    .is_some_and(|(hx, hy)| row_hit.contains(hx, hy));
                if !over {
                    continue;
                }
                let bw = 44.0 * k;
                let b = r(
                    list.right() - 12.0 * k - bw - 34.0 * k,
                    top + 3.0 * k,
                    bw,
                    sz.row - 6.0 * k,
                );
                let z = Zone::App(Hit::LinkMention(i));
                round(
                    self.c,
                    b,
                    4.0,
                    if self.hovered(&z) {
                        t.border_hover
                    } else {
                        t.button
                    },
                );
                let lw = width(self.c, "Link", sz.small, 500);
                text(
                    self.c,
                    "Link",
                    b.x + (b.w - lw) * 0.5,
                    b.y + b.h * 0.5 + sz.small * 0.36,
                    sz.small,
                    500,
                    t.text,
                );
                self.button(b, "Link", Hit::LinkMention(i));
            }
        }
    }

    fn draw_outgoing(&mut self, list: Rect) {
        let t = self.t();
        let sz = self.sz;
        let k = sz.scale;
        let mut y = list.y - self.app.right_scroll;
        let data: Vec<(String, bool)> = self.app.outgoing().to_vec();
        y = self.section(list, y, "Links", Some(data.len()), Hit::Section(0), true);
        if data.is_empty() {
            text(
                self.c,
                "No links found.",
                list.x + 19.0 * k,
                y + 14.0,
                sz.ui,
                400,
                t.faint,
            );
        }
        for (i, (name, ok)) in data.iter().enumerate() {
            if y > list.bottom() {
                break;
            }
            let nr = r(list.x + 12.0 * k, y, list.w - 24.0 * k, sz.row);
            let z = Zone::App(Hit::Outgoing(i));
            if self.hovered(&z) {
                round(self.c, nr, 6.0, t.hover);
            }
            icon(
                self.c,
                Icon::File,
                list.x + 26.0 * k,
                y + sz.row * 0.5,
                k * 0.85,
                t.faint,
            );
            fit(
                self.c,
                name,
                list.x + 40.0 * k,
                y + sz.row * 0.5 + sz.ui * 0.36,
                sz.ui,
                400,
                if *ok { t.text } else { t.faint },
                nr.w - 40.0 * k,
            );
            self.said.button(nr, name);
            self.hit(nr, z);
            y += sz.row;
        }
    }

    fn draw_tags(&mut self, list: Rect) {
        let t = self.t();
        let sz = self.sz;
        let k = sz.scale;
        let mut y = list.y - self.app.right_scroll;
        let rows = self.app.tag_rows();
        let pending = self.app.vault.as_ref().map_or(0, |v| v.index_pending());
        if rows.is_empty() {
            let msg = if pending > 0 {
                "Still reading the vault\u{2026}"
            } else {
                "No tags found."
            };
            text(
                self.c,
                msg,
                list.x + 19.0 * k,
                y + 16.0,
                sz.ui,
                400,
                t.faint,
            );
        }
        for (i, (full, label, n, depth, has)) in rows.iter().enumerate() {
            if y > list.bottom() {
                break;
            }
            if y + sz.row < list.y {
                y += sz.row;
                continue;
            }
            let nr = r(list.x + 12.0 * k, y, list.w - 24.0 * k, sz.row);
            let z = Zone::App(Hit::TagRow(i));
            if self.hovered(&z) {
                round(self.c, nr, 6.0, t.hover);
            }
            let tx = list.x + 36.0 * k + *depth as f32 * 17.0 * k;
            let cy = y + sz.row * 0.5;
            let num = grouped(*n);
            let nw = width(self.c, &num, sz.small, 400);
            fit(
                self.c,
                label,
                tx,
                cy + sz.ui * 0.36,
                sz.ui,
                400,
                t.text,
                nr.right() - tx - nw - 16.0 * k,
            );
            text(
                self.c,
                &num,
                nr.right() - nw - 8.0 * k,
                cy + sz.small * 0.36,
                sz.small,
                400,
                t.faint,
            );
            let mut s = String::from("#");
            s.push_str(full);
            self.said.button(nr, &s);
            self.hit(nr, z);
            if *has {
                let lower = crate::text::lower(full);
                let open = !self.app.tags_closed.contains(&lower);
                let cb = r(tx - 20.0 * k, y, 16.0 * k, sz.row);
                look::chevron(self.c, tx - 12.0 * k, cy, open, k, t.faint);
                self.button(cb, "Fold", Hit::TagFold(lower));
            }
            y += sz.row;
        }
    }

    fn draw_properties(&mut self, list: Rect) {
        let t = self.t();
        let sz = self.sz;
        let k = sz.scale;
        let mut y = list.y - self.app.right_scroll;
        let props = self.app.all_properties();
        if props.is_empty() {
            text(
                self.c,
                "No properties found.",
                list.x + 19.0 * k,
                y + 16.0,
                sz.ui,
                400,
                t.faint,
            );
        }
        for (i, (name, n)) in props.iter().enumerate() {
            if y > list.bottom() {
                break;
            }
            let nr = r(list.x + 12.0 * k, y, list.w - 24.0 * k, sz.row);
            let z = Zone::App(Hit::Property(i));
            if self.hovered(&z) {
                round(self.c, nr, 6.0, t.hover);
            }
            let cy = y + sz.row * 0.5;
            icon(self.c, Icon::Text, list.x + 26.0 * k, cy, k * 0.85, t.faint);
            let num = grouped(*n);
            let nw = width(self.c, &num, sz.small, 400);
            fit(
                self.c,
                name,
                list.x + 40.0 * k,
                cy + sz.ui * 0.36,
                sz.ui,
                400,
                t.text,
                nr.w - 56.0 * k - nw,
            );
            text(
                self.c,
                &num,
                nr.right() - nw - 8.0 * k,
                cy + sz.small * 0.36,
                sz.small,
                400,
                t.faint,
            );
            self.said.button(nr, name);
            self.hit(nr, z);
            y += sz.row;
        }
    }

    fn draw_outline(&mut self, list: Rect) {
        let t = self.t();
        let sz = self.sz;
        let k = sz.scale;
        let mut y = list.y - self.app.right_scroll;
        let data: Vec<(u8, String)> = self
            .app
            .outline()
            .iter()
            .map(|h| (h.level, h.text.clone()))
            .collect();
        if data.is_empty() {
            text(
                self.c,
                "No headings found.",
                list.x + 19.0 * k,
                y + 16.0,
                sz.ui,
                400,
                t.faint,
            );
        }
        let min = data.iter().map(|(l, _)| *l).min().unwrap_or(1);
        for (i, (level, name)) in data.iter().enumerate() {
            if y > list.bottom() {
                break;
            }
            if y + sz.row < list.y {
                y += sz.row;
                continue;
            }
            let nr = r(list.x + 12.0 * k, y, list.w - 24.0 * k, sz.row);
            let z = Zone::App(Hit::Outline(i));
            if self.hovered(&z) {
                round(self.c, nr, 6.0, t.hover);
            }
            let ind = (level.saturating_sub(min)) as f32 * 17.0 * k;
            for d in 0..level.saturating_sub(min) {
                fill(
                    self.c,
                    r(list.x + 24.0 * k + d as f32 * 17.0 * k, y, 1.0, sz.row),
                    look::alpha(t.text, 0.12),
                );
            }
            fit(
                self.c,
                name,
                list.x + 36.0 * k + ind,
                y + sz.row * 0.5 + sz.ui * 0.36,
                sz.ui,
                400,
                t.muted,
                nr.w - 30.0 * k - ind,
            );
            self.said.button(nr, name);
            self.hit(nr, z);
            y += sz.row;
        }
    }

    // ---- the status bar ---------------------------------------------------------------

    fn draw_status(&mut self, w: f32, h: f32, _left: f32) {
        let t = self.t();
        let sz = self.sz;
        let k = sz.scale;
        let mut parts: Vec<String> = Vec::new();
        if let Some((msg, _)) = self.app.status.as_ref() {
            parts.push(msg.clone());
        }
        if let Some(v) = self.app.vault.as_ref() {
            let pending = v.index_pending();
            if pending > 0 {
                let mut s = String::from("Indexing ");
                s.push_str(&grouped(v.indexed()));
                s.push_str(" of ");
                s.push_str(&grouped(v.note_count()));
                parts.push(s);
            }
        }
        let has_doc = self.app.doc().is_some();
        let has_file = self.app.active_path().is_some();
        let mut mode_icon = false;
        // The editing-mode pencil goes after this part (the backlinks).
        let mut backlink_at = usize::MAX;
        if has_file && self.app.settings.plugin_on("backlink") {
            let n = self.app.backlinks().len();
            parts.push(count(n, "backlink", "backlinks"));
            backlink_at = parts.len() - 1;
        }
        if has_doc {
            mode_icon = true;
            if let Some(d) = self.app.doc() {
                if self.app.settings.plugin_on("word-count") {
                    parts.push(count(d.words, "word", "words"));
                    parts.push(count(d.chars, "character", "characters"));
                }
                if d.save_error.is_some() {
                    parts.push(String::from("Not saved"));
                }
            }
        }
        if parts.is_empty() {
            return;
        }
        let gap = 16.0 * k;
        let mut widths: Vec<f32> = parts
            .iter()
            .map(|p| width(self.c, p, sz.small, 400))
            .collect();
        let icon_w = if mode_icon { 30.0 * k } else { 0.0 };
        let room = w - 8.0;
        let total_of =
            |ws: &[f32]| ws.iter().sum::<f32>() + gap * (ws.len() as f32 - 1.0) + 24.0 * k + icon_w;
        let mut total = total_of(&widths);
        while total > room && parts.len() > 1 {
            parts.pop();
            widths.pop();
            total = total_of(&widths);
        }
        let bh = 26.0 * k;
        let bx = w - total;
        let by = h - bh;
        let _ = canvas2d::fill_round_rect(
            self.c,
            look::g(r(bx - 1.0, by - 1.0, total + 1.0, bh + 1.0)),
            krate::gfx::types::CornerRadii {
                top_left: 8.0,
                top_right: 0.0,
                bottom_right: 0.0,
                bottom_left: 0.0,
            },
            t.border,
        );
        let _ = canvas2d::fill_round_rect(
            self.c,
            look::g(r(bx, by, total, bh)),
            krate::gfx::types::CornerRadii {
                top_left: 8.0,
                top_right: 0.0,
                bottom_right: 0.0,
                bottom_left: 0.0,
            },
            t.side,
        );
        let mut x = bx + 12.0 * k;
        let base = by + bh * 0.5 + sz.small * 0.36;
        for (i, (p, pw)) in parts.iter().zip(widths).enumerate() {
            text(self.c, p, x, base, sz.small, 400, t.muted);
            self.said.text(r(x, by, pw, bh), p);
            x += pw + gap;
            if mode_icon && i == backlink_at && has_file {
                let reading = self.app.doc().is_some_and(|d| d.mode == Mode::Reading);
                icon(
                    self.c,
                    if reading {
                        Icon::BookOpen
                    } else {
                        Icon::Pencil
                    },
                    x + 4.0 * k,
                    by + bh * 0.5,
                    k * 0.85,
                    t.muted,
                );
                self.hit(r(x - 6.0, by, 22.0 * k, bh), Zone::App(Hit::ModeToggle));
                x += icon_w - gap * 0.5;
            }
        }
    }

    // ---- overlays ------------------------------------------------------------------------

    fn draw_overlay(&mut self, w: f32, h: f32) {
        let t = self.t();
        let sz = self.sz;
        let k = sz.scale;
        // Link completion under the caret.
        if self.app.overlay.is_none() {
            if let (Some((cx, cy)), Some(comp)) = (self.caret_xy, self.app.completion.as_ref()) {
                let items: Vec<(String, String)> = comp.items.clone();
                let sel = comp.sel;
                let rh = 30.0 * k;
                let cw = 320.0 * k;
                let shown = items.len().min(8);
                let ch = if shown == 0 { rh } else { rh * shown as f32 } + 12.0;
                let x = cx.min(w - cw - 8.0).max(8.0);
                let y = if cy + ch + 8.0 > h {
                    (cy - ch - 30.0).max(8.0)
                } else {
                    cy + 4.0
                };
                let card = r(x, y, cw, ch);
                let _ = canvas2d::drop_shadow_round_rect(
                    self.c,
                    look::g(card),
                    look::radii(8.0),
                    16.0,
                    look::alpha(look::hex(0), 0.4),
                );
                round(self.c, card, 8.0, t.side);
                look::outline(self.c, card, 8.0, 1.0, t.border_hover);
                if items.is_empty() {
                    text(
                        self.c,
                        "No match found",
                        x + 12.0,
                        y + 6.0 + rh * 0.5 + sz.small * 0.36,
                        sz.small,
                        400,
                        t.faint,
                    );
                }
                let first = sel.saturating_sub(shown.saturating_sub(1));
                for (n, (name, path)) in items.iter().enumerate().skip(first).take(shown) {
                    let ir = r(x + 6.0, y + 6.0 + (n - first) as f32 * rh, cw - 12.0, rh);
                    if n == sel {
                        round(self.c, ir, 5.0, t.hover);
                    }
                    let nw = fit(
                        self.c,
                        name,
                        ir.x + 10.0,
                        ir.y + rh * 0.5 + sz.ui * 0.36,
                        sz.ui,
                        400,
                        t.text,
                        ir.w * 0.6,
                    );
                    let folder = parent(path);
                    if !folder.is_empty() {
                        fit(
                            self.c,
                            folder,
                            ir.x + 22.0 + nw,
                            ir.y + rh * 0.5 + sz.small * 0.36,
                            sz.small,
                            400,
                            t.faint,
                            ir.w - nw - 32.0,
                        );
                    }
                    self.button(ir, name, Hit::Completion(n));
                }
            }
            return;
        }
        let is_menu = matches!(self.app.overlay, Some(Overlay::Menu(_)));
        if !is_menu {
            fill(self.c, r(0.0, 0.0, w, h), t.scrim);
        }
        self.hit(r(0.0, 0.0, w, h), Zone::App(Hit::Scrim));
        if matches!(self.app.overlay, Some(Overlay::Settings(_))) {
            self.draw_settings(w, h);
            return;
        }
        let Some(ov) = self.app.overlay.as_mut() else {
            return;
        };
        match ov {
            Overlay::List(p) => {
                let pw = (700.0 * k).min(w * 0.8).min(w - 24.0);
                let x = (w - pw) * 0.5;
                let y = (h * 0.12).min(80.0 * k);
                let two_line = matches!(
                    p.kind,
                    ListKind::Switcher
                        | ListKind::MergeInto(_)
                        | ListKind::ExtractTo(_)
                        | ListKind::CanvasNote
                );
                let rh = if two_line { 46.0 * k } else { 36.0 * k };
                let in_h = 48.0 * k;
                let foot = 36.0 * k;
                let max_rows = (((h * 0.7 - in_h - foot - 16.0) / rh) as usize).clamp(1, 12);
                let shown = p.items.len().min(max_rows);
                if p.sel < p.first {
                    p.first = p.sel;
                }
                if p.sel >= p.first + max_rows {
                    p.first = p.sel + 1 - max_rows;
                }
                let card = r(x, y, pw, in_h + shown as f32 * rh + foot + 12.0);
                let items = p.items.clone();
                let (sel, first) = (p.sel, p.first);
                let input = p.input.clone();
                let placeholder = match &p.kind {
                    ListKind::Switcher => "Find or create a note...",
                    ListKind::Palette => "Select a command...",
                    ListKind::MoveTo(_) => "Type a folder",
                    ListKind::Template => "Find a template...",
                    ListKind::MergeInto(_) => "Find a note to merge into...",
                    ListKind::ExtractTo(_) => "Find a note, or type the new note's name...",
                    ListKind::CanvasNote => "Find a note to add...",
                    ListKind::LinkWith => "Choose a tab to link with...",
                };
                let footer = match &p.kind {
                    ListKind::Switcher => "\u{2191}\u{2193} to navigate    \u{21b5} to open    \u{2318} \u{21b5} to open in new tab    shift \u{21b5} to create    esc to dismiss",
                    ListKind::Palette => "\u{2191}\u{2193} to navigate    \u{21b5} to use    esc to dismiss",
                    _ => "\u{2191}\u{2193} to navigate    \u{21b5} to choose    esc to dismiss",
                };
                let _ = canvas2d::drop_shadow_round_rect(
                    self.c,
                    look::g(card),
                    look::radii(12.0),
                    30.0,
                    look::alpha(look::hex(0), 0.35),
                );
                round(self.c, card, 12.0, t.modal);
                look::outline(self.c, card, 12.0, 1.0, t.border_hover);
                self.list_area = Some(card);
                self.hit(card, Zone::App(Hit::ListBox));
                let base = y + in_h * 0.5 + sz.medium * 0.36;
                if input.text.is_empty() {
                    text(
                        self.c,
                        placeholder,
                        x + 18.0 * k,
                        base,
                        sz.medium,
                        400,
                        t.faint,
                    );
                }
                self.draw_input(
                    &input,
                    x + 18.0 * k,
                    base,
                    sz.medium,
                    t.text,
                    x + pw - 18.0,
                    true,
                );
                self.said.field(r(x, y, pw, in_h), placeholder, &input.text);
                fill(self.c, r(x, y + in_h, pw, 1.0), t.border);
                for (n, item) in items.iter().enumerate().skip(first).take(shown) {
                    let ir = r(
                        x + 6.0,
                        y + in_h + 6.0 + (n - first) as f32 * rh,
                        pw - 12.0,
                        rh,
                    );
                    if n == sel {
                        round(self.c, ir, 6.0, t.hover);
                    }
                    let hint_w = if item.hint.is_empty() {
                        0.0
                    } else {
                        width(self.c, &item.hint, sz.small, 400) + 14.0
                    };
                    if two_line {
                        fit(
                            self.c,
                            &item.title,
                            ir.x + 12.0,
                            ir.y + 20.0 * k,
                            sz.medium,
                            400,
                            t.text,
                            ir.w - 24.0 - hint_w,
                        );
                        if !item.detail.is_empty() {
                            fit(
                                self.c,
                                &item.detail,
                                ir.x + 12.0,
                                ir.y + 37.0 * k,
                                sz.small,
                                400,
                                t.faint,
                                ir.w - 24.0 - hint_w,
                            );
                        }
                    } else {
                        fit(
                            self.c,
                            &item.title,
                            ir.x + 12.0,
                            ir.y + rh * 0.5 + sz.medium * 0.36,
                            sz.medium,
                            400,
                            t.text,
                            ir.w - 24.0 - hint_w,
                        );
                    }
                    if hint_w > 0.0 {
                        let hb = r(
                            ir.right() - hint_w - 6.0,
                            ir.y + (rh - 20.0) * 0.5,
                            hint_w,
                            20.0,
                        );
                        round(self.c, hb, 10.0, t.hover);
                        text(
                            self.c,
                            &item.hint,
                            hb.x + 7.0,
                            hb.y + 10.0 + sz.small * 0.36,
                            sz.small,
                            400,
                            t.muted,
                        );
                    }
                    self.button(ir, &item.title, Hit::ListItem(n));
                }
                let fy = card.bottom() - foot;
                fill(self.c, r(x, fy, pw, 1.0), t.border);
                fit(
                    self.c,
                    footer,
                    x + 16.0,
                    fy + foot * 0.5 + sz.small * 0.36,
                    sz.small,
                    400,
                    t.muted,
                    pw - 32.0,
                );
            }
            Overlay::Menu(m) => {
                let items: Vec<String> = m.items.iter().map(|(l, _)| l.clone()).collect();
                let mw = 210.0 * k;
                let rh = 28.0 * k;
                let mh = items.len() as f32 * rh + 12.0;
                let x = m.x.min(w - mw - 4.0).max(4.0);
                let y = m.y.min(h - mh - 4.0).max(4.0);
                let card = r(x, y, mw, mh);
                let _ = canvas2d::drop_shadow_round_rect(
                    self.c,
                    look::g(card),
                    look::radii(8.0),
                    16.0,
                    look::alpha(look::hex(0), 0.4),
                );
                round(self.c, card, 8.0, t.side);
                look::outline(self.c, card, 8.0, 1.0, t.border_hover);
                for (i, label) in items.iter().enumerate() {
                    let ir = r(x + 6.0, y + 6.0 + i as f32 * rh, mw - 12.0, rh);
                    let z = Zone::App(Hit::MenuItem(i));
                    if self.hovered(&z) {
                        round(self.c, ir, 5.0, t.hover);
                    }
                    let danger = label == "Delete" || label == "Remove";
                    let (label, ticked) = match label.strip_suffix(crate::app::TICKED) {
                        Some(l) => (l, true),
                        None => (label.as_str(), false),
                    };
                    if ticked {
                        icon(
                            self.c,
                            Icon::Check,
                            ir.right() - 16.0 * k,
                            ir.y + rh * 0.5,
                            k,
                            t.text,
                        );
                    }
                    text(
                        self.c,
                        label,
                        ir.x + 10.0,
                        ir.y + rh * 0.5 + sz.ui * 0.36,
                        sz.ui,
                        400,
                        if danger { t.danger } else { t.text },
                    );
                    self.button(ir, label, Hit::MenuItem(i));
                }
            }
            Overlay::Confirm(cf) => {
                let title = cf.title.clone();
                let body = cf.body.clone();
                let yes = cf.yes;
                let cw = (460.0 * k).min(w - 24.0);
                let mut lines = Vec::new();
                for para in body.split('\n') {
                    if para.is_empty() {
                        lines.push(String::new());
                    } else {
                        lines.extend(self.wrap(para, sz.ui, cw - 48.0));
                    }
                }
                let ch = 70.0 * k + lines.len() as f32 * sz.ui * 1.5 + 64.0 * k;
                let x = (w - cw) * 0.5;
                let y = (h - ch) * 0.35;
                let card = r(x, y, cw, ch);
                round(self.c, card, 12.0, t.modal);
                look::outline(self.c, card, 12.0, 1.0, t.border_hover);
                self.hit(card, Zone::App(Hit::ListBox));
                text(
                    self.c,
                    &title,
                    x + 24.0,
                    y + 40.0 * k,
                    18.0 * k,
                    600,
                    t.text,
                );
                self.said.heading(r(x, y, cw, 50.0), &title);
                let mut ly = y + 72.0 * k;
                for l in &lines {
                    text(self.c, l, x + 24.0, ly, sz.ui, 400, t.text);
                    ly += sz.ui * 1.5;
                }
                self.said.text(r(x, y + 50.0, cw, ly - y - 50.0), &body);
                let bw = 84.0 * k;
                let bh = 30.0 * k;
                let by = card.bottom() - bh - 20.0 * k;
                let yes_r = r(card.right() - 24.0 - bw, by, bw, bh);
                let no_r = r(yes_r.x - 10.0 - bw, by, bw, bh);
                round(
                    self.c,
                    yes_r,
                    5.0,
                    if yes == "Delete" { t.danger } else { t.accent },
                );
                round(self.c, no_r, 5.0, t.button);
                let yw = width(self.c, yes, sz.ui, 500);
                text(
                    self.c,
                    yes,
                    yes_r.x + (bw - yw) * 0.5,
                    by + bh * 0.5 + sz.ui * 0.36,
                    sz.ui,
                    500,
                    t.on_accent,
                );
                let nw = width(self.c, "Cancel", sz.ui, 500);
                text(
                    self.c,
                    "Cancel",
                    no_r.x + (bw - nw) * 0.5,
                    by + bh * 0.5 + sz.ui * 0.36,
                    sz.ui,
                    500,
                    t.text,
                );
                self.button(yes_r, yes, Hit::ConfirmYes);
                self.button(no_r, "Cancel", Hit::ConfirmNo);
            }
            Overlay::Ask(a) => {
                let title = a.title.clone();
                let input = a.input.clone();
                let cw = (420.0 * k).min(w - 24.0);
                let ch = 150.0 * k;
                let x = (w - cw) * 0.5;
                let y = (h - ch) * 0.3;
                let card = r(x, y, cw, ch);
                round(self.c, card, 12.0, t.modal);
                look::outline(self.c, card, 12.0, 1.0, t.border_hover);
                self.hit(card, Zone::App(Hit::ListBox));
                text(
                    self.c,
                    &title,
                    x + 24.0,
                    y + 38.0 * k,
                    18.0 * k,
                    600,
                    t.text,
                );
                let fb = r(x + 24.0, y + 56.0 * k, cw - 48.0, 32.0 * k);
                round(self.c, fb, 5.0, t.field);
                look::outline(self.c, fb, 5.0, 1.0, t.border_focus);
                self.draw_input(
                    &input,
                    fb.x + 10.0,
                    fb.y + fb.h * 0.5 + sz.ui * 0.36,
                    sz.ui,
                    t.text,
                    fb.right() - 8.0,
                    true,
                );
                self.said.field(fb, &title, &input.text);
                let bw = 84.0 * k;
                let ok = r(
                    card.right() - 24.0 - bw,
                    card.bottom() - 46.0 * k,
                    bw,
                    30.0 * k,
                );
                round(self.c, ok, 5.0, t.accent);
                let ow = width(self.c, "Save", sz.ui, 500);
                text(
                    self.c,
                    "Save",
                    ok.x + (bw - ow) * 0.5,
                    ok.y + ok.h * 0.5 + sz.ui * 0.36,
                    sz.ui,
                    500,
                    t.on_accent,
                );
                self.button(ok, "Save", Hit::AskOk);
            }
            Overlay::Settings(_) => {}
        }
    }

    fn wrap(&self, s: &str, size: f32, room: f32) -> Vec<String> {
        let mut out = Vec::new();
        let mut line = String::new();
        for word in s.split(' ') {
            let mut tryl = line.clone();
            if !tryl.is_empty() {
                tryl.push(' ');
            }
            tryl.push_str(word);
            if !line.is_empty() && width(self.c, &tryl, size, 400) > room {
                out.push(core::mem::take(&mut line));
                line.push_str(word);
            } else {
                line = tryl;
            }
        }
        if !line.is_empty() {
            out.push(line);
        }
        out
    }

    // ---- input -------------------------------------------------------------------------------

    fn after_input(&mut self) {
        self.last_input = ms();
        self.app.now = krate::time::now_millis();
        if let Some(view) = self.view.as_ref() {
            let view = EdView {
                area: view.area,
                x: view.x,
                width: view.width,
                sz: view.sz,
            };
            let c = self.c;
            if let Some(doc) = self.app.doc_mut() {
                if doc.reveal {
                    editor::reveal(&mut self.cache, c, doc, &view);
                }
            }
        }
    }

    fn editor_pos(&self, px: f32, py: f32) -> Option<(usize, crate::buffer::Pos)> {
        let doc = self.app.doc()?;
        let rows = &self.shown.rows;
        let first = rows.first()?;
        let mut pick = first;
        if py < first.1 {
            pick = first;
        } else {
            for row in rows {
                if py >= row.1 {
                    pick = row;
                }
            }
        }
        let (vi, y, l) = pick;
        if *vi == 0 {
            return Some((0, crate::buffer::Pos::new(0, 0)));
        }
        let n = vi - 1;
        let text = doc.line(n);
        let row = if py < *y { 0 } else { l.row_at(py - y) };
        let col = l.col_at(self.c, text, row, px - self.shown.x);
        Some((*vi, crate::buffer::Pos::new(n, col)))
    }

    fn press_editor(&mut self, px: f32, py: f32, m: Mods) {
        self.app.focus = Focus::Editor;
        let now_ms = ms();
        let (lt, lx, ly, ln) = self.last_click;
        let clicks =
            if now_ms.saturating_sub(lt) < 450 && (px - lx).abs() < 5.0 && (py - ly).abs() < 5.0 {
                (ln % 3) + 1
            } else {
                1
            };
        self.last_click = (now_ms, px, py, clicks);
        // Links, tags, checkboxes, folds and blocks drawn whole.
        if self.editor_press(px, py, m) {
            return;
        }
        let Some((_, pos)) = self.editor_pos(px, py) else {
            return;
        };
        if let Some(d) = self.app.doc_mut() {
            if d.mode == Mode::Reading {
                return;
            }
            match clicks {
                2 => {
                    let (a, b) = d.word_at(pos);
                    d.select_range(a, b);
                }
                3 => {
                    let end = if pos.line < d.buf.last_line() {
                        crate::buffer::Pos::new(pos.line + 1, 0)
                    } else {
                        crate::buffer::Pos::new(pos.line, d.buf.line_len(pos.line))
                    };
                    d.select_range(crate::buffer::Pos::new(pos.line, 0), end);
                }
                _ if m.alt && !m.shift && d.mode != Mode::Reading => d.add_caret(pos),
                // A press inside the selection starts dragging the text.
                _ if !m.shift && d.selection().is_some_and(|(a, b)| pos > a && pos < b) => {
                    self.cache.text_drag = true;
                }
                _ => {
                    d.extra.clear();
                    d.place(pos, m.shift);
                    self.drag = Drag::Select;
                }
            }
        }
        self.app.refresh_completion();
    }

    fn ed_view(&self) -> Option<EdView> {
        self.view.as_ref().map(|v| EdView {
            area: v.area,
            x: v.x,
            width: v.width,
            sz: v.sz,
        })
    }

    fn drag_step(&mut self) {
        let Some((px, py)) = pointer::position(self.win) else {
            return;
        };
        if !pointer::button_held(pointer::PointerButton::Primary) {
            self.release(px, py);
            return;
        }
        self.drag_to(px, py);
    }

    /// A drag goes on to (px, py): from the pointer's position each frame,
    /// or from a move event, so a drag follows however the moves arrive.
    fn drag_to(&mut self, px: f32, py: f32) {
        match self.drag.clone() {
            Drag::Select => {
                if let Some(view) = self.ed_view() {
                    let c = self.c;
                    let edge = if py < view.area.y {
                        py - view.area.y
                    } else if py > view.area.bottom() {
                        py - view.area.bottom()
                    } else {
                        0.0
                    };
                    if edge != 0.0 {
                        if let Some(d) = self.app.doc_mut() {
                            editor::scroll_by(&mut self.cache, c, d, &view, edge * 0.5);
                        }
                        self.draw();
                    }
                }
                if let Some((_, pos)) = self.editor_pos(px, py) {
                    if let Some(d) = self.app.doc_mut() {
                        if d.anchor.is_none() {
                            d.anchor = Some(d.cur);
                        }
                        d.cur = d.buf.clamp(pos);
                        d.goal_x = None;
                    }
                }
            }
            Drag::Thumb { grab } => {
                if let (Some(view), Some(track)) = (self.ed_view(), self.shown.track) {
                    let th = self.shown.thumb.map_or(30.0, |t| t.h);
                    let f = ((py - grab - track.y) / (track.h - th).max(1.0)).clamp(0.0, 1.0);
                    let c = self.c;
                    if let Some(d) = self.app.doc_mut() {
                        let visible = 30.0;
                        let total = editor::rows_total(d) as f32;
                        editor::scroll_to_fraction(
                            &mut self.cache,
                            c,
                            d,
                            &view,
                            f * (1.0 - (visible / total).min(1.0)),
                        );
                    }
                }
            }
            Drag::Left => {
                let rib = if self.app.settings.show_ribbon() {
                    self.sz.ribbon
                } else {
                    0.0
                };
                self.app.left_w = (px - rib).clamp(200.0, self.w * 0.45);
            }
            Drag::Right => {
                self.app.right_w = (self.w - px).clamp(200.0, self.w * 0.45);
            }
            Drag::Row { row, x, y, moved } => {
                if !moved && ((px - x).abs() > 4.0 || (py - y).abs() > 4.0) {
                    self.drag = Drag::Row {
                        row,
                        x,
                        y,
                        moved: true,
                    };
                }
            }
            Drag::PopRow {
                list,
                from,
                y,
                moved,
            } => {
                if !moved && (py - y).abs() > 4.0 {
                    self.drag = Drag::PopRow {
                        list,
                        from,
                        y,
                        moved: true,
                    };
                }
            }
            Drag::Card { row, x, y, moved } => {
                if !moved && ((px - x).abs() > 5.0 || (py - y).abs() > 5.0) {
                    self.drag = Drag::Card {
                        row,
                        x,
                        y,
                        moved: true,
                    };
                }
            }
            Drag::Tab {
                pane,
                tab,
                x,
                y,
                moved,
            } => {
                if !moved && ((px - x).abs() > 5.0 || (py - y).abs() > 5.0) {
                    self.drag = Drag::Tab {
                        pane,
                        tab,
                        x,
                        y,
                        moved: true,
                    };
                }
            }
            Drag::Divider(i) => {
                if let Some(d) = self.dividers.get(i).cloned() {
                    let pos = if d.down { py } else { px };
                    let id = self.in_window;
                    self.app
                        .window_layout_mut(id)
                        .drag(&d.path, d.after, d.whole, pos);
                }
            }
            Drag::Graph { .. } | Drag::Slider(_) | Drag::Canvas { .. } => self.view_drag(px, py),
            Drag::Scrub(p) => self.file_drag(p, px),
            Drag::PdfSel(p) => self.pdf_drag(p, px, py),
            Drag::None => {}
        }
    }

    fn release(&mut self, px: f32, py: f32) {
        // Selected text dragged in the note drops where the pointer is.
        if core::mem::take(&mut self.cache.text_drag) {
            let to = self.editor_pos(px, py).map(|(_, p)| p);
            let now = self.app.now;
            if let (Some(to), Some(d)) = (to, self.app.doc_mut()) {
                if !d.move_selection(to, now) {
                    d.place(to, false);
                }
            }
            return;
        }
        let drag = core::mem::replace(&mut self.drag, Drag::None);
        match drag {
            Drag::Row { row, moved, .. } => {
                if moved {
                    let onto = self
                        .tree_area
                        .and_then(|list| self.zone_at_rows(list, px, py));
                    let inside = self.tree_area.is_some_and(|l| l.contains(px, py));
                    if inside {
                        self.app.drop_on(row, onto);
                    }
                } else {
                    let before = self.app.active_path();
                    self.app.click(Hit::TreeRow(row), self.drag_mods);
                    if self.app.active_path() != before {
                        self.sheet = false;
                    }
                }
            }
            Drag::Tab {
                pane, tab, moved, ..
            } => {
                if !moved {
                    self.app.focus_pane(pane);
                    self.app.click(Hit::Tab(tab), self.drag_mods);
                    return;
                }
                self.drop_tab(pane, tab, px, py);
            }
            Drag::Graph { .. } | Drag::Canvas { .. } | Drag::Slider(_) => {
                self.view_release(drag, px, py);
            }
            Drag::PopRow {
                list, from, moved, ..
            } => {
                let target = self
                    .pop_rows
                    .iter()
                    .find(|(r, _)| py >= r.y && py < r.bottom())
                    .or_else(|| self.pop_rows.iter().rfind(|(r, _)| py >= r.y))
                    .or_else(|| self.pop_rows.first())
                    .map(|(_, i)| *i);
                if !moved {
                    if list == 1 {
                        if let Some(p) = self.pop_props.get(from).cloned() {
                            self.app.base_ui(crate::basemenu::BaseUi::Column(p));
                        }
                    }
                } else if let Some(to) = target {
                    self.app.base_act(if list == 0 {
                        crate::basemenu::BaseAct::SortMove(from, to)
                    } else {
                        crate::basemenu::BaseAct::ColumnMove(from, to)
                    });
                }
            }
            Drag::Card { row, moved, .. } => {
                if !moved {
                    self.app.click(Hit::BaseRow(row), self.drag_mods);
                } else if let Some(&(_, g)) =
                    self.board_cols.iter().find(|(r, _)| r.contains(px, py))
                {
                    // Dropped on a column: the note takes that column's value.
                    self.app.base_move_card(row, g);
                }
            }
            _ => {}
        }
    }

    /// A tab let go: onto a tab bar (to that place), or onto the edge of a
    /// group (a new group there).
    fn drop_tab(&mut self, from_pane: usize, tab: usize, px: f32, py: f32) {
        // Let go outside the window: a window of its own, as the notes app
        // does. (Onto another window's tab bar needs where the windows are,
        // which Krate cannot tell an app yet: K-1352.)
        if px < 0.0 || py < 0.0 || px > self.w || py > self.h {
            self.app.move_tab_to_new_window(from_pane, tab);
            return;
        }
        // Onto a tab bar.
        if let Some((p, _, strip)) = self
            .pane_areas
            .iter()
            .find(|(_, _, s)| s.contains(px, py))
            .cloned()
        {
            let mut at = self
                .tab_rects
                .iter()
                .filter(|(tp, _, _)| *tp == p)
                .filter(|(_, _, rc)| px > rc.x + rc.w * 0.5)
                .count();
            if p == from_pane && at > tab {
                at -= 1;
            }
            let _ = strip;
            self.app.move_tab(from_pane, tab, p, at);
            return;
        }
        // Onto the right or bottom part of a group's body: split there.
        if let Some((p, a, _)) = self
            .pane_areas
            .iter()
            .find(|(_, a, _)| a.contains(px, py))
            .cloned()
        {
            let right = px > a.x + a.w * 0.66;
            let down = py > a.y + a.h * 0.66;
            if right || down {
                self.app.split_with_tab(from_pane, tab, p, down && !right);
            } else if p != from_pane {
                let n = self.app.pane_tabs(p).0.len();
                self.app.move_tab(from_pane, tab, p, n);
            }
        }
    }

    fn press(&mut self, px: f32, py: f32, m: Mods) {
        self.drag_mods = m;
        self.app.pointer = (px, py);
        // A press outside an open base menu closes it.
        if let Some(pop) = self.base_pop {
            let inside = pop.contains(px, py)
                || matches!(self.zone_at(px, py), Some(Zone::App(Hit::BaseUi(_))))
                || self.app.overlay.is_some();
            if !inside {
                self.app.base_ui(crate::basemenu::BaseUi::Close);
            }
        }
        // A player's menu or a video filling the window (src/guest/widgets.rs).
        let window = look::r(0.0, 0.0, self.w, self.h);
        if widgets::media_overlay_press(
            &mut self.cache.env,
            self.win,
            &mut self.app,
            px,
            py,
            window,
        ) {
            return;
        }
        let Some(z) = self.zone_at(px, py) else {
            self.files.page_edit = None;
            return;
        };
        if !matches!(z, Zone::File(_, media::FileCtl::PageBox)) {
            self.files.page_edit = None;
        }
        if !matches!(
            z,
            Zone::File(_, media::FileCtl::More | media::FileCtl::MenuItem(_))
        ) {
            self.files.menu = None;
        }
        match z {
            Zone::Editor => {
                if self.app.explorer.rename.is_some() {
                    self.app.commit_rename();
                }
                self.press_editor(px, py, m)
            }
            Zone::LeftDivider => self.drag = Drag::Left,
            Zone::RightDivider => self.drag = Drag::Right,
            Zone::PaneDivider(i) => self.drag = Drag::Divider(i),
            Zone::Thumb => {
                let grab = self.shown.thumb.map_or(0.0, |t| py - t.y);
                self.drag = Drag::Thumb { grab };
            }
            Zone::Track => {
                let th = self.shown.thumb.map_or(30.0, |t| t.h);
                self.drag = Drag::Thumb { grab: th * 0.5 };
                self.drag_step();
            }
            Zone::TreeRow(i) => {
                if self.app.explorer.rename.is_some() {
                    self.app.click(Hit::TreeRow(i), m);
                    return;
                }
                self.drag = Drag::Row {
                    row: i,
                    x: px,
                    y: py,
                    moved: false,
                };
            }
            Zone::Tree => self.app.click(Hit::TreeBlank, m),
            Zone::PopRow(list, from) => {
                self.drag = Drag::PopRow {
                    list,
                    from,
                    y: py,
                    moved: false,
                };
            }
            Zone::BoardCard(row) => {
                self.drag = Drag::Card {
                    row,
                    x: px,
                    y: py,
                    moved: false,
                };
            }
            Zone::Tab(p, i) => {
                self.drag = Drag::Tab {
                    pane: p,
                    tab: i,
                    x: px,
                    y: py,
                    moved: false,
                };
            }
            Zone::Pane(p) => {
                self.app.focus_pane(p);
                self.app.focus = Focus::Editor;
            }
            Zone::Graph(_)
            | Zone::GraphPanel
            | Zone::GraphSlider(_)
            | Zone::Canvas(_)
            | Zone::Settings => {
                self.view_press(z, px, py, m);
            }
            Zone::File(p, ctl) => self.file_press(p, ctl, px, py),
            Zone::App(h) => {
                // A press in a group not in focus focuses it first.
                if let Some((p, _, _)) = self
                    .pane_areas
                    .iter()
                    .find(|(_, a, _)| a.contains(px, py))
                    .copied()
                {
                    if p != self.app.pane && self.app.overlay.is_none() {
                        self.app.focus_pane(p);
                    }
                }
                let phone = self.w < 600.0;
                if phone && matches!(h, Hit::Ribbon(Cmd::ToggleLeft)) {
                    self.sheet = !self.sheet;
                    return;
                }
                if phone && matches!(h, Hit::LeftTab(_) | Hit::Ribbon(Cmd::SearchVault)) {
                    self.sheet = true;
                }
                // The "..." in a view header opens the tab's menu.
                if h == Hit::Command(Cmd::TogglePin) {
                    let i = self.app.active;
                    self.app.tab_menu(i, px - 180.0, py + 14.0);
                    return;
                }
                let before = self.app.active_path();
                self.app.click(h, m);
                if phone && self.app.active_path() != before {
                    self.sheet = false;
                }
            }
        }
    }

    fn wheel(&mut self, x: f32, y: f32, dx: f32, dy: f32, m: Mods) {
        if let Some(Overlay::List(p)) = self.app.overlay.as_mut() {
            let n = p.items.len();
            if dy > 0.0 && p.sel + 1 < n {
                p.sel += 1;
            } else if dy < 0.0 && p.sel > 0 {
                p.sel -= 1;
            }
            return;
        }
        if matches!(self.app.overlay, Some(Overlay::Settings(_))) {
            self.app.settings_scroll = (self.app.settings_scroll + dy).max(0.0);
            return;
        }
        if self.app.overlay.is_some() {
            return;
        }
        if let Some((p, a)) = self
            .view_areas
            .iter()
            .find(|(_, a)| a.contains(x, y))
            .copied()
        {
            self.view_wheel(p, a, x, y, dx, dy, m);
            return;
        }
        if self.tree_area.is_some_and(|a| a.contains(x, y)) {
            self.app.explorer.scroll = (self.app.explorer.scroll + dy).max(0.0);
        } else if self.search_area.is_some_and(|a| a.contains(x, y)) {
            self.app.search_scroll = (self.app.search_scroll + dy).max(0.0);
        } else if self.right_area.is_some_and(|a| a.contains(x, y)) {
            self.app.right_scroll = (self.app.right_scroll + dy).max(0.0);
        } else if self.shown.area.is_some_and(|a| a.contains(x, y)) {
            if let Some(view) = self.ed_view() {
                let c = self.c;
                if let Some(d) = self.app.doc_mut() {
                    editor::scroll_by(&mut self.cache, c, d, &view, dy);
                }
            }
        } else if let Some((p, _, _)) = self
            .pane_areas
            .iter()
            .find(|(_, a, _)| a.contains(x, y))
            .copied()
        {
            let focused = p == self.app.pane;
            if let Some(View::Base(b)) = self.app.view_mut().filter(|_| focused) {
                b.scroll = (b.scroll + dy).max(0.0);
            }
        }
    }

    /// Up, Down, Page Up and Page Down in the note: they move through rows
    /// as drawn, so only the guest, which lays them out, can do them.
    fn vertical_key(&mut self, key: &str, m: Mods) -> bool {
        if self.app.overlay.is_some()
            || self.app.completion.is_some()
            || self.app.explorer.rename.is_some()
            || self.app.focus != Focus::Editor
            || m.primary
        {
            return false;
        }
        let Some(view) = self.ed_view() else {
            return false;
        };
        let c = self.c;
        let rows_page = ((view.area.h / (view.sz.body * 1.5)) as usize)
            .saturating_sub(2)
            .max(1);
        let Some(d) = self.app.doc_mut() else {
            return false;
        };
        match key {
            "ArrowUp" | "ArrowDown" => {
                if d.mode == Mode::Reading {
                    editor::scroll_by(
                        &mut self.cache,
                        c,
                        d,
                        &view,
                        if key == "ArrowUp" { -40.0 } else { 40.0 },
                    );
                } else {
                    editor::move_vertical(
                        &mut self.cache,
                        c,
                        d,
                        &view,
                        key == "ArrowDown",
                        1,
                        m.shift,
                    );
                }
            }
            "PageUp" | "PageDown" => {
                let down = key == "PageDown";
                editor::scroll_by(
                    &mut self.cache,
                    c,
                    d,
                    &view,
                    if down {
                        view.area.h * 0.9
                    } else {
                        -view.area.h * 0.9
                    },
                );
                if d.mode != Mode::Reading {
                    editor::move_vertical(&mut self.cache, c, d, &view, down, rows_page, m.shift);
                }
            }
            _ => return false,
        }
        true
    }

    /// Take in the web pages that have arrived for link cards.
    fn poll_fetches(&mut self) -> bool {
        if self.fetches.is_empty() {
            return false;
        }
        let mut done = false;
        let list = core::mem::take(&mut self.fetches);
        for (url, h) in list {
            match krate::net::poll(h) {
                krate::net::FetchStatus::Pending => self.fetches.push((url, h)),
                krate::net::FetchStatus::Ready(res) => {
                    let html = String::from_utf8_lossy(&res.body);
                    let p = crate::canvas::preview(&html, &url);
                    self.app.previews.insert(url, Some(Ok(p)));
                    done = true;
                }
                _ => {
                    self.app
                        .previews
                        .insert(url, Some(Err(String::from("The page could not be read"))));
                    done = true;
                }
            }
        }
        done
    }

    fn host_requests(&mut self) -> bool {
        let reqs = core::mem::take(&mut self.app.host);
        let any = !reqs.is_empty();
        for req in reqs {
            match req {
                HostRequest::PickVault { create } => {
                    let title = if create {
                        "Choose a folder for the new vault"
                    } else {
                        "Open a folder as a vault"
                    };
                    match krate::ui::dialog::open_folder(self.win, title) {
                        Ok(Some(folder)) => {
                            remember_vault(&folder.token, &folder.name);
                            let fs = disk::DiskFs::picked(&folder.token);
                            let mut v = Vault::open(&folder.name, Box::new(fs));
                            if create && v.note_count() == 0 {
                                let _ = v.create_note("", "Welcome", WELCOME);
                            }
                            self.cache.clear();
                            self.app.open_vault(v);
                            self.after_vault();
                            if create {
                                self.app.open("Welcome.md", false);
                            }
                        }
                        Ok(None) => {}
                        Err(_) => self
                            .app
                            .say("Krate did not let this app open a folder here"),
                    }
                }
                HostRequest::Sample => {
                    self.cache.clear();
                    self.app.open_vault(crate::sample::vault());
                    self.after_vault();
                    self.app.open("Start here.md", false);
                    self.app
                        .say("The sample vault is kept in memory: changes go when Vault closes");
                }
                HostRequest::Copy(s) => {
                    let _ = krate::ui::clipboard::write_text(&s);
                }
                HostRequest::Attach => {
                    match krate::ui::dialog::open_file(self.win, "Insert attachment", "") {
                        Ok(Some(chosen)) => match krate::fs::read_chosen(&chosen.token) {
                            Ok(bytes) => self.app.attach(&chosen.name, &bytes),
                            Err(_) => self.app.say("That file could not be read"),
                        },
                        Ok(None) => {}
                        Err(_) => self.app.say("Krate did not let this app open a file here"),
                    }
                }
                HostRequest::Fetch(url) => {
                    let req = krate::net::Request {
                        method: krate::net::HttpMethod::Get,
                        url: url.clone(),
                        headers: Vec::new(),
                        body: Vec::new(),
                        timeout_millis: Some(10_000),
                    };
                    match krate::net::begin(req) {
                        Ok(h) => self.fetches.push((url, h)),
                        Err(_) => {
                            self.app.previews.insert(
                                url,
                                Some(Err(String::from(
                                    "Krate did not let this app read web pages",
                                ))),
                            );
                        }
                    }
                }
            }
        }
        any
    }

    /// A vault was opened: its saved snapshots come in, and today's daily
    /// note opens if the vault asks for that.
    fn after_vault(&mut self) {
        self.app.local = local_now();
        load_recovery(&mut self.app);
        if self.app.settings.plugin_on("daily-notes")
            && self.app.settings.daily.bool_or("autorun", false)
        {
            self.app.run(Cmd::DailyNote);
        }
    }
}

const WELCOME: &str = "# Welcome\n\nThis is your new vault. Every note is a Markdown file in this folder.\n\n- Press Cmd+N for a new note, Cmd+O to find one.\n- Link notes with [[double brackets]].\n";

/// Read `key=value` from the run's arguments.
fn arg<'a>(raw: &'a str, key: &str) -> Option<&'a str> {
    raw.split('\n')
        .find_map(|a| a.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
}

/// The local date and time, when Krate tells this app the time zone.
fn local_now() -> crate::dates::When {
    let l = krate::locale::local_now();
    crate::dates::When {
        year: l.year,
        month: l.month,
        day: l.day,
        hour: l.hour,
        minute: l.minute,
        second: l.second,
    }
}

/// Snapshots are kept in the app's own store, one key per note, never in
/// the vault.
fn recovery_key(vault: &str, path: &str) -> String {
    let mut k = String::from("recovery/");
    k.push_str(vault);
    k.push('/');
    k.push_str(path);
    k
}

fn load_recovery(app: &mut App) {
    let Some(name) = app.vault.as_ref().map(|v| v.name.clone()) else {
        return;
    };
    let Some(paths) = app
        .vault
        .as_ref()
        .map(|v| v.notes.iter().map(|n| n.path.clone()).collect::<Vec<_>>())
    else {
        return;
    };
    let mut index = recovery_key(&name, "");
    index.push_str("\u{0}index");
    let listed: Vec<String> = store_kv::get(&index)
        .ok()
        .flatten()
        .and_then(|b| String::from_utf8(b).ok())
        .map(|s| {
            s.split('\n')
                .filter(|p| !p.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    for p in listed.iter().filter(|p| paths.contains(p)) {
        if let Some(text) = store_kv::get(&recovery_key(&name, p))
            .ok()
            .flatten()
            .and_then(|b| String::from_utf8(b).ok())
        {
            app.recovery.load(p, &text);
        }
    }
    let now = app.now;
    let keep = app.settings.recovery_keep_days();
    app.recovery.prune(now, keep);
}

fn save_recovery(app: &mut App) {
    if app.recovery.dirty.is_empty() {
        return;
    }
    let Some(name) = app.vault.as_ref().map(|v| v.name.clone()) else {
        return;
    };
    let dirty = core::mem::take(&mut app.recovery.dirty);
    for p in &dirty {
        let key = recovery_key(&name, p);
        if app.recovery.snapshots(p).is_empty() {
            let _ = store_kv::delete(&key);
            continue;
        }
        // A value holds 1 MiB: the oldest copies go first if a note's
        // history grows past that.
        let mut text = app.recovery.to_text(p);
        while text.len() > 900_000 {
            match app.recovery.files.get_mut(p) {
                Some(list) if list.len() > 1 => {
                    list.remove(0);
                    text = app.recovery.to_text(p);
                }
                _ => break,
            }
        }
        let _ = store_kv::set(&key, text.as_bytes());
    }
    let mut index = recovery_key(&name, "");
    index.push_str("\u{0}index");
    let all: Vec<&str> = app.recovery.files.keys().map(String::as_str).collect();
    let _ = store_kv::set(&index, all.join("\n").as_bytes());
}

struct Component;

impl krate::Guest for Component {
    // Key handling matches an event, then checks inside the arm on purpose,
    // as Krate Code does: a guard that fails would fall to a later arm.
    #[allow(clippy::collapsible_match)]
    fn run() -> i32 {
        let t0 = ms();
        let Ok(win) = window::create(
            "Vault",
            types::WindowSize {
                width: START_W,
                height: START_H,
            },
        ) else {
            return 30;
        };
        // The title bar is the app's own top row, as in the notes app.
        let _ = window::set_full_bleed(win, true);
        if window::show(win).is_err() {
            return 31;
        }
        if tree::set_root(win, &widget(ROOT_ID, None, types::WidgetKind::Stack)).is_err()
            || tree::upsert_node(
                win,
                &widget(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas),
            )
            .is_err()
        {
            let _ = window::close(win);
            return 32;
        }
        let Ok(c) = canvas2d::bind(win, CANVAS_ID) else {
            let _ = window::close(win);
            return 33;
        };
        let raw = krate::io::args::raw();
        let mut ui = Ui {
            win,
            c,
            app: App::new(),
            cache: LayCache::default(),
            shown: Shown::default(),
            hits: Vec::new(),
            said: Said::new(),
            sz: Sizes::new(1.0),
            view: None,
            tree_area: None,
            search_area: None,
            right_area: None,
            list_area: None,
            settings_area: None,
            pane_areas: Vec::new(),
            dividers: Vec::new(),
            tab_rects: Vec::new(),
            view_areas: Vec::new(),
            row_h: 27.0,
            hover: None,
            drag: Drag::None,
            drag_mods: Mods::default(),
            last_click: (0, 0.0, 0.0, 0),
            last_input: ms(),
            caret_xy: None,
            title: String::new(),
            frames: 0,
            w: START_W as f32,
            h: START_H as f32,
            drop_text: None,
            paste_next: false,
            trace: raw.split('\n').any(|a| a == "trace"),
            sheet: false,
            t0,
            first_frame_done: false,
            index_done_said: false,
            graph_img: views::GraphImage::default(),
            files: media::Files::default(),
            extra: Vec::new(),
            overlay_in: 0,
            in_window: 0,
            base_anchors: Vec::new(),
            base_pop: None,
            board_cols: Vec::new(),
            card_docs: alloc::collections::BTreeMap::new(),
            slides_on: false,
            pop_rows: Vec::new(),
            pop_props: Vec::new(),
            unseen: false,
            hover_point: None,
            block_count: None,
            fetches: Vec::new(),
        };
        ui.app.now = krate::time::now_millis();
        ui.app.local = local_now();
        ui.app.system_dark = Look::now().dark;
        ui.mark("t_window_ms");
        if raw.split('\n').next() == Some("quick") {
            let out = crate::sample::quick(&mut ui.app);
            ui.draw();
            let _ = krate::io::stdio::write(out.as_bytes());
            let _ = window::close(win);
            return 0;
        }
        // `vault=./vault`: open a folder inside the app's own folder (an
        // automated run; needs the optional fs grants). `sample` opens the
        // sample vault. `open=<note path>` opens a note in it.
        if let Some(path) = arg(&raw, "vault") {
            let name = String::from(crate::vfs::file_name(path.trim_end_matches('/')));
            let v = Vault::open(
                if name.is_empty() || name == "." {
                    "vault"
                } else {
                    &name
                },
                Box::new(disk::DiskFs::at(path)),
            );
            ui.mark("t_vault_listed_ms");
            ui.app.open_vault(v);
            ui.after_vault();
        } else if raw.split('\n').any(|a| a == "sample") {
            ui.app.open_vault(crate::sample::vault());
            ui.after_vault();
        } else if let Some((token, name)) = last_vault() {
            // Reopen the vault from last time, as a notes app does.
            let v = Vault::open(&name, Box::new(disk::DiskFs::picked(&token)));
            ui.mark("t_vault_listed_ms");
            ui.app.open_vault(v);
            ui.after_vault();
        }
        if let Some(note) = arg(&raw, "open") {
            if ui.app.vault.is_some() {
                ui.app.open_file(note, false);
                ui.mark("t_note_open_ms");
            }
        }
        // `also=<path>` opens more files, each in a new tab.
        for line in raw.split('\n') {
            if let Some(p) = line.strip_prefix("also=") {
                if ui.app.vault.is_some() {
                    ui.app.open_file(p, true);
                }
            }
        }
        if let Some(q) = arg(&raw, "search") {
            ui.app.search = crate::app::Input::with(q);
            ui.app.run_search();
            ui.app.left = LeftPane::Search;
        }
        // `preview=<link>`: show the page preview for a link, as Cmd-hover
        // does (a headless run has no pointer to hover with).
        if let Some(target) = arg(&raw, "preview") {
            ui.cache.preview = Some(widgets::Preview {
                target: String::from(target),
                x: 520.0,
                y: 300.0,
                rect: None,
            });
        }
        if raw.split('\n').any(|a| a == "reading") {
            ui.app.run(Cmd::ToggleReading);
        }
        if raw.split('\n').any(|a| a == "light") {
            ui.app.dark = false;
        }
        if raw.split('\n').any(|a| a == "right") {
            ui.app.right_open = true;
        }
        if let Some(cmd) = arg(&raw, "show") {
            for one in cmd.split(',') {
                match one {
                    "switcher" => ui.app.run(Cmd::QuickSwitcher),
                    "palette" => ui.app.run(Cmd::Palette),
                    "outline" => ui.app.run(Cmd::ShowOutline),
                    "tags" => ui.app.run(Cmd::ShowTags),
                    "outgoing" => ui.app.run(Cmd::ShowOutgoing),
                    "backlinks" => ui.app.run(Cmd::ShowBacklinks),
                    "unlinked" => {
                        ui.app.run(Cmd::ShowBacklinks);
                        ui.app.unlinked_open = true;
                    }
                    "properties" => ui.app.run(Cmd::ShowProperties),
                    "bookmarks" => ui.app.run(Cmd::ShowBookmarks),
                    "graph" => ui.app.run(Cmd::Graph),
                    "localgraph" => ui.app.run(Cmd::LocalGraph),
                    "settings" => ui.app.run(Cmd::Settings),
                    "plugins" => {
                        ui.app.run(Cmd::Settings);
                        ui.app.click(
                            Hit::SettingsNav(crate::app::SettingsPage::CorePlugins),
                            Mods::default(),
                        );
                    }
                    "appearance" => {
                        ui.app.run(Cmd::Settings);
                        ui.app.click(
                            Hit::SettingsNav(crate::app::SettingsPage::Appearance),
                            Mods::default(),
                        );
                    }
                    "splitright" => ui.app.run(Cmd::SplitRight),
                    "splitdown" => ui.app.run(Cmd::SplitDown),
                    "daily" => ui.app.run(Cmd::DailyNote),
                    "recovery" => ui.app.run(Cmd::Recovery),
                    "ribbon" => ui.app.run(Cmd::ToggleRibbon),
                    "newtab" => ui.app.run(Cmd::NewTab),
                    "pin" => ui.app.run(Cmd::TogglePin),
                    "stack" => ui.app.run(Cmd::StackTabs),
                    "basesort" => ui
                        .app
                        .base_ui(crate::basemenu::BaseUi::Tool(crate::basemenu::Panel::Sort)),
                    "basegroup" => ui
                        .app
                        .base_ui(crate::basemenu::BaseUi::Tool(crate::basemenu::Panel::Group)),
                    "basefilter" => ui.app.base_ui(crate::basemenu::BaseUi::Tool(
                        crate::basemenu::Panel::Filter,
                    )),
                    "baseprops" => ui.app.base_ui(crate::basemenu::BaseUi::Tool(
                        crate::basemenu::Panel::Properties,
                    )),
                    "basesearch" => ui.app.base_ui(crate::basemenu::BaseUi::Search),
                    "newwindow" => ui.app.run(Cmd::OpenInNewWindow),
                    "slides" => ui.app.run(Cmd::StartPresentation),
                    "movewindow" => ui.app.run(Cmd::MoveToNewWindow),
                    "pdfthumbs" => ui.app.menu_action(crate::app::MenuAction::PdfThumbs),
                    "loop" => ui.app.menu_action(crate::app::MenuAction::MediaLoop),
                    "outlinehere" => ui.app.run(Cmd::OpenOutlineHere),
                    "backlinkshere" => ui.app.run(Cmd::OpenBacklinksHere),
                    "hotkeys" => {
                        ui.app.run(Cmd::Settings);
                        ui.app.click(
                            Hit::SettingsNav(crate::app::SettingsPage::Hotkeys),
                            Mods::default(),
                        );
                    }
                    "files" => {
                        ui.app.run(Cmd::Settings);
                        ui.app.click(
                            Hit::SettingsNav(crate::app::SettingsPage::Files),
                            Mods::default(),
                        );
                    }
                    _ => {}
                }
            }
        }
        // `pdfsel=<page>,<from>,<to>`: words selected in the PDF tab, for a
        // headless picture (a check task cannot drag across them).
        if let Some(v) = arg(&raw, "pdfsel") {
            let n: Vec<usize> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            if let (Some((_, path)), [p, a, b]) = (ui.pdf_in_focus(), n.as_slice()) {
                ui.files.sel = Some(pdftext::Sel {
                    path,
                    a: (*p as u32, *a),
                    b: (*p as u32, *b),
                    shown: true,
                });
            }
        }
        // `cardedit=<id>`: a canvas card being edited in place, as a double
        // press starts it (a check task cannot double-click: K-1362).
        if let Some(id) = arg(&raw, "cardedit") {
            ui.app.canvas_live_start(id);
        }
        // `view=<name>`: a base tab shows its view of that name.
        if let Some(name) = arg(&raw, "view") {
            if let Some(View::Base(b)) = ui.app.view_mut() {
                if let Some(i) = b.base.views.iter().position(|v| v.name == name) {
                    b.view = i;
                    b.generation = 0;
                }
            }
        }
        // `page=<n>`: a PDF tab goes to that page; `zoom=<percent>` zooms it.
        if let Some(n) = arg(&raw, "page").and_then(|n| n.parse::<u32>().ok()) {
            if let Some(View::File(f)) = ui.app.view_mut() {
                f.goto = Some(n);
            }
        }
        if let Some(z) = arg(&raw, "zoom").and_then(|n| n.parse::<i32>().ok()) {
            ui.app.menu_action(crate::app::MenuAction::PdfZoom(z));
        }
        let _ = krate::ui::text_entry::start(win);
        // A headless picture of a graph waits for its layout to settle.
        if raw.split('\n').any(|a| a == "settle") {
            for _ in 0..400 {
                let start = krate::time::monotonic_nanos();
                let mut more = || krate::time::monotonic_nanos().saturating_sub(start) < 50_000_000;
                let worked = ui.app.tick(&mut more);
                ui.host_requests();
                ui.poll_fetches();
                if !worked && !ui.app.busy() && ui.fetches.is_empty() {
                    break;
                }
                if !ui.fetches.is_empty() {
                    krate::time::sleep_millis(25);
                }
            }
        }
        ui.sync_windows();
        // `shootwindow`: a headless picture of the first window of its own,
        // drawn by the same code into the main window's first frame (the
        // runtime shoots the first window).
        if raw.split('\n').any(|a| a == "shootwindow") && !ui.extra.is_empty() {
            ui.in_window = ui.extra[0].id;
            ui.draw_window();
            ui.in_window = 0;
        }
        ui.draw();
        ui.draw_windows();
        let mut last_watch = ms();
        let mut last_clock = ms();
        loop {
            // How long to sleep: not at all while there is work, until the
            // next thing that is due otherwise, forever when nothing is.
            let now = ms();
            let mut timeout: Option<u64> = None;
            let mut due = |t: u64| timeout = Some(timeout.map_or(t, |x: u64| x.min(t)));
            if ui.drag != Drag::None {
                due(16);
            }
            if !ui.fetches.is_empty() {
                due(50);
            }
            if ui.app.busy() || ui.views_busy() {
                due(0);
            }
            // A playing sound or video: its picture and clock, each frame.
            if ui.files_busy() {
                due(16);
            }
            if ui.app.focus == Focus::Editor
                && ui.app.doc().is_some_and(|d| d.mode != Mode::Reading)
                && ui.app.overlay.is_none()
            {
                let since = now.saturating_sub(ui.last_input);
                due(BLINK_MS - since % BLINK_MS);
            }
            if let Some(d) = ui.app.next_due() {
                due(d);
            }
            // A playing picture's next frame (src/guest/widgets.rs).
            if let Some(d) = widgets::anim_due(&ui.cache) {
                due(d);
            }
            if ui.app.vault.as_ref().is_some_and(|v| v.fs.real()) {
                due((last_watch + disk::WATCH_EVERY_MS).saturating_sub(now));
            }
            let mut ev = events::wait(timeout.map(|t| t.min(60_000) as u32));
            let mut changed = false;
            let mut close = false;
            while let Some(e) = ev {
                // An event from a window of its own is handled with that
                // window's state swapped in (src/guest/windows.rs).
                let from = match &e {
                    types::Event::Pointer(p) => Some(p.window),
                    types::Event::Key(k) => Some(k.window),
                    types::Event::Wheel(w) => Some(w.window),
                    types::Event::CloseRequested(w) | types::Event::RedrawRequested(w) => Some(*w),
                    _ => None,
                }
                .and_then(|w| ui.window_index(w));
                if let (Some(i), types::Event::CloseRequested(_)) = (from, &e) {
                    // Closing it closes its tabs; nothing goes back.
                    let id = ui.extra[i].id;
                    ui.app.close_floating(id);
                    changed = true;
                    ev = events::poll();
                    continue;
                }
                if let Some(i) = from {
                    ui.swap_window(i);
                }
                let had_overlay = ui.app.overlay.is_some();
                match e {
                    types::Event::CloseRequested(_) => close = true,
                    types::Event::Resized(_)
                    | types::Event::RedrawRequested(_)
                    | types::Event::ThemeChanged(_) => changed = true,
                    types::Event::Key(k)
                        if k.pressed
                            && (ui.app.focus == Focus::Editor || ui.files.fullscreen.is_some())
                            && ui.file_key(&k.key, mods(k.modifiers)) =>
                    {
                        if k.key == "Space" {
                            ui.drop_text = Some(' ');
                        }
                        // The letter of a Cmd chord comes as text too: drop it.
                        let m = mods(k.modifiers);
                        if m.primary && k.key.chars().count() == 1 {
                            ui.drop_text = k.key.chars().next().map(|c| c.to_ascii_lowercase());
                        }
                        ui.after_input();
                        changed = true;
                    }
                    types::Event::TextInput(s)
                        if ui.files.page_edit.is_some() || ui.files.find.is_some() =>
                    {
                        let drop = ui.drop_text.take();
                        let chord = drop.is_some_and(|d| {
                            s.chars().count() == 1 && s.to_lowercase().starts_with(d)
                        });
                        if !chord {
                            ui.file_text(&s);
                        }
                        changed = true;
                    }
                    types::Event::Key(k) => {
                        if k.pressed && widgets::media_key(&mut ui.cache.env, &k.key) {
                            // A player's menu or full screen took it.
                            changed = true;
                        } else if k.pressed {
                            let m = mods(k.modifiers);
                            let single = {
                                let mut cs = k.key.chars();
                                match (cs.next(), cs.next()) {
                                    (Some(ch), None) => Some(ch),
                                    _ => None,
                                }
                            };
                            if let (Some(ch), true) = (single, m.primary) {
                                let lc = ch.to_ascii_lowercase();
                                ui.drop_text = Some(lc);
                                if lc == 'v' && !m.shift {
                                    ui.paste_next = true;
                                } else {
                                    ui.app.now = krate::time::now_millis();
                                    ui.app.key(&k.key, m);
                                }
                                changed = true;
                            } else if single.is_none() || k.key == "Space" {
                                if k.key != "Space" {
                                    ui.drop_text = None;
                                    ui.paste_next = false;
                                    ui.app.now = krate::time::now_millis();
                                    if !ui.vertical_key(&k.key, m) {
                                        ui.app.key(&k.key, m);
                                    }
                                    changed = true;
                                }
                            } else if m.ctrl && single == Some('\t') {
                                ui.app.key("Tab", m);
                                changed = true;
                            }
                            ui.after_input();
                        }
                    }
                    types::Event::TextInput(s) => {
                        let drop = ui.drop_text.take();
                        if drop.is_some_and(|d| {
                            s.chars().count() == 1 && s.to_lowercase().starts_with(d)
                        }) {
                            // The letter of a Cmd chord, sent as text too.
                        } else if ui.paste_next {
                            ui.paste_next = false;
                            ui.app.now = krate::time::now_millis();
                            ui.app.text(&s.replace("\r\n", "\n"));
                        } else {
                            let clean: String = s
                                .chars()
                                .filter(|c| !c.is_control() || *c == '\n')
                                .collect();
                            if !clean.is_empty() {
                                ui.app.now = krate::time::now_millis();
                                ui.app.text(&clean);
                            }
                        }
                        ui.after_input();
                        changed = true;
                    }
                    types::Event::Pointer(p) => {
                        let m = mods(p.modifiers);
                        match (p.button, p.pressed) {
                            (Some(types::PointerButton::Primary), true) => {
                                ui.app.now = krate::time::now_millis();
                                ui.press(p.x, p.y, m);
                                ui.after_input();
                                changed = true;
                            }
                            (Some(types::PointerButton::Primary), false) => {
                                ui.release(p.x, p.y);
                                ui.after_input();
                                changed = true;
                            }
                            (Some(types::PointerButton::Secondary), true) => {
                                match ui.zone_at(p.x, p.y) {
                                    Some(Zone::Editor) => ui.editor_context(p.x, p.y),
                                    Some(Zone::TreeRow(i)) => ui.app.context(Some(i), p.x, p.y),
                                    Some(Zone::Tree) => ui.app.context(None, p.x, p.y),
                                    Some(Zone::Tab(pane, i)) => {
                                        ui.app.focus_pane(pane);
                                        ui.app.tab_menu(i, p.x, p.y);
                                    }
                                    Some(Zone::App(Hit::Bookmark(place))) => {
                                        ui.app.overlay = Some(Overlay::Menu(crate::app::Menu {
                                            x: p.x,
                                            y: p.y,
                                            items: alloc::vec![(
                                                String::from("Remove"),
                                                crate::app::MenuAction::RemoveBookmark(place)
                                            )],
                                        }));
                                    }
                                    Some(Zone::Canvas(_)) => ui.canvas_context(p.x, p.y),
                                    Some(Zone::File(pane, _)) => {
                                        if pane != media::FULL {
                                            ui.app.focus_pane(pane);
                                        }
                                        ui.player_context(p.x, p.y);
                                    }
                                    _ => {}
                                }
                                changed = true;
                            }
                            (None, _) => {
                                ui.hover_point = Some((p.x, p.y));
                                ui.files.stirred = krate::time::now_millis();
                                // A move while a drag is on carries it.
                                if ui.drag != Drag::None {
                                    ui.drag_to(p.x, p.y);
                                    changed = true;
                                }
                                let z = ui.zone_at(p.x, p.y);
                                if ui.graph_hover(p.x, p.y) {
                                    changed = true;
                                }
                                if z != ui.hover {
                                    ui.hover = z;
                                    changed = true;
                                }
                                if ui.editor_hover(p.x, p.y, m.primary) {
                                    changed = true;
                                }
                            }
                            _ => {}
                        }
                    }
                    types::Event::Wheel(wh) => {
                        ui.wheel(wh.x, wh.y, wh.dx, wh.dy, mods(wh.modifiers));
                        changed = true;
                    }
                    _ => {}
                }
                if !had_overlay && ui.app.overlay.is_some() {
                    ui.overlay_in = ui.in_window;
                }
                if let Some(i) = from {
                    ui.swap_window(i);
                }
                ev = events::poll();
            }
            if ui.sync_windows() {
                changed = true;
            }
            if close {
                break;
            }
            ui.app.now = krate::time::now_millis();
            if ms() >= last_clock + 30_000 {
                last_clock = ms();
                ui.app.local = local_now();
            }
            if ui.drag != Drag::None {
                ui.drag_step();
                changed = true;
            }
            if ui.host_requests() {
                changed = true;
            }
            if ui.poll_fetches() {
                changed = true;
            }
            // Background work, for at most 8 ms a frame.
            let start = krate::time::monotonic_nanos();
            let mut more = || krate::time::monotonic_nanos().saturating_sub(start) < 8_000_000;
            if ui.app.tick(&mut more) {
                changed = true;
            }
            save_recovery(&mut ui.app);
            if !ui.index_done_said
                && ui
                    .app
                    .vault
                    .as_ref()
                    .is_some_and(|v| v.index_pending() == 0 && v.note_count() > 0)
            {
                ui.index_done_said = true;
                ui.mark("t_index_done_ms");
            }
            if ui.app.vault.as_ref().is_some_and(|v| v.fs.real())
                && ms() >= last_watch + disk::WATCH_EVERY_MS
            {
                last_watch = ms();
                if ui.app.outside_changes() {
                    changed = true;
                }
            }
            // The caret blinks; the frame that flips it is a frame.
            if ui.app.focus == Focus::Editor && ui.app.doc().is_some() {
                changed = true;
            }
            if ui.views_busy() || ui.files_busy() {
                changed = true;
            }
            if changed {
                ui.draw();
                ui.draw_windows();
            }
        }
        ui.app.close();
        save_recovery(&mut ui.app);
        let _ = window::close(win);
        let mut out = ui.app.report();
        out.push_str("frames:");
        crate::text::push_num(&mut out, ui.frames as usize);
        out.push('\n');
        out.push_str("lines_laid_out:");
        crate::text::push_num(&mut out, ui.cache.misses as usize);
        out.push('\n');
        let _ = krate::io::stdio::write(out.as_bytes());
        0
    }
}

krate::export!(Component);

/// Where the vault the person picked last is remembered: its folder token
/// and display name. On a computer Krate keeps a picked folder for this app,
/// so the same token opens it on the next run with no dialog (K-1336).
const LAST_VAULT: &str = "last-vault";

fn remember_vault(token: &str, name: &str) {
    let mut v = String::from(token);
    v.push('\n');
    v.push_str(name);
    let _ = store_kv::set(LAST_VAULT, v.as_bytes());
}

/// The vault picked last time, if Krate still lets this app reach it: the
/// folder is there and the person has not taken it back.
fn last_vault() -> Option<(String, String)> {
    let bytes = store_kv::get(LAST_VAULT).ok()??;
    let text = String::from_utf8(bytes).ok()?;
    let (token, name) = text.split_once('\n')?;
    let mut root = String::from("picked/");
    root.push_str(token);
    krate::fs::list(&root).ok()?;
    Some((String::from(token), String::from(name)))
}
