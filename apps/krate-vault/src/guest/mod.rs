//! The guest: draws the [`App`] on one canvas and turns window events into
//! calls on it. Wasm only; everything it draws comes from the plain modules.

mod disk;
mod editor;
mod look;

use crate::app::{
    App, Cmd, Focus, Hit, HostRequest, LeftPane, ListKind, Mods, Overlay, RightPane, RowKind,
};
use crate::editor::Mode;
use crate::md::Target;
use crate::text::{count, grouped};
use crate::vault::Vault;
use crate::vfs::parent;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use editor::{LayCache, Shown, View};
use krate::gfx::canvas2d;
use krate::layout::{Look, Rect, Said};
#[allow(unused_imports)]
use krate::prelude::*;
use krate::ui::{events, pointer, tree, types, window};
use look::{fill, fit, icon, r, round, text, width, Icon, Sizes, Theme, DARK, LIGHT};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;
const START_W: u32 = 1280;
const START_H: u32 = 800;
const BLINK_MS: u64 = 600;

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
    view: Option<View>,
    tree_area: Option<Rect>,
    search_area: Option<Rect>,
    right_area: Option<Rect>,
    list_area: Option<Rect>,
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
        let size = canvas2d::canvas_size(self.c).unwrap_or(krate::gfx::types::Size {
            width: START_W as f32,
            height: START_H as f32,
        });
        let (w, h) = (size.width.max(320.0), size.height.max(240.0));
        self.w = w;
        self.h = h;
        let look = Look::now();
        self.sz = Sizes::new(look.text_scale);
        let t = self.t();
        self.hits.clear();
        self.said = Said::new();
        self.caret_xy = None;
        self.tree_area = None;
        self.search_area = None;
        self.right_area = None;
        self.list_area = None;
        let _ = canvas2d::clear(self.c, t.bg);
        if self.app.vault.is_none() {
            self.draw_welcome(w, h);
        } else {
            self.draw_workspace(w, h);
        }
        self.draw_overlay(w, h);
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
        if let Some(d) = self.app.doc() {
            want.push_str(&d.title);
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
            let my = 40.0 * k;
            for i in 0..3 {
                let o = i as f32 * 5.0 * k;
                round(
                    self.c,
                    r(mx + o, my - o, 24.0 * k, 30.0 * k),
                    4.0,
                    if i == 2 { t.accent } else { t.active },
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
            let b = r(x + col_w - bw, ry + (row_h - 34.0 * k) * 0.5, bw, 34.0 * k);
            let z = Zone::App(Hit::Welcome(i as u8));
            let primary = i == 1;
            let fillc = if primary {
                t.accent
            } else if self.hovered(&z) {
                t.hover
            } else {
                t.active
            };
            round(self.c, b, 6.0, fillc);
            let bt = width(self.c, btn, self.sz.ui, 600);
            text(
                self.c,
                btn,
                b.x + (b.w - bt) * 0.5,
                b.y + b.h * 0.5 + self.sz.ui * 0.36,
                self.sz.ui,
                600,
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
        let ribbon = sz.ribbon;
        let narrow = w < 640.0;
        let left_open = self.app.left_open && w >= 600.0;
        let right_open = self.app.right_open && w >= 900.0;
        let left_w = if left_open {
            self.app.left_w.clamp(180.0, (w * 0.4).max(180.0))
        } else {
            0.0
        };
        let right_w = if right_open {
            self.app.right_w.clamp(200.0, (w * 0.35).max(200.0))
        } else {
            0.0
        };
        let _ = narrow;
        // The ribbon.
        fill(self.c, r(0.0, 0.0, ribbon, h), t.chrome);
        fill(self.c, r(ribbon - 1.0, 0.0, 1.0, h), t.border);
        let ribbon_items: [(Icon, Cmd, &str); 5] = [
            (Icon::SidebarLeft, Cmd::ToggleLeft, "Toggle left sidebar"),
            (Icon::Switcher, Cmd::QuickSwitcher, "Open quick switcher"),
            (Icon::Search, Cmd::SearchVault, "Search all notes"),
            (Icon::Palette, Cmd::Palette, "Open command palette"),
            (Icon::NewNote, Cmd::NewNote, "Create new note"),
        ];
        for (i, (ic, cmd, label)) in ribbon_items.iter().enumerate() {
            let cy = 20.0 * sz.scale + i as f32 * 36.0 * sz.scale + if i > 0 { 8.0 } else { 0.0 };
            let b = r(6.0, cy - 15.0 * sz.scale, ribbon - 12.0, 30.0 * sz.scale);
            let z = Zone::App(Hit::Ribbon(*cmd));
            if self.hovered(&z) {
                round(self.c, b, 5.0, t.hover);
            }
            icon(self.c, *ic, ribbon * 0.5, cy, sz.scale, t.muted);
            self.button(b, label, Hit::Ribbon(*cmd));
        }
        {
            let cy = h - 24.0 * sz.scale;
            let b = r(6.0, cy - 15.0 * sz.scale, ribbon - 12.0, 30.0 * sz.scale);
            icon(self.c, Icon::Theme, ribbon * 0.5, cy, sz.scale, t.muted);
            self.button(b, "Switch between dark and light", Hit::Ribbon(Cmd::Theme));
        }
        let mut x = ribbon;
        if left_open {
            let area = r(x, 0.0, left_w, h);
            self.draw_left(area);
            x += left_w;
            fill(self.c, r(x - 1.0, 0.0, 1.0, h), t.border);
            self.hit(r(x - 4.0, 0.0, 8.0, h), Zone::LeftDivider);
        }
        let main_w = (w - x - right_w).max(120.0);
        let main = r(x, 0.0, main_w, h);
        if w < 600.0 && self.sheet {
            // A phone: the file tree or search is a sheet over the note.
            self.draw_left(main);
        } else {
            self.draw_main(main, right_open);
        }
        if right_open {
            let rx = x + main_w;
            fill(self.c, r(rx, 0.0, 1.0, h), t.border);
            self.draw_right(r(rx + 1.0, 0.0, right_w - 1.0, h));
            self.hit(r(rx - 4.0, 0.0, 8.0, h), Zone::RightDivider);
        }
        self.draw_status(w, h);
    }

    // ---- the left sidebar ------------------------------------------------------

    fn draw_left(&mut self, area: Rect) {
        let t = self.t();
        let sz = self.sz;
        fill(self.c, area, t.side);
        let head = r(area.x, 0.0, area.w, sz.head);
        let tabs: [(Icon, LeftPane, &str); 2] = [
            (Icon::Files, LeftPane::Files, "Files"),
            (Icon::Search, LeftPane::Search, "Search"),
        ];
        for (i, (ic, pane, label)) in tabs.iter().enumerate() {
            let b = r(
                head.x + 8.0 + i as f32 * 34.0 * sz.scale,
                6.0 * sz.scale,
                30.0 * sz.scale,
                28.0 * sz.scale,
            );
            if self.app.left == *pane {
                round(self.c, b, 5.0, t.active);
            }
            icon(
                self.c,
                *ic,
                b.x + b.w * 0.5,
                b.y + b.h * 0.5,
                sz.scale,
                if self.app.left == *pane {
                    t.text
                } else {
                    t.muted
                },
            );
            self.button(b, label, Hit::LeftTab(*pane));
        }
        fill(
            self.c,
            r(area.x, head.bottom() - 1.0, area.w, 1.0),
            t.border,
        );
        let body = r(area.x, head.bottom(), area.w, area.h - head.h);
        match self.app.left {
            LeftPane::Files => self.draw_files(body),
            LeftPane::Search => self.draw_search(body),
        }
    }

    fn draw_files(&mut self, area: Rect) {
        let t = self.t();
        let sz = self.sz;
        // The tree's own buttons.
        let bar = r(area.x, area.y, area.w, 34.0 * sz.scale);
        let acts: [(Icon, Cmd, &str); 3] = [
            (Icon::NewNote, Cmd::NewNote, "New note"),
            (Icon::NewFolder, Cmd::NewFolder, "New folder"),
            (Icon::Collapse, Cmd::CollapseAll, "Collapse all"),
        ];
        let total_w = acts.len() as f32 * 30.0 * sz.scale;
        for (i, (ic, cmd, label)) in acts.iter().enumerate() {
            let b = r(
                bar.x + (bar.w - total_w) * 0.5 + i as f32 * 30.0 * sz.scale,
                bar.y + 3.0,
                28.0 * sz.scale,
                28.0 * sz.scale,
            );
            let z = Zone::App(Hit::Command(*cmd));
            if self.hovered(&z) {
                round(self.c, b, 5.0, t.hover);
            }
            icon(
                self.c,
                *ic,
                b.x + b.w * 0.5,
                b.y + b.h * 0.5,
                sz.scale * 0.95,
                t.muted,
            );
            self.button(b, label, Hit::Command(*cmd));
        }
        let list = r(area.x, bar.bottom(), area.w, area.bottom() - bar.bottom());
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
        let active_path = self.app.doc().map(|d| d.path.clone());
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
        let mut y = list.y + 4.0 + first as f32 * row_h - scroll;
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
        for (k, (name, path, is_folder)) in names.iter().enumerate() {
            let i = first + k;
            if y > list.bottom() {
                break;
            }
            let Some(row) = rows.get(i) else {
                break;
            };
            let depth = row.depth as f32;
            let rr = r(list.x + 6.0, y, list.w - 12.0, row_h);
            let z = Zone::TreeRow(i);
            let is_active = active_path.as_deref() == Some(path.as_str());
            let is_sel = selected.as_deref() == Some(path.as_str());
            let is_drop = drop_row == Some(i) && *is_folder;
            if is_active || is_drop {
                round(self.c, rr, 4.0, t.active);
            } else if self.hovered(&z) || (is_sel && self.app.focus == Focus::Tree) {
                round(self.c, rr, 4.0, t.hover);
            }
            // Indent guides.
            for d in 0..row.depth {
                fill(
                    self.c,
                    r(
                        list.x + 18.0 * sz.scale + d as f32 * 16.0 * sz.scale,
                        y,
                        1.0,
                        row_h,
                    ),
                    t.border,
                );
            }
            let tx = list.x + 14.0 * sz.scale + depth * 16.0 * sz.scale;
            let base = y + row_h * 0.5 + sz.ui * 0.36;
            if *is_folder {
                let open = self.app.explorer.expanded.contains(path);
                look::chevron(
                    self.c,
                    tx + 4.0 * sz.scale,
                    y + row_h * 0.5,
                    open,
                    sz.scale,
                    t.faint,
                );
            }
            let nx = tx + 15.0 * sz.scale;
            let room = rr.right() - nx - 6.0;
            if rename.as_ref().is_some_and(|rn| rn.path == *path) {
                if let Some(rn) = rename.as_ref() {
                    let fbox = r(nx - 4.0, y + 2.0, room + 4.0, row_h - 4.0);
                    round(self.c, fbox, 4.0, t.field);
                    look::outline(self.c, fbox, 4.0, 1.0, t.accent);
                    self.draw_input(&rn.input, nx, base, sz.ui, t.text, fbox.right() - 4.0, true);
                    self.said.field(fbox, "Name", &rn.input.text);
                }
            } else {
                let col = if is_active || *is_folder {
                    t.text
                } else {
                    t.muted
                };
                fit(
                    self.c,
                    name,
                    nx,
                    base,
                    sz.ui,
                    if *is_folder { 500 } else { 400 },
                    col,
                    room,
                );
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
                list.x + 20.0,
                list.y + 30.0,
                sz.ui,
                400,
                t.faint,
            );
            self.said.text(r(list.x, list.y + 14.0, list.w, 22.0), msg);
        }
    }

    fn zone_at_rows(&self, list: Rect, px: f32, py: f32) -> Option<usize> {
        if !list.contains(px, py) {
            return None;
        }
        let i = ((py - list.y - 4.0 + self.app.explorer.scroll) / self.row_h.max(1.0)) as usize;
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
        let boxr = r(area.x + 10.0, area.y + 10.0, area.w - 20.0, 30.0 * sz.scale);
        round(self.c, boxr, 6.0, t.field);
        let focused = self.app.focus == Focus::Search;
        look::outline(
            self.c,
            boxr,
            6.0,
            1.0,
            if focused { t.accent } else { t.border },
        );
        icon(
            self.c,
            Icon::Search,
            boxr.x + 14.0,
            boxr.y + boxr.h * 0.5,
            sz.scale * 0.8,
            t.faint,
        );
        let base = boxr.y + boxr.h * 0.5 + sz.ui * 0.36;
        let inp = self.app.search.clone();
        if inp.text.is_empty() && !focused {
            text(
                self.c,
                "Search all notes...",
                boxr.x + 28.0,
                base,
                sz.ui,
                400,
                t.faint,
            );
        }
        self.draw_input(
            &inp,
            boxr.x + 28.0,
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
            let hint = "Words, \"exact phrases\", tag:#name, path:, file:";
            fit(
                self.c,
                hint,
                area.x + 14.0,
                y + 16.0,
                sz.small,
                400,
                t.faint,
                area.w - 28.0,
            );
            return;
        };
        let summary = if job.done {
            let mut s = count(job.notes_matched, "note", "notes");
            s.push_str(" \u{b7} ");
            s.push_str(&count(job.matches, "match", "matches"));
            s
        } else {
            let mut s = String::from("Searching ");
            s.push_str(&grouped(job.scanned()));
            s.push_str(" of ");
            s.push_str(&grouped(job.total));
            s.push_str("\u{2026}  ");
            s.push_str(&count(job.notes_matched, "note", "notes"));
            s
        };
        text(
            self.c,
            &summary,
            area.x + 14.0,
            y + 14.0,
            sz.small,
            400,
            t.muted,
        );
        self.said.text(r(area.x, y, area.w, 20.0), &summary);
        y += 24.0;
        let list = r(area.x, y, area.w, area.bottom() - y);
        self.search_area = Some(list);
        let row_h = sz.row;
        let line_h = (sz.row * 0.95).round();
        // Every result's height, so the list scrolls as one.
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
        let mut total = 0.0;
        for (i, res) in results.iter().enumerate() {
            total += row_h;
            if !paths
                .get(i)
                .is_some_and(|p| self.app.search_collapsed.contains(p))
            {
                total += res.3.len() as f32 * line_h + 6.0;
            }
        }
        let max = (total - list.h + 12.0).max(0.0);
        self.app.search_scroll = self.app.search_scroll.clamp(0.0, max);
        let _ = canvas2d::set_clip(self.c, list.x, list.y, list.w, list.h);
        let mut yy = list.y - self.app.search_scroll;
        for (i, (name, folder, n, lines)) in results.iter().enumerate() {
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
                    lines.len() as f32 * line_h + 6.0
                };
            if yy + block_h < list.y {
                yy += block_h;
                continue;
            }
            let hr = r(list.x + 6.0, yy, list.w - 12.0, row_h);
            let z = Zone::App(Hit::SearchFile(i));
            if self.hovered(&z) {
                round(self.c, hr, 4.0, t.hover);
            }
            let base = yy + row_h * 0.5 + sz.ui * 0.36;
            look::chevron(
                self.c,
                hr.x + 10.0,
                yy + row_h * 0.5,
                !collapsed,
                sz.scale,
                t.faint,
            );
            let badge = grouped(*n);
            let bw = width(self.c, &badge, sz.small, 500) + 12.0;
            let _ = folder;
            fit(
                self.c,
                name,
                hr.x + 20.0,
                base,
                sz.ui,
                600,
                t.text,
                hr.w - 32.0 - bw,
            );
            round(
                self.c,
                r(hr.right() - bw - 4.0, yy + (row_h - 18.0) * 0.5, bw, 18.0),
                9.0,
                t.active,
            );
            text(
                self.c,
                &badge,
                hr.right() - bw + 2.0,
                yy + row_h * 0.5 + sz.small * 0.36,
                sz.small,
                500,
                t.muted,
            );
            self.said.button(hr, name);
            self.hit(hr, z);
            yy += row_h;
            if !collapsed {
                for (l, (line, at)) in lines.iter().enumerate() {
                    let lr = r(list.x + 14.0, yy, list.w - 22.0, line_h);
                    let lz = Zone::App(Hit::SearchLine(i, l));
                    if self.hovered(&lz) {
                        round(self.c, lr, 4.0, t.hover);
                    }
                    let lb = yy + line_h * 0.5 + sz.small * 0.36;
                    let lx = lr.x + 8.0;
                    let pre = line.get(..at.0).unwrap_or("");
                    let mid = line.get(at.0..at.1).unwrap_or("");
                    let room = lr.w - 14.0;
                    let pw = width(self.c, pre, sz.small, 400);
                    if pw < room {
                        let mw = width(self.c, mid, sz.small, 600);
                        fill(
                            self.c,
                            r(
                                lx + pw - 1.0,
                                yy + 3.0,
                                (mw + 2.0).min(room - pw),
                                line_h - 6.0,
                            ),
                            t.find,
                        );
                    }
                    fit(self.c, line, lx, lb, sz.small, 400, t.muted, room);
                    self.said.button(lr, line);
                    self.hit(lr, lz);
                    yy += line_h;
                }
                yy += 6.0;
            }
        }
        let _ = canvas2d::clear_clip(self.c);
    }

    // ---- the middle: tabs, the view header, the note ------------------------------

    fn draw_main(&mut self, area: Rect, right_open: bool) {
        let t = self.t();
        let sz = self.sz;
        let strip = r(area.x, 0.0, area.w, sz.tab);
        fill(self.c, strip, t.chrome);
        fill(
            self.c,
            r(area.x, strip.bottom() - 1.0, area.w, 1.0),
            t.border,
        );
        let n = self.app.tabs.len().max(1);
        let right_btn = 40.0 * sz.scale;
        let avail = area.w - 16.0 - 36.0 * sz.scale - right_btn;
        let tab_w = (avail / n as f32).clamp(60.0, 210.0 * sz.scale);
        let titles: Vec<String> = self
            .app
            .tabs
            .iter()
            .map(|tb| String::from(tb.title()))
            .collect();
        let mut x = area.x + 8.0;
        for (i, title) in titles.iter().enumerate() {
            let tr = r(x, 6.0 * sz.scale, tab_w - 2.0, strip.h - 6.0 * sz.scale);
            let active = i == self.app.active;
            let z = Zone::App(Hit::Tab(i));
            if active {
                let _ = canvas2d::fill_round_rect(
                    self.c,
                    look::g(r(tr.x, tr.y, tr.w, tr.h + 1.0)),
                    krate::gfx::types::CornerRadii {
                        top_left: 7.0,
                        top_right: 7.0,
                        bottom_right: 0.0,
                        bottom_left: 0.0,
                    },
                    t.bg,
                );
            } else if self.hovered(&z) {
                round(self.c, r(tr.x, tr.y + 2.0, tr.w, tr.h - 6.0), 6.0, t.hover);
            } else if i + 1 != self.app.active && i + 1 < n {
                fill(
                    self.c,
                    r(tr.right(), tr.y + 8.0, 1.0, tr.h - 16.0),
                    t.border,
                );
            }
            let base = tr.y + tr.h * 0.5 + sz.ui * 0.36;
            let close_w = 22.0 * sz.scale;
            fit(
                self.c,
                title,
                tr.x + 12.0,
                base,
                sz.ui,
                if active { 500 } else { 400 },
                if active { t.text } else { t.muted },
                tr.w - 24.0 - close_w,
            );
            self.said.button(tr, title);
            self.hit(tr, z.clone());
            if active || self.hovered(&z) || self.hovered(&Zone::App(Hit::TabClose(i))) {
                let cb = r(
                    tr.right() - close_w - 4.0,
                    tr.y + (tr.h - close_w) * 0.5,
                    close_w,
                    close_w,
                );
                if self.hovered(&Zone::App(Hit::TabClose(i))) {
                    round(self.c, cb, 4.0, t.hover);
                }
                icon(
                    self.c,
                    Icon::Close,
                    cb.x + cb.w * 0.5,
                    cb.y + cb.h * 0.5,
                    sz.scale * 0.8,
                    t.muted,
                );
                let mut lbl = String::from("Close ");
                lbl.push_str(title);
                self.button(cb, &lbl, Hit::TabClose(i));
            }
            x += tab_w;
        }
        let pb = r(x + 4.0, 9.0 * sz.scale, 28.0 * sz.scale, 26.0 * sz.scale);
        if self.hovered(&Zone::App(Hit::NewTab)) {
            round(self.c, pb, 5.0, t.hover);
        }
        icon(
            self.c,
            Icon::Plus,
            pb.x + pb.w * 0.5,
            pb.y + pb.h * 0.5,
            sz.scale * 0.9,
            t.muted,
        );
        self.button(pb, "New tab", Hit::NewTab);
        let rb = r(
            area.right() - right_btn + 4.0,
            8.0 * sz.scale,
            30.0 * sz.scale,
            28.0 * sz.scale,
        );
        icon(
            self.c,
            Icon::SidebarRight,
            rb.x + rb.w * 0.5,
            rb.y + rb.h * 0.5,
            sz.scale,
            if right_open { t.text } else { t.muted },
        );
        self.button(rb, "Toggle right sidebar", Hit::Ribbon(Cmd::ToggleRight));

        // The status bar has its own strip, so it never covers the note.
        let status_h = 24.0 * sz.scale;
        let body = r(area.x, strip.bottom(), area.w, area.h - strip.h - status_h);
        if self.app.doc().is_none() {
            self.view = None;
            self.shown.area = None;
            self.draw_empty(body);
            return;
        }
        // The view header: back and forward, where the note lives, its mode.
        let vh = r(body.x, body.y, body.w, sz.view_head);
        let can_back = self.app.can_navigate(false);
        let can_fwd = self.app.can_navigate(true);
        for (j, (fwd, ic, can, label)) in [
            (false, Icon::Back, can_back, "Navigate back"),
            (true, Icon::Forward, can_fwd, "Navigate forward"),
        ]
        .iter()
        .enumerate()
        {
            let b = r(
                vh.x + 8.0 + j as f32 * 30.0 * sz.scale,
                vh.y + 5.0,
                28.0 * sz.scale,
                vh.h - 10.0,
            );
            if *can && self.hovered(&Zone::App(Hit::Nav(*fwd))) {
                round(self.c, b, 5.0, t.hover);
            }
            icon(
                self.c,
                *ic,
                b.x + b.w * 0.5,
                b.y + b.h * 0.5,
                sz.scale * 0.9,
                if *can { t.muted } else { t.faint },
            );
            self.button(b, label, Hit::Nav(*fwd));
        }
        let (title, folder, mode) = {
            let d = self.app.doc();
            (
                d.map(|d| d.title.clone()).unwrap_or_default(),
                d.map(|d| String::from(parent(&d.path))).unwrap_or_default(),
                d.map_or(Mode::Live, |d| d.mode),
            )
        };
        let mut crumb = String::new();
        if !folder.is_empty() {
            crumb.push_str(&folder.replace('/', " / "));
            crumb.push_str(" / ");
        }
        let fw = width(self.c, &crumb, sz.ui, 400);
        let tw = width(self.c, &title, sz.ui, 500);
        let room = vh.w - 160.0 * sz.scale;
        let cx = vh.x + ((vh.w - (fw + tw).min(room)) * 0.5).max(70.0 * sz.scale);
        let base = vh.y + vh.h * 0.5 + sz.ui * 0.36;
        let used = fit(self.c, &crumb, cx, base, sz.ui, 400, t.faint, room * 0.6);
        fit(
            self.c,
            &title,
            cx + used,
            base,
            sz.ui,
            500,
            t.muted,
            room - used,
        );
        self.said.text(r(cx, vh.y, fw + tw, vh.h), &title);
        let mb = r(
            vh.right() - 40.0 * sz.scale,
            vh.y + 5.0,
            30.0 * sz.scale,
            vh.h - 10.0,
        );
        if self.hovered(&Zone::App(Hit::ModeToggle)) {
            round(self.c, mb, 5.0, t.hover);
        }
        let reading = mode == Mode::Reading;
        icon(
            self.c,
            if reading {
                Icon::Editing
            } else {
                Icon::Reading
            },
            mb.x + mb.w * 0.5,
            mb.y + mb.h * 0.5,
            sz.scale,
            t.muted,
        );
        self.button(
            mb,
            if reading {
                "Editing view"
            } else {
                "Reading view"
            },
            Hit::ModeToggle,
        );

        // The note.
        let ed = r(body.x, vh.bottom(), body.w, body.bottom() - vh.bottom());
        let pad = (28.0 * sz.scale).min(ed.w * 0.06);
        let col_w = sz.measure.min(ed.w - pad * 2.0).max(80.0);
        let col_x = ed.x + (ed.w - col_w) * 0.5;
        let view = View {
            area: ed,
            x: col_x,
            width: col_w,
            sz,
        };
        self.hit(ed, Zone::Editor);
        let caret_on = self.caret_visible();
        let app = &self.app;
        let resolved = |name: &str| -> bool { app.resolved(name) };
        if let Some(doc) = app.tabs.get(app.active).and_then(|tb| tb.doc.as_ref()) {
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
        }
        if let Some(thumb) = self.shown.thumb {
            self.hit(self.shown.track.unwrap_or(thumb), Zone::Track);
            self.hit(thumb, Zone::Thumb);
        }
        self.view = Some(view);
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

    fn draw_empty(&mut self, area: Rect) {
        let t = self.t();
        let sz = self.sz;
        let lines: [(&str, Option<Cmd>); 4] = [
            ("No note is open", None),
            ("Create new note (Cmd+N)", Some(Cmd::NewNote)),
            ("Go to a note (Cmd+O)", Some(Cmd::QuickSwitcher)),
            ("Close this tab", Some(Cmd::CloseTab)),
        ];
        let mut y = area.y + area.h * 0.38;
        for (i, (s, cmd)) in lines.iter().enumerate() {
            let size = if i == 0 {
                22.0 * sz.scale
            } else {
                sz.body * 0.95
            };
            let w = width(self.c, s, size, if i == 0 { 600 } else { 400 });
            let x = area.x + (area.w - w) * 0.5;
            text(
                self.c,
                s,
                x,
                y,
                size,
                if i == 0 { 600 } else { 400 },
                if cmd.is_some() { t.link } else { t.muted },
            );
            let rc = r(x - 4.0, y - size, w + 8.0, size * 1.4);
            match cmd {
                Some(c) => self.button(rc, s, Hit::Command(*c)),
                None => {
                    self.said.heading(rc, s);
                }
            }
            y += if i == 0 {
                44.0 * sz.scale
            } else {
                30.0 * sz.scale
            };
        }
    }

    // ---- the right sidebar ---------------------------------------------------------

    fn draw_right(&mut self, area: Rect) {
        let t = self.t();
        let sz = self.sz;
        fill(self.c, area, t.side);
        let panes: [(Icon, RightPane, &str); 4] = [
            (Icon::Backlinks, RightPane::Backlinks, "Backlinks"),
            (Icon::Outgoing, RightPane::Outgoing, "Outgoing links"),
            (Icon::Tags, RightPane::Tags, "Tags"),
            (Icon::Outline, RightPane::Outline, "Outline"),
        ];
        for (i, (ic, pane, label)) in panes.iter().enumerate() {
            let b = r(
                area.x + 8.0 + i as f32 * 34.0 * sz.scale,
                6.0 * sz.scale,
                30.0 * sz.scale,
                28.0 * sz.scale,
            );
            let on = self.app.right == *pane;
            if on {
                round(self.c, b, 5.0, t.active);
            }
            icon(
                self.c,
                *ic,
                b.x + b.w * 0.5,
                b.y + b.h * 0.5,
                sz.scale,
                if on { t.text } else { t.muted },
            );
            self.button(b, label, Hit::RightTab(*pane));
        }
        fill(self.c, r(area.x, sz.head - 1.0, area.w, 1.0), t.border);
        let body = r(area.x, sz.head, area.w, area.h - sz.head);
        self.right_area = Some(body);
        let title = match self.app.right {
            RightPane::Backlinks => "Backlinks",
            RightPane::Outgoing => "Outgoing links",
            RightPane::Tags => "Tags",
            RightPane::Outline => "Outline",
        };
        let mut head = String::from(title);
        if let Some(d) = self.app.doc() {
            if self.app.right != RightPane::Tags {
                head.push_str(" for ");
                head.push_str(&d.title);
            }
        }
        fit(
            self.c,
            &head,
            body.x + 14.0,
            body.y + 22.0,
            sz.ui,
            600,
            t.text,
            body.w - 28.0,
        );
        self.said.heading(r(body.x, body.y, body.w, 30.0), &head);
        let list = r(body.x, body.y + 34.0, body.w, body.h - 34.0);
        let _ = canvas2d::set_clip(self.c, list.x, list.y, list.w, list.h);
        let mut y = list.y - self.app.right_scroll;
        let row_h = sz.row;
        let pending = self.app.vault.as_ref().map_or(0, |v| v.index_pending());
        match self.app.right {
            RightPane::Backlinks => {
                let data: Vec<(String, Vec<String>)> = self
                    .app
                    .backlinks()
                    .iter()
                    .map(|b| {
                        let name =
                            String::from(crate::vfs::file_name(&b.path).trim_end_matches(".md"));
                        let ctx = if b.context.is_empty() {
                            b.lines.iter().map(|_| String::new()).collect()
                        } else {
                            b.context.clone()
                        };
                        (name, ctx)
                    })
                    .collect();
                let mut s = String::from("Linked mentions  ");
                s.push_str(&grouped(data.len()));
                text(self.c, &s, list.x + 14.0, y + 16.0, sz.small, 500, t.muted);
                self.said.text(r(list.x, y, list.w, 22.0), &s);
                y += 28.0;
                if data.is_empty() {
                    let msg = if pending > 0 {
                        "Still reading the vault\u{2026}"
                    } else {
                        "No backlinks found."
                    };
                    text(self.c, msg, list.x + 14.0, y + 14.0, sz.ui, 400, t.faint);
                    self.said.text(r(list.x, y, list.w, 22.0), msg);
                }
                for (i, (name, ctx)) in data.iter().enumerate() {
                    if y > list.bottom() {
                        break;
                    }
                    let nr = r(list.x + 6.0, y, list.w - 12.0, row_h);
                    let z = Zone::App(Hit::Backlink(i, 0));
                    if self.hovered(&z) {
                        round(self.c, nr, 4.0, t.hover);
                    }
                    fit(
                        self.c,
                        name,
                        nr.x + 8.0,
                        y + row_h * 0.5 + sz.ui * 0.36,
                        sz.ui,
                        600,
                        t.text,
                        nr.w - 16.0,
                    );
                    self.said.button(nr, name);
                    self.hit(nr, z);
                    y += row_h;
                    for (l, line) in ctx.iter().enumerate().take(4) {
                        if line.is_empty() {
                            continue;
                        }
                        let lr = r(list.x + 14.0, y, list.w - 22.0, row_h * 0.9);
                        let shown = crate::md::plain(line, 0);
                        round(
                            self.c,
                            lr,
                            4.0,
                            if self.hovered(&Zone::App(Hit::Backlink(i, l))) {
                                t.hover
                            } else {
                                t.bg
                            },
                        );
                        fit(
                            self.c,
                            &shown,
                            lr.x + 8.0,
                            y + lr.h * 0.5 + sz.small * 0.36,
                            sz.small,
                            400,
                            t.muted,
                            lr.w - 16.0,
                        );
                        self.said.button(lr, &shown);
                        self.hit(lr, Zone::App(Hit::Backlink(i, l)));
                        y += lr.h + 3.0;
                    }
                    y += 4.0;
                }
            }
            RightPane::Outgoing => {
                let data: Vec<(String, bool)> = self.app.outgoing().to_vec();
                if data.is_empty() {
                    text(
                        self.c,
                        "No links in this note.",
                        list.x + 14.0,
                        y + 16.0,
                        sz.ui,
                        400,
                        t.faint,
                    );
                }
                for (i, (name, ok)) in data.iter().enumerate() {
                    if y > list.bottom() {
                        break;
                    }
                    let nr = r(list.x + 6.0, y, list.w - 12.0, row_h);
                    let z = Zone::App(Hit::Outgoing(i));
                    if self.hovered(&z) {
                        round(self.c, nr, 4.0, t.hover);
                    }
                    fit(
                        self.c,
                        name,
                        nr.x + 8.0,
                        y + row_h * 0.5 + sz.ui * 0.36,
                        sz.ui,
                        400,
                        if *ok { t.text } else { t.faint },
                        nr.w - 16.0,
                    );
                    self.said.button(nr, name);
                    self.hit(nr, z);
                    y += row_h;
                }
            }
            RightPane::Tags => {
                let data: Vec<(String, usize)> = self.app.tags().to_vec();
                if data.is_empty() {
                    let msg = if pending > 0 {
                        "Still reading the vault\u{2026}"
                    } else {
                        "No tags found."
                    };
                    text(self.c, msg, list.x + 14.0, y + 16.0, sz.ui, 400, t.faint);
                }
                for (i, (tag, n)) in data.iter().enumerate() {
                    if y > list.bottom() {
                        break;
                    }
                    if y + row_h < list.y {
                        y += row_h;
                        continue;
                    }
                    let nr = r(list.x + 6.0, y, list.w - 12.0, row_h);
                    let z = Zone::App(Hit::Tag(i));
                    if self.hovered(&z) {
                        round(self.c, nr, 4.0, t.hover);
                    }
                    let mut s = String::from("#");
                    s.push_str(tag);
                    let num = grouped(*n);
                    let nw = width(self.c, &num, sz.small, 400);
                    fit(
                        self.c,
                        &s,
                        nr.x + 8.0,
                        y + row_h * 0.5 + sz.ui * 0.36,
                        sz.ui,
                        400,
                        t.text,
                        nr.w - nw - 24.0,
                    );
                    text(
                        self.c,
                        &num,
                        nr.right() - nw - 8.0,
                        y + row_h * 0.5 + sz.small * 0.36,
                        sz.small,
                        400,
                        t.faint,
                    );
                    self.said.button(nr, &s);
                    self.hit(nr, z);
                    y += row_h;
                }
            }
            RightPane::Outline => {
                let data: Vec<(u8, String)> = self
                    .app
                    .outline()
                    .iter()
                    .map(|h| (h.level, h.text.clone()))
                    .collect();
                if data.is_empty() {
                    text(
                        self.c,
                        "No headings in this note.",
                        list.x + 14.0,
                        y + 16.0,
                        sz.ui,
                        400,
                        t.faint,
                    );
                }
                for (i, (level, name)) in data.iter().enumerate() {
                    if y > list.bottom() {
                        break;
                    }
                    if y + row_h < list.y {
                        y += row_h;
                        continue;
                    }
                    let nr = r(list.x + 6.0, y, list.w - 12.0, row_h);
                    let z = Zone::App(Hit::Outline(i));
                    if self.hovered(&z) {
                        round(self.c, nr, 4.0, t.hover);
                    }
                    let ind = (level.saturating_sub(1)) as f32 * 14.0 * sz.scale;
                    fit(
                        self.c,
                        name,
                        nr.x + 8.0 + ind,
                        y + row_h * 0.5 + sz.ui * 0.36,
                        sz.ui,
                        400,
                        t.text,
                        nr.w - 16.0 - ind,
                    );
                    self.said.button(nr, name);
                    self.hit(nr, z);
                    y += row_h;
                }
            }
        }
        let _ = canvas2d::clear_clip(self.c);
    }

    // ---- the status bar ---------------------------------------------------------------

    fn draw_status(&mut self, w: f32, h: f32) {
        let t = self.t();
        let sz = self.sz;
        let mut parts: Vec<String> = Vec::new();
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
        if has_doc {
            let n = self.app.backlinks().len();
            parts.push(count(n, "backlink", "backlinks"));
            if let Some(d) = self.app.doc() {
                parts.push(count(d.words, "word", "words"));
                parts.push(count(d.chars, "character", "characters"));
                if d.save_error.is_some() {
                    parts.push(String::from("Not saved"));
                }
            }
        }
        if let Some((msg, _)) = self.app.status.as_ref() {
            parts.insert(0, msg.clone());
        }
        if parts.is_empty() {
            return;
        }
        let gap = 18.0 * sz.scale;
        let mut widths: Vec<f32> = parts
            .iter()
            .map(|p| width(self.c, p, sz.small, 400))
            .collect();
        let room = w - sz.ribbon - 8.0;
        let mut total: f32 = widths.iter().sum::<f32>() + gap * (parts.len() as f32 - 1.0) + 24.0;
        // A narrow window keeps the parts that fit, the first ones first.
        while total > room && parts.len() > 1 {
            parts.pop();
            widths.pop();
            total = widths.iter().sum::<f32>() + gap * (parts.len() as f32 - 1.0) + 24.0;
        }
        let bh = 24.0 * sz.scale;
        let bx = w - total;
        let by = h - bh;
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
        fill(self.c, r(bx + 8.0, by, total - 8.0, 1.0), t.border);
        let mut x = bx + 12.0;
        for (p, pw) in parts.iter().zip(widths) {
            text(
                self.c,
                p,
                x,
                by + bh * 0.5 + sz.small * 0.36,
                sz.small,
                400,
                t.muted,
            );
            self.said.text(r(x, by, pw, bh), p);
            x += pw + gap;
        }
    }

    // ---- overlays ------------------------------------------------------------------------

    fn draw_overlay(&mut self, w: f32, h: f32) {
        let t = self.t();
        let sz = self.sz;
        // Link completion under the caret.
        if self.app.overlay.is_none() {
            if let (Some((cx, cy)), Some(comp)) = (self.caret_xy, self.app.completion.as_ref()) {
                let items: Vec<(String, String)> = comp.items.clone();
                let sel = comp.sel;
                let rh = 30.0 * sz.scale;
                let cw = 320.0 * sz.scale;
                let shown = items.len().min(8);
                let ch = if shown == 0 { rh } else { rh * shown as f32 } + 8.0;
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
                round(self.c, card, 8.0, t.modal);
                look::outline(self.c, card, 8.0, 1.0, t.border);
                if items.is_empty() {
                    text(
                        self.c,
                        "No notes match. Keep typing to link a new one.",
                        x + 12.0,
                        y + 4.0 + rh * 0.5 + sz.small * 0.36,
                        sz.small,
                        400,
                        t.faint,
                    );
                }
                let first = sel.saturating_sub(shown.saturating_sub(1));
                for (k, (name, path)) in items.iter().enumerate().skip(first).take(shown) {
                    let ir = r(x + 4.0, y + 4.0 + (k - first) as f32 * rh, cw - 8.0, rh);
                    if k == sel {
                        round(self.c, ir, 5.0, t.active);
                    }
                    let nw = fit(
                        self.c,
                        name,
                        ir.x + 10.0,
                        ir.y + rh * 0.5 + sz.ui * 0.36,
                        sz.ui,
                        500,
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
                    self.button(ir, name, Hit::Completion(k));
                }
            }
            return;
        }
        // Everything else dims the window behind it.
        fill(self.c, r(0.0, 0.0, w, h), t.scrim);
        self.hit(r(0.0, 0.0, w, h), Zone::App(Hit::Scrim));
        let Some(ov) = self.app.overlay.as_mut() else {
            return;
        };
        match ov {
            Overlay::List(p) => {
                let pw = (700.0 * sz.scale).min(w - 24.0);
                let x = (w - pw) * 0.5;
                let y = (h * 0.12).min(90.0);
                let two_line = p.kind == ListKind::Switcher;
                let rh = if two_line {
                    44.0 * sz.scale
                } else {
                    36.0 * sz.scale
                };
                let in_h = 50.0 * sz.scale;
                let foot = 34.0 * sz.scale;
                let max_rows = (((h - y - in_h - foot - 24.0) / rh) as usize).clamp(1, 12);
                let shown = p.items.len().min(max_rows);
                if p.sel < p.first {
                    p.first = p.sel;
                }
                if p.sel >= p.first + max_rows {
                    p.first = p.sel + 1 - max_rows;
                }
                let card = r(x, y, pw, in_h + shown as f32 * rh + foot + 10.0);
                let items = p.items.clone();
                let (sel, first) = (p.sel, p.first);
                let input = p.input.clone();
                let placeholder = match &p.kind {
                    ListKind::Switcher => "Find or create a note...",
                    ListKind::Palette => "Select a command...",
                    ListKind::MoveTo(_) => "Choose a folder...",
                };
                let footer = match &p.kind {
                    ListKind::Switcher => "\u{2191}\u{2193} to navigate    Enter to open    Cmd+Enter in a new tab    Shift+Enter to create    esc to dismiss",
                    ListKind::Palette => "\u{2191}\u{2193} to navigate    Enter to run    esc to dismiss",
                    ListKind::MoveTo(_) => "\u{2191}\u{2193} to navigate    Enter to move    esc to dismiss",
                };
                let _ = canvas2d::drop_shadow_round_rect(
                    self.c,
                    look::g(card),
                    look::radii(10.0),
                    24.0,
                    look::alpha(look::hex(0), 0.45),
                );
                round(self.c, card, 10.0, t.modal);
                look::outline(self.c, card, 10.0, 1.0, t.border);
                self.list_area = Some(card);
                self.hit(card, Zone::App(Hit::ListBox));
                let base = y + in_h * 0.5 + sz.body * 0.36;
                if input.text.is_empty() {
                    text(self.c, placeholder, x + 18.0, base, sz.body, 400, t.faint);
                }
                self.draw_input(&input, x + 18.0, base, sz.body, t.text, x + pw - 18.0, true);
                self.said.field(r(x, y, pw, in_h), placeholder, &input.text);
                fill(self.c, r(x, y + in_h, pw, 1.0), t.border);
                for (k, item) in items.iter().enumerate().skip(first).take(shown) {
                    let ir = r(
                        x + 6.0,
                        y + in_h + 5.0 + (k - first) as f32 * rh,
                        pw - 12.0,
                        rh,
                    );
                    if k == sel {
                        round(self.c, ir, 6.0, t.active);
                    }
                    let hint_w = if item.hint.is_empty() {
                        0.0
                    } else {
                        width(self.c, &item.hint, sz.small, 500) + 16.0
                    };
                    if two_line {
                        fit(
                            self.c,
                            &item.title,
                            ir.x + 12.0,
                            ir.y + 19.0 * sz.scale,
                            sz.body * 0.94,
                            500,
                            t.text,
                            ir.w - 24.0 - hint_w,
                        );
                        if !item.detail.is_empty() {
                            fit(
                                self.c,
                                &item.detail,
                                ir.x + 12.0,
                                ir.y + 36.0 * sz.scale,
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
                            ir.y + rh * 0.5 + sz.body * 0.33,
                            sz.body * 0.94,
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
                        round(self.c, hb, 4.0, t.hover);
                        text(
                            self.c,
                            &item.hint,
                            hb.x + 8.0,
                            hb.y + 10.0 + sz.small * 0.36,
                            sz.small,
                            500,
                            t.muted,
                        );
                    }
                    self.button(ir, &item.title, Hit::ListItem(k));
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
                    t.faint,
                    pw - 32.0,
                );
            }
            Overlay::Menu(m) => {
                let items: Vec<String> = m.items.iter().map(|(l, _)| l.clone()).collect();
                let mw = 200.0 * sz.scale;
                let rh = 28.0 * sz.scale;
                let mh = items.len() as f32 * rh + 8.0;
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
                round(self.c, card, 8.0, t.modal);
                look::outline(self.c, card, 8.0, 1.0, t.border);
                for (i, label) in items.iter().enumerate() {
                    let ir = r(x + 4.0, y + 4.0 + i as f32 * rh, mw - 8.0, rh);
                    let z = Zone::App(Hit::MenuItem(i));
                    if self.hovered(&z) {
                        round(self.c, ir, 5.0, t.active);
                    }
                    let danger = label == "Delete";
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
                let cw = (440.0 * sz.scale).min(w - 24.0);
                let lines = self.wrap(&body, sz.ui, cw - 48.0);
                let ch = 70.0 * sz.scale + lines.len() as f32 * sz.ui * 1.5 + 60.0 * sz.scale;
                let x = (w - cw) * 0.5;
                let y = (h - ch) * 0.35;
                let card = r(x, y, cw, ch);
                let _ = canvas2d::drop_shadow_round_rect(
                    self.c,
                    look::g(card),
                    look::radii(10.0),
                    24.0,
                    look::alpha(look::hex(0), 0.45),
                );
                round(self.c, card, 10.0, t.modal);
                look::outline(self.c, card, 10.0, 1.0, t.border);
                self.hit(card, Zone::App(Hit::ListBox));
                text(
                    self.c,
                    &title,
                    x + 24.0,
                    y + 38.0 * sz.scale,
                    17.0 * sz.scale,
                    700,
                    t.text,
                );
                self.said.heading(r(x, y, cw, 50.0), &title);
                let mut ly = y + 70.0 * sz.scale;
                for l in &lines {
                    text(self.c, l, x + 24.0, ly, sz.ui, 400, t.muted);
                    ly += sz.ui * 1.5;
                }
                self.said.text(r(x, y + 50.0, cw, ly - y - 50.0), &body);
                let bw = 90.0 * sz.scale;
                let bh = 32.0 * sz.scale;
                let by = card.bottom() - bh - 18.0 * sz.scale;
                let yes_r = r(card.right() - 24.0 - bw, by, bw, bh);
                let no_r = r(yes_r.x - 10.0 - bw, by, bw, bh);
                round(self.c, yes_r, 6.0, t.danger);
                round(self.c, no_r, 6.0, t.active);
                let yw = width(self.c, yes, sz.ui, 600);
                text(
                    self.c,
                    yes,
                    yes_r.x + (bw - yw) * 0.5,
                    by + bh * 0.5 + sz.ui * 0.36,
                    sz.ui,
                    600,
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
            let view = View {
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
        // Links, tags and checkboxes in what is drawn.
        let mut follow: Option<Hit> = None;
        let mut toggle: Option<usize> = None;
        let mut tag: Option<String> = None;
        let mut url: Option<String> = None;
        if let Some(doc) = self.app.doc() {
            let mode = doc.mode;
            let active_line = doc.cur.line;
            if let Some((vi, y, l)) = self.shown.rows.iter().rev().find(|(_, y, _)| py >= *y) {
                if *vi == 0 && mode != Mode::Reading {
                    let path = doc.path.clone();
                    self.app.start_rename(&path);
                    return;
                }
                if *vi > 0 {
                    let n = vi - 1;
                    let text = doc.line(n);
                    let lx = px - self.shown.x;
                    let row = l.row_at(py - y);
                    if let editor::Deco::Task(_) = l.deco {
                        if lx < l.x0 && lx >= l.marker - 12.0 && row == 0 {
                            toggle = Some(n);
                        }
                    }
                    let on_line = n == active_line && mode != Mode::Reading;
                    if toggle.is_none() && mode != Mode::Source && (!on_line || m.primary) {
                        if let Some(run) = l.run_at(row, lx) {
                            match run.target {
                                Some(Target::Wiki(a, b)) => {
                                    follow = Some(Hit::LinkNote(String::from(
                                        text.get(a..b).unwrap_or(""),
                                    )));
                                }
                                Some(Target::Tag(a, b)) => {
                                    tag = Some(String::from(text.get(a + 1..b).unwrap_or("")))
                                }
                                Some(Target::Url(a, b)) => {
                                    url = Some(String::from(text.get(a..b).unwrap_or("")))
                                }
                                None => {}
                            }
                        }
                    }
                }
            }
        }
        if let Some(n) = toggle {
            let now = self.app.now;
            if let Some(d) = self.app.doc_mut() {
                d.toggle_task(n, now);
            }
            return;
        }
        if let Some(h) = follow {
            self.app.click(h, m);
            return;
        }
        if let Some(t) = tag {
            self.app.search_tag(&t);
            return;
        }
        if let Some(u) = url {
            if krate::ui::launcher::open_url(&u).is_err() {
                self.app.say("Krate did not let this app open web links");
            }
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
                _ => {
                    d.place(pos, m.shift);
                    self.drag = Drag::Select;
                }
            }
        }
        self.app.refresh_completion();
    }

    fn drag_step(&mut self) {
        let Some((px, py)) = pointer::position(self.win) else {
            return;
        };
        if !pointer::button_held(pointer::PointerButton::Primary) {
            self.release(px, py);
            return;
        }
        match self.drag.clone() {
            Drag::Select => {
                let view = self.view.as_ref().map(|v| View {
                    area: v.area,
                    x: v.x,
                    width: v.width,
                    sz: v.sz,
                });
                if let Some(view) = view {
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
                let view = self.view.as_ref().map(|v| View {
                    area: v.area,
                    x: v.x,
                    width: v.width,
                    sz: v.sz,
                });
                if let (Some(view), Some(track)) = (view, self.shown.track) {
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
                self.app.left_w = (px - self.sz.ribbon).clamp(180.0, self.w * 0.45);
            }
            Drag::Right => {
                self.app.right_w = (self.w - px).clamp(200.0, self.w * 0.4);
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
            Drag::None => {}
        }
    }

    fn release(&mut self, px: f32, py: f32) {
        let drag = core::mem::replace(&mut self.drag, Drag::None);
        if let Drag::Row { row, moved, .. } = drag {
            if moved {
                let onto = self
                    .tree_area
                    .and_then(|list| self.zone_at_rows(list, px, py));
                let inside = self.tree_area.is_some_and(|l| l.contains(px, py));
                if inside {
                    self.app.drop_on(row, onto);
                }
            } else {
                let before = self.app.doc().map(|d| d.path.clone());
                self.app.click(Hit::TreeRow(row), self.drag_mods);
                if self.app.doc().map(|d| d.path.clone()) != before {
                    self.sheet = false;
                }
            }
        }
    }

    fn press(&mut self, px: f32, py: f32, m: Mods) {
        self.drag_mods = m;
        let Some(z) = self.zone_at(px, py) else {
            return;
        };
        match z {
            Zone::Editor => {
                if self.app.explorer.rename.is_some() {
                    self.app.commit_rename();
                }
                self.press_editor(px, py, m)
            }
            Zone::LeftDivider => self.drag = Drag::Left,
            Zone::RightDivider => self.drag = Drag::Right,
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
            Zone::App(h) => {
                let phone = self.w < 600.0;
                if phone && matches!(h, Hit::Ribbon(Cmd::ToggleLeft)) {
                    self.sheet = !self.sheet;
                    return;
                }
                if phone && matches!(h, Hit::LeftTab(_) | Hit::Ribbon(Cmd::SearchVault)) {
                    self.sheet = true;
                }
                let before = self.app.doc().map(|d| d.path.clone());
                self.app.click(h, m);
                if phone && self.app.doc().map(|d| d.path.clone()) != before {
                    self.sheet = false;
                }
            }
        }
    }

    fn wheel(&mut self, x: f32, y: f32, dy: f32) {
        if let Some(Overlay::List(p)) = self.app.overlay.as_mut() {
            let n = p.items.len();
            if dy > 0.0 && p.sel + 1 < n {
                p.sel += 1;
            } else if dy < 0.0 && p.sel > 0 {
                p.sel -= 1;
            }
            return;
        }
        if self.app.overlay.is_some() {
            return;
        }
        if self.tree_area.is_some_and(|a| a.contains(x, y)) {
            self.app.explorer.scroll = (self.app.explorer.scroll + dy).max(0.0);
        } else if self.search_area.is_some_and(|a| a.contains(x, y)) {
            self.app.search_scroll = (self.app.search_scroll + dy).max(0.0);
        } else if self.right_area.is_some_and(|a| a.contains(x, y)) {
            self.app.right_scroll = (self.app.right_scroll + dy).max(0.0);
        } else if self.shown.area.is_some_and(|a| a.contains(x, y)) {
            let view = self.view.as_ref().map(|v| View {
                area: v.area,
                x: v.x,
                width: v.width,
                sz: v.sz,
            });
            if let Some(view) = view {
                let c = self.c;
                if let Some(d) = self.app.doc_mut() {
                    editor::scroll_by(&mut self.cache, c, d, &view, dy);
                }
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
        let Some(view) = self.view.as_ref().map(|v| View {
            area: v.area,
            x: v.x,
            width: v.width,
            sz: v.sz,
        }) else {
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
                            let fs = disk::DiskFs::picked(&folder.token);
                            let mut v = Vault::open(&folder.name, Box::new(fs));
                            if create && v.note_count() == 0 {
                                let _ = v.create_note("", "Welcome", WELCOME);
                            }
                            self.cache.clear();
                            self.app.open_vault(v);
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
                    self.app.open("Start here.md", false);
                    self.app
                        .say("The sample vault is kept in memory: changes go when Vault closes");
                }
                HostRequest::Copy(s) => {
                    let _ = krate::ui::clipboard::write_text(&s);
                }
            }
        }
        any
    }
}

const WELCOME: &str = "# Welcome\n\nThis is your new vault. Every note is a Markdown file in this folder.\n\n- Press Cmd+N for a new note, Cmd+O to find one.\n- Link notes with [[double brackets]].\n";

/// Read `key=value` from the run's arguments.
fn arg<'a>(raw: &'a str, key: &str) -> Option<&'a str> {
    raw.split('\n')
        .find_map(|a| a.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
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
            row_h: 26.0,
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
        };
        ui.app.now = krate::time::now_millis();
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
        } else if raw.split('\n').any(|a| a == "sample") {
            ui.app.open_vault(crate::sample::vault());
        }
        if let Some(note) = arg(&raw, "open") {
            if ui.app.vault.is_some() {
                ui.app.open(note, false);
                ui.mark("t_note_open_ms");
            }
        }
        if let Some(q) = arg(&raw, "search") {
            ui.app.search = crate::app::Input::with(q);
            ui.app.run_search();
            ui.app.left = LeftPane::Search;
        }
        if raw.split('\n').any(|a| a == "reading") {
            ui.app.run(Cmd::ToggleReading);
        }
        if let Some(cmd) = arg(&raw, "show") {
            match cmd {
                "switcher" => ui.app.run(Cmd::QuickSwitcher),
                "palette" => ui.app.run(Cmd::Palette),
                "outline" => ui.app.run(Cmd::ShowOutline),
                "tags" => ui.app.run(Cmd::ShowTags),
                "outgoing" => ui.app.run(Cmd::ShowOutgoing),
                _ => {}
            }
        }
        let _ = krate::ui::text_entry::start(win);
        ui.draw();
        let mut last_watch = ms();
        loop {
            // How long to sleep: not at all while there is work, until the
            // next thing that is due otherwise, forever when nothing is.
            let now = ms();
            let mut timeout: Option<u64> = None;
            let mut due = |t: u64| timeout = Some(timeout.map_or(t, |x: u64| x.min(t)));
            if ui.drag != Drag::None {
                due(16);
            }
            if ui.app.busy() {
                due(0);
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
            if ui.app.vault.as_ref().is_some_and(|v| v.fs.real()) {
                due((last_watch + disk::WATCH_EVERY_MS).saturating_sub(now));
            }
            let mut ev = events::wait(timeout.map(|t| t.min(60_000) as u32));
            let mut changed = false;
            let mut close = false;
            while let Some(e) = ev {
                match e {
                    types::Event::CloseRequested(_) => close = true,
                    types::Event::Resized(_)
                    | types::Event::RedrawRequested(_)
                    | types::Event::ThemeChanged(_) => changed = true,
                    types::Event::Key(k) => {
                        if k.pressed {
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
                                    Some(Zone::TreeRow(i)) => ui.app.context(Some(i), p.x, p.y),
                                    Some(Zone::Tree) => ui.app.context(None, p.x, p.y),
                                    _ => {}
                                }
                                changed = true;
                            }
                            (None, _) => {
                                let z = ui.zone_at(p.x, p.y);
                                if z != ui.hover {
                                    ui.hover = z;
                                    changed = true;
                                }
                            }
                            _ => {}
                        }
                    }
                    types::Event::Wheel(wh) => {
                        ui.wheel(wh.x, wh.y, wh.dy);
                        changed = true;
                    }
                    _ => {}
                }
                ev = events::poll();
            }
            if close {
                break;
            }
            ui.app.now = krate::time::now_millis();
            if ui.drag != Drag::None {
                ui.drag_step();
                changed = true;
            }
            if ui.host_requests() {
                changed = true;
            }
            // Background work, for at most 8 ms a frame.
            let start = krate::time::monotonic_nanos();
            let mut more = || krate::time::monotonic_nanos().saturating_sub(start) < 8_000_000;
            if ui.app.tick(&mut more) {
                changed = true;
            }
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
            if changed {
                ui.draw();
            }
        }
        ui.app.close();
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
