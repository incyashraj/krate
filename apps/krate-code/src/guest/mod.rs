//! The guest: one canvas, the workspace drawn on it, window events turned
//! into editor input.
//!
//! Layout comes from the canvas's real size every frame: the file tree down
//! the left, tabs across the top, the splits side by side, the bottom panel
//! when it is open, then the status line and the command line. On a narrow
//! screen the tree folds away behind a button and the gutter tightens.
//!
//! Typed characters come only from `TextInput`; key events carry Escape,
//! Enter, Backspace, Tab, the arrows and Cmd/Ctrl chords. A chord's own
//! letter, which some hosts also send as text, is dropped once, and the text
//! after Cmd/Ctrl+V is a paste (inserted as text, never run as commands).

mod disk;
mod panels;
mod run;

use crate::buffer::Pos;
use crate::palette::Action;
use crate::project;
use crate::syntax::{self, Kind};
use crate::text;
use crate::theme::{Rgba, Theme};
use crate::tools::{Severity, Tool};
use crate::vim::{self, Input, Key, Mode};
use crate::workspace::{
    Focus, HostRequest, PaletteMode, PanelTab, PromptKind, Settings, ThemeChoice, Workspace,
};
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use krate::gfx::canvas2d;
use krate::gfx::types as gfx;
use krate::layout::{Look, Rect, Said, Screen};
use krate::math::F32Ext;
use krate::ui::{events, pointer, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;
const START_W: u32 = 1280;
const START_H: u32 = 800;
const SETTINGS_KEY: &str = "settings";

// ---- what was drawn where ----------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum Hit {
    Tab(usize),
    TabClose(usize),
    TreeRow(usize),
    TreeNewFile,
    TreeNewFolder,
    TreeToggle,
    PanelTab(PanelTab),
    PanelClose,
    Problem(usize),
    PaletteRow(usize),
    PaletteBackdrop,
    PromptOk,
    PromptAlt,
    PromptCancel,
    Act(Action),
    ModeChip,
    KeysClose,
    Gutter(usize),
    OpenPalette,
    /// Inside a card: a click there is not a click on the backdrop.
    Card,
    TermTab(usize),
    TermClose(usize),
    TermArea,
    OutputArea,
    Frame(usize),
    Var(usize),
    Completion(usize),
    Breakpoint(usize),
}

#[derive(Debug, Clone, Copy)]
struct PaneGeo {
    pane: usize,
    area: Rect,
    text_x: f32,
    text_y: f32,
    first: usize,
    offset: f32,
    left: f32,
}

struct Metrics {
    code: f32,
    ui: f32,
    small: f32,
    cw: f32,
    lh: f32,
    ascent: f32,
    ui_ascent: f32,
    ui_h: f32,
}

struct Frame {
    c: u64,
    said: Said,
    hits: Vec<(Rect, Hit)>,
    panes: Vec<PaneGeo>,
    tree_area: Option<Rect>,
    panel_area: Option<Rect>,
    m: Metrics,
}

fn col(c: Rgba) -> gfx::Color {
    gfx::Color {
        r: c.r,
        g: c.g,
        b: c.b,
        a: c.a,
    }
}

fn grect(r: Rect) -> gfx::Rect {
    gfx::Rect {
        x: r.x,
        y: r.y,
        width: r.w.max(0.0),
        height: r.h.max(0.0),
    }
}

fn style(weight: u16, mono: bool, italic: bool) -> gfx::TextStyle {
    gfx::TextStyle {
        weight,
        italic,
        letter_spacing: 0.0,
        family: if mono {
            gfx::FontFamily::Mono
        } else {
            gfx::FontFamily::Sans
        },
    }
}

fn radii(r: f32) -> gfx::CornerRadii {
    gfx::CornerRadii {
        top_left: r,
        top_right: r,
        bottom_right: r,
        bottom_left: r,
    }
}

impl Frame {
    fn fill(&self, r: Rect, c: Rgba) {
        let _ = canvas2d::fill_rect(self.c, grect(r), col(c));
    }

    fn round(&self, r: Rect, rad: f32, c: Rgba) {
        let _ = canvas2d::fill_round_rect(self.c, grect(r), radii(rad), col(c));
    }

    fn outline(&self, r: Rect, rad: f32, w: f32, c: Rgba) {
        let _ = canvas2d::stroke_round_rect(self.c, grect(r), radii(rad), w, col(c));
    }

    /// Text with its top edge at `y`.
    fn text(&self, s: &str, x: f32, y: f32, size: f32, c: Rgba, weight: u16) {
        if s.is_empty() {
            return;
        }
        let asc = if (size - self.m.ui).abs() < 0.01 {
            self.m.ui_ascent
        } else {
            size * 0.93
        };
        let _ = canvas2d::draw_text_styled(
            self.c,
            s,
            gfx::Point { x, y: y + asc },
            size,
            col(c),
            style(weight, false, false),
        );
    }

    /// Text vertically centred in a row.
    fn text_mid(&self, s: &str, x: f32, row: Rect, size: f32, c: Rgba, weight: u16) {
        let h = if (size - self.m.ui).abs() < 0.01 {
            self.m.ui_h
        } else {
            size * 1.25
        };
        self.text(s, x, row.y + (row.h - h) * 0.5, size, c, weight);
    }

    fn width(&self, s: &str, size: f32, weight: u16) -> f32 {
        canvas2d::measure_text_styled(self.c, s, size, style(weight, false, false))
            .map(|m| m.width)
            .unwrap_or(s.len() as f32 * size * 0.55)
    }

    /// `s` cut to fit `max_w`, ending in "..." when it was cut.
    fn fit(&self, s: &str, size: f32, weight: u16, max_w: f32) -> String {
        if self.width(s, size, weight) <= max_w {
            return String::from(s);
        }
        let chars: Vec<char> = s.chars().collect();
        let (mut lo, mut hi) = (0usize, chars.len());
        while lo < hi {
            let mid = (lo + hi + 1) / 2;
            let mut t: String = chars.iter().take(mid).collect();
            t.push_str("...");
            if self.width(&t, size, weight) <= max_w {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        let mut t: String = chars.iter().take(lo).collect();
        t.push_str("...");
        t
    }

    fn code(&self, s: &str, x: f32, line_top: f32, c: Rgba, italic: bool) {
        if s.is_empty() {
            return;
        }
        let _ = canvas2d::draw_text_styled(
            self.c,
            s,
            gfx::Point {
                x,
                y: line_top + (self.m.lh - self.m.code * 1.2) * 0.5 + self.m.ascent,
            },
            self.m.code,
            col(c),
            style(400, true, italic),
        );
    }

    fn clip(&self, r: Rect) {
        let _ = canvas2d::set_clip(self.c, r.x, r.y, r.w.max(0.0), r.h.max(0.0));
    }

    fn unclip(&self) {
        let _ = canvas2d::clear_clip(self.c);
    }

    fn hit(&mut self, r: Rect, h: Hit) {
        self.hits.push((r, h));
    }

    fn button(&mut self, r: Rect, label: &str, h: Hit) {
        self.said.button(r, label);
        self.hit(r, h);
    }
}

fn metrics(c: u64, ws: &Workspace, look: &Look, compact: bool) -> Metrics {
    let scale = look.text_scale.clamp(0.8, 2.4);
    let code = ws.settings.font * scale * if compact { 0.96 } else { 1.0 };
    let ui = 13.0 * scale;
    let small = 11.5 * scale;
    let m = canvas2d::measure_text_styled(c, "0000000000", code, style(400, true, false)).ok();
    let cw = m
        .as_ref()
        .map(|m| m.width / 10.0)
        .unwrap_or(code * 0.6)
        .max(4.0);
    let ascent = m.as_ref().map(|m| m.ascent).unwrap_or(code * 0.95);
    let lh = (code * 1.6).round();
    let u = canvas2d::measure_text_styled(c, "Ag", ui, style(400, false, false)).ok();
    Metrics {
        code,
        ui,
        small,
        cw,
        lh,
        ascent: ascent.min(code * 1.2),
        ui_ascent: u.as_ref().map(|m| m.ascent).unwrap_or(ui * 0.93),
        ui_h: u.as_ref().map(|m| m.height).unwrap_or(ui * 1.25),
    }
}

fn theme_for(ws: &Workspace, look: &Look) -> &'static Theme {
    let dark = match ws.settings.theme {
        ThemeChoice::Auto => look.dark,
        ThemeChoice::Dark => true,
        ThemeChoice::Light => false,
    };
    Theme::for_dark(dark)
}

fn mode_color(t: &Theme, m: Mode) -> Rgba {
    match m {
        Mode::Normal => t.mode_normal,
        Mode::Insert => t.mode_insert,
        Mode::Visual | Mode::VisualLine => t.mode_visual,
        Mode::Select => t.mode_select,
        Mode::Command | Mode::Search { .. } => t.mode_command,
    }
}

// ---- drawing -------------------------------------------------------------------

struct Ui {
    win: u64,
    canvas: u64,
    ws: Workspace,
    hits: Vec<(Rect, Hit)>,
    panes: Vec<PaneGeo>,
    tree_area: Option<Rect>,
    panel_area: Option<Rect>,
    lh: f32,
    cw: f32,
    /// Phone width: the tree is a sheet over the editor, shown on demand.
    tree_sheet: bool,
    last_click: (u64, f32, f32, u8),
    title: String,
    term_geo: Option<panels::GridGeo>,
    output_geo: Option<panels::GridGeo>,
    /// The selection being made is in the Output panel (else a terminal).
    sel_in_output: bool,
    /// A drag is selecting text in a grid.
    grid_drag: bool,
    /// Where the mouse rests and since when, for hover; true once asked.
    rest: (f32, f32, u64, bool),
    /// The last time a program printed, to keep the frame quick then.
    last_output: u64,
}

impl Ui {
    fn draw(&mut self) {
        let size = canvas2d::canvas_size(self.canvas).unwrap_or(gfx::Size {
            width: START_W as f32,
            height: START_H as f32,
        });
        let (w, h) = (size.width.max(1.0), size.height.max(1.0));
        let look = Look::now();
        let t = theme_for(&self.ws, &look);
        let compact = w < 720.0;
        let m = metrics(self.canvas, &self.ws, &look, compact);
        let mut f = Frame {
            c: self.canvas,
            said: Said::new(),
            hits: Vec::new(),
            panes: Vec::new(),
            tree_area: None,
            panel_area: None,
            m,
        };
        let _ = canvas2d::clear(self.canvas, col(t.editor));

        // Clear of a phone's notch, home bar and Krate's close button.
        let screen = Screen::of(self.win);
        let safe = screen.safe_area();
        let mut area = if safe.w > 100.0
            && safe.h > 100.0
            && safe.right() <= w + 1.0
            && safe.bottom() <= h + 1.0
        {
            safe
        } else {
            Rect {
                x: 0.0,
                y: 0.0,
                w,
                h,
            }
        };

        let cmd_h = (f.m.ui * 2.0).round();
        let status_h = (f.m.ui * 2.15).round();
        let cmd = area.take_bottom(cmd_h);
        let status = area.take_bottom(status_h);

        let start = self.ws.project.is_none() && self.ws.docs.is_empty();
        if start {
            self.draw_start(&mut f, t, area);
        } else {
            let show_tree =
                self.ws.project.is_some() && self.ws.settings.tree && (!compact || self.tree_sheet);
            if show_tree && !compact {
                let tw = (w * 0.2).clamp(200.0, 300.0);
                let tree_r = area.take_left(tw);
                self.draw_tree(&mut f, t, tree_r);
            }
            if self.ws.panel_open {
                let ph = (area.h * 0.3).clamp(120.0, 280.0);
                let pr = area.take_bottom(ph);
                self.draw_panel(&mut f, t, pr);
            }
            let tabs_h = (f.m.ui * 2.6).round();
            let tabs = area.take_top(tabs_h);
            self.draw_tabs(&mut f, t, tabs, compact);
            self.draw_panes(&mut f, t, area, compact);
            if show_tree && compact {
                let top = safe.y.max(0.0);
                f.fill(
                    Rect {
                        x: 0.0,
                        y: top,
                        w,
                        h: status.y - top,
                    },
                    Rgba {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
                        a: 0.4,
                    },
                );
                let sheet = Rect {
                    x: area.x,
                    y: top,
                    w: (area.w * 0.82).min(320.0),
                    h: status.y - top,
                };
                self.draw_tree(&mut f, t, sheet);
            }
        }
        if !start {
            self.draw_popups(
                &mut f,
                t,
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w,
                    h,
                },
            );
        }
        self.draw_status(&mut f, t, status, compact);
        self.draw_cmdline(&mut f, t, cmd);

        if self.ws.show_keys {
            self.draw_keys(
                &mut f,
                t,
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w,
                    h,
                },
            );
        }
        if self.ws.palette.is_some() {
            self.draw_palette(
                &mut f,
                t,
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w,
                    h,
                },
            );
        }
        if self.ws.prompt.is_some() {
            self.draw_prompt(
                &mut f,
                t,
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w,
                    h,
                },
            );
        }
        let _ = canvas2d::present(self.canvas);
        f.said.send(self.win);
        self.hits = f.hits;
        self.panes = f.panes;
        self.tree_area = f.tree_area;
        self.panel_area = f.panel_area;
        self.lh = f.m.lh;
        self.cw = f.m.cw;
        self.update_title();
    }

    fn update_title(&mut self) {
        let mut title = String::new();
        if let Some(d) = self.ws.active_doc() {
            title.push_str(d.name());
            if d.buf.modified() {
                title.push_str(" (changed)");
            }
            title.push_str(" - ");
        }
        if let Some(p) = &self.ws.project {
            title.push_str(&p.name);
            title.push_str(" - ");
        }
        title.push_str("Krate Code");
        if title != self.title {
            let _ = window::set_title(self.win, &title);
            self.title = title;
        }
    }

    fn draw_start(&mut self, f: &mut Frame, t: &Theme, area: Rect) {
        f.fill(area, t.editor);
        let colw = area.w.min(560.0) - 48.0;
        let x = area.x + (area.w - colw) * 0.5;
        let mut y = area.y + (area.h * 0.16).max(28.0);
        let big = f.m.ui * 2.3;
        let title = Rect {
            x,
            y,
            w: colw,
            h: big * 1.3,
        };
        f.text("Krate Code", x, y, big, t.ink, 650);
        f.said.heading(title, "Krate Code");
        y += big * 1.45;
        let lines = [
            "A keyboard-first Rust editor. Normal, insert and visual modes",
            "work as in Neovim; clicking into the text and typing works too.",
        ];
        for l in lines {
            f.text(l, x, y, f.m.ui, t.muted, 400);
            f.said.text(
                Rect {
                    x,
                    y,
                    w: colw,
                    h: f.m.ui_h,
                },
                l,
            );
            y += f.m.ui_h * 1.25;
        }
        y += f.m.ui * 1.6;
        let bh = (f.m.ui * 3.0).round();
        let items: [(&str, &str, Action); 3] = [
            ("Open folder", "Ctrl+O", Action::OpenFolder),
            (
                "New Rust project",
                "Cargo.toml and src/main.rs",
                Action::NewProject,
            ),
            (
                "Take the tour",
                "a sample crate, kept in memory",
                Action::Tour,
            ),
        ];
        for (i, (label, hint, act)) in items.iter().enumerate() {
            let r = Rect {
                x,
                y,
                w: colw,
                h: bh,
            };
            let primary = i == 0;
            f.round(r, 8.0, if primary { t.accent } else { t.raised });
            if !primary {
                f.outline(r, 8.0, 1.0, t.line);
            }
            let ink = if primary { t.on_accent } else { t.ink };
            f.text_mid(label, x + 16.0, r, f.m.ui, ink, 600);
            let hw = f.width(hint, f.m.small, 400);
            if hw + f.width(label, f.m.ui, 600) + 48.0 < colw {
                f.text_mid(
                    hint,
                    r.right() - 16.0 - hw,
                    r,
                    f.m.small,
                    if primary {
                        t.on_accent.with_alpha(0.8)
                    } else {
                        t.muted
                    },
                    400,
                );
            }
            f.button(r, label, Hit::Act(*act));
            y += bh + 10.0;
        }
        y += f.m.ui * 1.2;
        let tips = [
            ("i", "type"),
            ("Esc", "stop typing; commands again"),
            (":w", "save"),
            ("Ctrl+Shift+P", "every action, searchable"),
        ];
        for (k, what) in tips {
            if y + f.m.ui_h > area.bottom() - 8.0 {
                break;
            }
            let kw = f.width(k, f.m.small, 600) + 14.0;
            let kr = Rect {
                x,
                y,
                w: kw,
                h: f.m.ui_h + 6.0,
            };
            f.round(kr, 5.0, t.chrome);
            f.outline(kr, 5.0, 1.0, t.line);
            f.text_mid(k, x + 7.0, kr, f.m.small, t.ink, 600);
            f.text_mid(what, x + kw + 10.0, kr, f.m.small, t.muted, 400);
            let mut line = String::from(k);
            line.push_str(": ");
            line.push_str(what);
            f.said.text(
                Rect {
                    x,
                    y,
                    w: colw,
                    h: kr.h,
                },
                &line,
            );
            y += kr.h + 8.0;
        }
    }

    fn draw_tree(&mut self, f: &mut Frame, t: &Theme, r: Rect) {
        f.fill(r, t.chrome);
        f.fill(
            Rect {
                x: r.right() - 1.0,
                y: r.y,
                w: 1.0,
                h: r.h,
            },
            t.line,
        );
        f.tree_area = Some(r);
        let focused = self.ws.focus == Focus::Tree;
        let Some(p) = self.ws.project.as_ref() else {
            return;
        };
        let head_h = (f.m.ui * 2.6).round();
        let head = Rect {
            x: r.x,
            y: r.y,
            w: r.w - 1.0,
            h: head_h,
        };
        let real = p.fs.is_real();
        let name = if real {
            String::from(&p.name)
        } else {
            String::from("Tour, in memory")
        };
        let bsz = (head_h - 12.0).max(22.0);
        let folder_b = Rect {
            x: head.right() - 8.0 - bsz,
            y: head.y + 6.0,
            w: bsz,
            h: bsz,
        };
        let file_b = Rect {
            x: folder_b.x - 4.0 - bsz,
            y: folder_b.y,
            w: bsz,
            h: bsz,
        };
        let shown = f.fit(&name, f.m.ui, 650, file_b.x - r.x - 20.0);
        f.text_mid(&shown, r.x + 14.0, head, f.m.ui, t.ink, 650);
        f.said.heading(
            Rect {
                x: r.x,
                y: head.y,
                w: file_b.x - r.x,
                h: head.h,
            },
            &name,
        );
        for (b, label, hit) in [
            (file_b, "New file", Hit::TreeNewFile),
            (folder_b, "New folder", Hit::TreeNewFolder),
        ] {
            f.round(b, 5.0, t.raised);
            let (cx, cy) = (b.x + b.w * 0.5, b.y + b.h * 0.5);
            if hit == Hit::TreeNewFile {
                // A page with a folded corner.
                f.outline(
                    Rect {
                        x: cx - 4.5,
                        y: cy - 6.0,
                        w: 9.0,
                        h: 12.0,
                    },
                    1.5,
                    1.3,
                    t.muted,
                );
                f.fill(
                    Rect {
                        x: cx - 2.0,
                        y: cy - 0.6,
                        w: 4.0,
                        h: 1.3,
                    },
                    t.muted,
                );
                f.fill(
                    Rect {
                        x: cx - 0.65,
                        y: cy - 2.0,
                        w: 1.3,
                        h: 4.0,
                    },
                    t.muted,
                );
            } else {
                // A folder: its tab, then its body.
                f.round(
                    Rect {
                        x: cx - 6.5,
                        y: cy - 5.0,
                        w: 5.5,
                        h: 3.0,
                    },
                    1.0,
                    t.muted,
                );
                f.outline(
                    Rect {
                        x: cx - 6.5,
                        y: cy - 3.0,
                        w: 13.0,
                        h: 9.0,
                    },
                    1.5,
                    1.3,
                    t.muted,
                );
            }
            f.button(b, label, hit);
        }
        let mut list = Rect {
            x: r.x,
            y: head.bottom(),
            w: r.w - 1.0,
            h: r.h - head_h,
        };
        if self.tree_sheet {
            let close = list.take_bottom((f.m.ui * 2.8).round());
            f.round(close.pad(6.0), 6.0, t.raised);
            f.text_mid(
                "Close the file list",
                close.x + 16.0,
                close,
                f.m.ui,
                t.ink,
                500,
            );
            f.button(close, "Close the file list", Hit::TreeToggle);
        }
        let rh = (f.m.ui * 1.95).round();
        let rows = &p.tree.rows;
        let visible = (list.h / rh).floor().max(1.0) as usize;
        let mut first = p.tree.scroll.max(0.0) as usize;
        if p.tree.selected < first {
            first = p.tree.selected;
        } else if p.tree.selected >= first + visible {
            first = p.tree.selected + 1 - visible;
        }
        first = first.min(rows.len().saturating_sub(visible));
        let active_path = self.ws.active_doc().map(|d| d.path.clone());
        f.clip(list);
        for (k, row) in rows.iter().enumerate().skip(first).take(visible + 1) {
            let y = list.y + (k - first) as f32 * rh;
            let rr = Rect {
                x: list.x,
                y,
                w: list.w,
                h: rh,
            };
            let is_sel = k == p.tree.selected;
            let open = active_path.as_deref() == Some(row.path.as_str());
            if is_sel && focused {
                f.round(rr.inset(1.0, 6.0, 1.0, 6.0), 5.0, t.selection);
            } else if open {
                f.round(rr.inset(1.0, 6.0, 1.0, 6.0), 5.0, t.raised);
            }
            let x = list.x + 12.0 + row.depth as f32 * 14.0;
            if row.is_dir {
                // A small drawn chevron: two strokes, open or closed.
                let (cx, cy) = (x + 4.0, rr.y + rh * 0.5);
                let pts: [(f32, f32, f32, f32); 2] = if row.expanded {
                    [
                        (cx - 3.5, cy - 1.5, cx, cy + 2.0),
                        (cx, cy + 2.0, cx + 3.5, cy - 1.5),
                    ]
                } else {
                    [
                        (cx - 1.5, cy - 3.5, cx + 2.0, cy),
                        (cx + 2.0, cy, cx - 1.5, cy + 3.5),
                    ]
                };
                for (x1, y1, x2, y2) in pts {
                    // A line as a run of small squares along it.
                    let steps = 6;
                    for k in 0..=steps {
                        let tt = k as f32 / steps as f32;
                        let (px, py) = (x1 + (x2 - x1) * tt, y1 + (y2 - y1) * tt);
                        f.fill(
                            Rect {
                                x: px - 0.7,
                                y: py - 0.7,
                                w: 1.4,
                                h: 1.4,
                            },
                            t.muted,
                        );
                    }
                }
            } else {
                let dot = match row.name.rsplit('.').next() {
                    Some("rs") => t.keyword,
                    Some("toml") => t.ty,
                    Some("md") => t.string,
                    _ => t.faint,
                };
                f.round(
                    Rect {
                        x: x + 1.0,
                        y: rr.y + rh * 0.5 - 3.0,
                        w: 6.0,
                        h: 6.0,
                    },
                    3.0,
                    dot,
                );
            }
            let heavy = row.is_dir && project::is_heavy_dir(&row.name);
            let ink = if heavy {
                t.faint
            } else if open {
                t.ink
            } else {
                t.muted.mix(t.ink, 0.45)
            };
            let shown = f.fit(
                &row.name,
                f.m.ui,
                if open { 600 } else { 400 },
                rr.right() - x - 22.0,
            );
            f.text_mid(
                &shown,
                x + 14.0,
                rr,
                f.m.ui,
                ink,
                if open { 600 } else { 400 },
            );
            f.said.button(rr, &row.name);
            f.hit(rr, Hit::TreeRow(k));
        }
        f.unclip();
        if let Some(err) = &p.tree.error {
            let er = Rect {
                x: list.x + 12.0,
                y: list.bottom() - f.m.ui_h * 2.0,
                w: list.w - 24.0,
                h: f.m.ui_h,
            };
            f.text(err, er.x, er.y, f.m.small, t.danger, 400);
        }
    }

    fn draw_tabs(&mut self, f: &mut Frame, t: &Theme, r: Rect, compact: bool) {
        f.fill(r, t.chrome);
        f.fill(
            Rect {
                x: r.x,
                y: r.bottom() - 1.0,
                w: r.w,
                h: 1.0,
            },
            t.line,
        );
        let mut x = r.x;
        if compact && self.ws.project.is_some() {
            let b = Rect {
                x: r.x + 4.0,
                y: r.y + 4.0,
                w: (f.m.ui * 4.4).max(44.0),
                h: r.h - 8.0,
            };
            f.round(b, 6.0, t.raised);
            f.text_mid("Files", b.x + 10.0, b, f.m.ui, t.ink, 600);
            f.button(b, "Files", Hit::TreeToggle);
            x = b.right() + 4.0;
        }
        let active = self.ws.active_doc_index();
        let pal = Rect {
            x: r.right() - (f.m.ui * 2.6),
            y: r.y + 5.0,
            w: f.m.ui * 2.2,
            h: r.h - 10.0,
        };
        let avail_right = pal.x - 4.0;
        f.clip(Rect {
            x,
            y: r.y,
            w: (avail_right - x).max(0.0),
            h: r.h,
        });
        for (i, d) in self.ws.docs.iter().enumerate() {
            let name = d.name();
            let tw = f.width(name, f.m.ui, 500) + 46.0;
            if x + tw > avail_right && i != active.unwrap_or(usize::MAX) && x > r.x + 4.0 {
                break;
            }
            let tr = Rect {
                x,
                y: r.y,
                w: tw.min(avail_right - x),
                h: r.h - 1.0,
            };
            let on = Some(i) == active;
            if on {
                f.fill(tr, t.editor);
                f.fill(
                    Rect {
                        x: tr.x,
                        y: tr.y,
                        w: tr.w,
                        h: 2.0,
                    },
                    t.accent,
                );
            }
            f.text_mid(
                name,
                x + 14.0,
                tr,
                f.m.ui,
                if on { t.ink } else { t.muted },
                if on { 600 } else { 400 },
            );
            let close = Rect {
                x: tr.right() - 26.0,
                y: tr.y + (tr.h - 18.0) * 0.5,
                w: 18.0,
                h: 18.0,
            };
            if d.buf.modified() {
                f.round(
                    Rect {
                        x: close.x + 5.0,
                        y: close.y + 5.0,
                        w: 8.0,
                        h: 8.0,
                    },
                    4.0,
                    t.warn,
                );
            } else {
                f.text_mid("x", close.x + 5.0, close, f.m.small, t.faint, 500);
            }
            let mut label = String::from(name);
            if d.buf.modified() {
                label.push_str(", changed");
            }
            f.said.button(
                Rect {
                    x: tr.x,
                    y: tr.y,
                    w: tr.w - 28.0,
                    h: tr.h,
                },
                &label,
            );
            f.hit(
                Rect {
                    x: tr.x,
                    y: tr.y,
                    w: tr.w - 28.0,
                    h: tr.h,
                },
                Hit::Tab(i),
            );
            let mut cl = String::from("Close ");
            cl.push_str(name);
            f.said.button(close, &cl);
            f.hit(close, Hit::TabClose(i));
            f.fill(
                Rect {
                    x: tr.right(),
                    y: tr.y + 8.0,
                    w: 1.0,
                    h: tr.h - 16.0,
                },
                t.line,
            );
            x += tw + 1.0;
            if x > avail_right {
                break;
            }
        }
        f.unclip();
        // The palette, one click away for a mouse user.
        f.round(pal, 6.0, t.raised);
        let g = "...";
        let gw = f.width(g, f.m.ui, 700);
        f.text_mid(g, pal.x + (pal.w - gw) * 0.5, pal, f.m.ui, t.muted, 700);
        f.button(pal, "All actions", Hit::OpenPalette);
    }

    fn draw_panes(&mut self, f: &mut Frame, t: &Theme, area: Rect, compact: bool) {
        let n = self.ws.panes.len().max(1);
        let cols = area.columns(n, 0.0);
        for (i, r) in cols.iter().enumerate() {
            self.draw_pane(f, t, i, *r, compact, n > 1);
            if i + 1 < n {
                f.fill(
                    Rect {
                        x: r.right() - 1.0,
                        y: r.y,
                        w: 1.0,
                        h: r.h,
                    },
                    t.line,
                );
            }
        }
    }

    fn draw_pane(
        &mut self,
        f: &mut Frame,
        t: &Theme,
        i: usize,
        r: Rect,
        compact: bool,
        split: bool,
    ) {
        let mut r = r;
        let active = i == self.ws.active && self.ws.focus == Focus::Editor;
        if split {
            let head = r.take_top((f.m.ui * 1.9).round());
            f.fill(head, if active { t.editor } else { t.chrome });
            f.fill(
                Rect {
                    x: head.x,
                    y: head.bottom() - 1.0,
                    w: head.w,
                    h: 1.0,
                },
                t.line,
            );
            if i == self.ws.active {
                f.fill(
                    Rect {
                        x: head.x,
                        y: head.bottom() - 2.0,
                        w: head.w,
                        h: 2.0,
                    },
                    t.accent.with_alpha(0.7),
                );
            }
            let name = self
                .ws
                .pane_doc(i)
                .map(|d| d.path.as_str())
                .unwrap_or("empty");
            let shown = f.fit(name, f.m.small, 500, head.w - 20.0);
            f.text_mid(
                &shown,
                head.x + 12.0,
                head,
                f.m.small,
                if active { t.ink } else { t.muted },
                500,
            );
            f.said.text(head, name);
        }
        let Some(di) = self.ws.panes.get(i).and_then(|p| p.doc) else {
            f.fill(r, t.editor);
            let msg = if self.ws.project.is_some() {
                "Open a file from the tree, or press Ctrl+P to find one"
            } else {
                "No file open"
            };
            let mw = f.width(msg, f.m.ui, 400);
            f.text(
                msg,
                r.x + ((r.w - mw) * 0.5).max(12.0),
                r.y + r.h * 0.4,
                f.m.ui,
                t.muted,
                400,
            );
            f.said.text(r, msg);
            f.panes.push(PaneGeo {
                pane: i,
                area: r,
                text_x: r.x,
                text_y: r.y,
                first: 0,
                offset: 0.0,
                left: 0.0,
            });
            return;
        };
        let lh = f.m.lh;
        let cw = f.m.cw;
        let tab = self.ws.vim.tab;
        let lines = self.ws.docs[di].buf.line_count();
        let digits = text::num(lines).len().max(3);
        let gutter_w = if compact {
            digits as f32 * cw + 18.0
        } else {
            digits as f32 * cw + 30.0
        };
        let text_x = r.x + gutter_w + 8.0;
        let text_w = (r.right() - text_x - 6.0).max(cw * 4.0);
        let rows = ((r.h - 4.0) / lh).floor().max(1.0) as usize;
        let cols = (text_w / cw).floor().max(4.0) as usize;
        if let Some(p) = self.ws.panes.get_mut(i) {
            p.rows = rows;
            p.cols = cols;
        }
        let pane = self.ws.panes[i];
        let first = pane.top.floor().max(0.0) as usize;
        let offset = (pane.top - first as f32) * lh;
        let text_y = r.y + 2.0;
        f.panes.push(PaneGeo {
            pane: i,
            area: r,
            text_x,
            text_y,
            first,
            offset,
            left: pane.left,
        });
        let left = pane.left.floor().max(0.0) as usize;
        let left_frac = (pane.left - left as f32) * cw;

        f.fill(r, t.editor);
        let gut = Rect {
            x: r.x,
            y: r.y,
            w: gutter_w,
            h: r.h,
        };
        f.fill(gut, t.editor);

        let vim = &self.ws.vim;
        let is_active_pane = i == self.ws.active;
        let sel = if is_active_pane {
            vim.selection(&self.ws.docs[di].buf, &pane.cur)
        } else {
            None
        };
        let search = if vim.highlight {
            vim.last_search.as_ref().map(|(p, _)| p.clone())
        } else {
            None
        };
        let cursor = pane.cur.pos;
        let rust = self.ws.docs[di].is_rust();
        let relative = vim.relative_numbers;
        let mode = vim.mode;

        // Highlight states down to the first visible line.
        let start_state = if rust {
            let doc = &mut self.ws.docs[di];
            let buf = &doc.buf;
            doc.hl.state_at(first, |k| String::from(buf.line(k)))
        } else {
            syntax::State::Code
        };
        let doc = &self.ws.docs[di];
        let buf = &doc.buf;
        let debug_line = self.ws.debug_line(&doc.path);
        let problems: Vec<(usize, usize, usize, usize, crate::theme::Rgba)> = self
            .ws
            .problems
            .iter()
            .filter(|d| d.file == doc.path)
            .map(|d| {
                let c = match d.severity {
                    Severity::Error => t.danger,
                    Severity::Warning => t.warn,
                    Severity::Note => t.muted,
                };
                (d.line, d.col, d.end_line.max(d.line), d.end_col, c)
            })
            .collect();
        let mut state = start_state;
        f.clip(r);
        let mut said_lines = 0;
        for k in 0..=rows + 1 {
            let ln = first + k;
            if ln >= lines {
                break;
            }
            let y = text_y + k as f32 * lh - offset;
            if y > r.bottom() {
                break;
            }
            let line = buf.line(ln);
            let line_r = Rect {
                x: r.x,
                y,
                w: r.w,
                h: lh,
            };
            if ln == cursor.line && is_active_pane && sel.is_none() {
                f.fill(line_r, t.current_line);
            }
            if debug_line == Some(ln) {
                f.fill(line_r, t.warn.with_alpha(0.16));
                f.fill(
                    Rect {
                        x: r.x + 2.0,
                        y: y + 3.0,
                        w: 3.0,
                        h: lh - 6.0,
                    },
                    t.warn,
                );
            }
            // Gutter: breakpoint and number.
            if doc.breakpoints.contains(&ln) {
                f.round(
                    Rect {
                        x: r.x + 7.0,
                        y: y + lh * 0.5 - 4.5,
                        w: 9.0,
                        h: 9.0,
                    },
                    4.5,
                    t.breakpoint,
                );
            }
            let number = if relative && ln != cursor.line {
                text::num(ln.abs_diff(cursor.line))
            } else {
                text::num(ln + 1)
            };
            let nx = r.x + gutter_w - 8.0 - number.len() as f32 * cw;
            f.code(
                &number,
                nx,
                y,
                if ln == cursor.line {
                    t.gutter_current
                } else {
                    t.gutter
                },
                false,
            );
            f.hit(
                Rect {
                    x: r.x,
                    y,
                    w: gutter_w,
                    h: lh,
                },
                Hit::Gutter(ln),
            );
            // The left edge of the gutter sets and clears breakpoints.
            f.hit(
                Rect {
                    x: r.x,
                    y,
                    w: 20.0,
                    h: lh,
                },
                Hit::Breakpoint(ln),
            );

            f.clip(Rect {
                x: text_x - 2.0,
                y: r.y,
                w: text_w + 8.0,
                h: r.h,
            });
            let x0 = text_x - left_frac;
            let dcol = |c: usize| text::display_col(line, c, tab) as f32;
            // Selection.
            if let Some((a, b, linewise)) = sel {
                if ln >= a.line && ln <= b.line {
                    let sa = if ln == a.line && !linewise {
                        dcol(a.col)
                    } else {
                        0.0
                    };
                    let sb = if ln == b.line && !linewise {
                        dcol(b.col)
                    } else {
                        text::display_width(line, tab) as f32 + 1.0
                    };
                    let sb = if ln < b.line || linewise {
                        sb.max(sa + 0.5)
                    } else {
                        sb
                    };
                    let xa = x0 + (sa - left as f32) * cw;
                    let xb = x0 + (sb - left as f32) * cw;
                    if xb > xa {
                        f.fill(
                            Rect {
                                x: xa,
                                y,
                                w: xb - xa,
                                h: lh,
                            },
                            t.selection,
                        );
                    }
                }
            }
            // Search hits.
            if let Some(pat) = &search {
                for (a, b) in vim::matches_on_line(line, pat) {
                    let xa = x0 + (dcol(a) - left as f32) * cw;
                    let xb = x0 + (dcol(b) - left as f32) * cw;
                    f.round(
                        Rect {
                            x: xa,
                            y: y + 2.0,
                            w: xb - xa,
                            h: lh - 4.0,
                        },
                        3.0,
                        t.search_hit,
                    );
                }
            }
            // Code.
            let has_tabs = line.contains('\t');
            let expanded = if has_tabs {
                text::expand_tabs(line, tab)
            } else {
                String::new()
            };
            let spans = if rust {
                let (spans, next) = syntax::highlight_line(line, state);
                state = next;
                spans
            } else {
                alloc::vec![syntax::Span {
                    start: 0,
                    end: line.len(),
                    kind: Kind::Text
                }]
            };
            let max_col = left + cols;
            let mut run_start: Option<(usize, usize, Kind)> = None;
            let flush = |f: &Frame, s: usize, e: usize, kind: Kind| {
                let ds = text::display_col(line, s, tab);
                let de = text::display_col(line, e, tab);
                // Only the columns that fit in the pane are drawn at all.
                let (from, to) = (ds.max(left), de.min(max_col));
                if from >= to {
                    return;
                }
                let piece: String = if has_tabs {
                    expanded.chars().skip(from).take(to - from).collect()
                } else if from == ds && to == de {
                    String::from(line.get(s..e).unwrap_or(""))
                } else {
                    line.get(s..e)
                        .unwrap_or("")
                        .chars()
                        .skip(from - ds)
                        .take(to - from)
                        .collect()
                };
                let x = x0 + (from as f32 - left as f32) * cw;
                f.code(&piece, x, y, t.syntax(kind), Theme::italic(kind));
            };
            for sp in &spans {
                match run_start {
                    Some((s, _e, kind))
                        if t.syntax(kind) == t.syntax(sp.kind)
                            && Theme::italic(kind) == Theme::italic(sp.kind) =>
                    {
                        run_start = Some((s, sp.end, kind));
                    }
                    Some((s, e, kind)) => {
                        flush(f, s, e, kind);
                        run_start = Some((sp.start, sp.end, sp.kind));
                    }
                    None => run_start = Some((sp.start, sp.end, sp.kind)),
                }
            }
            if let Some((s, e, kind)) = run_start {
                flush(f, s, e, kind);
            }
            // Problems: a squiggle under the place, and a mark in the gutter.
            let mut worst: Option<crate::theme::Rgba> = None;
            for &(l0, c0, l1, c1, c) in &problems {
                if ln < l0 || ln > l1 {
                    continue;
                }
                worst.get_or_insert(c);
                let a = if ln == l0 {
                    text::byte_of_char(line, c0)
                } else {
                    0
                };
                let b = if ln == l1 {
                    text::byte_of_char(line, c1)
                } else {
                    line.len()
                };
                let (da, db) = (dcol(a), dcol(b.max(a)));
                let db = if db <= da { da + 1.0 } else { db };
                let xa = x0 + (da - left as f32) * cw;
                let xb = x0 + (db - left as f32) * cw;
                Self::squiggle(f, xa, xb, y + lh - 4.0, c);
            }
            if let Some(c) = worst {
                if !doc.breakpoints.contains(&ln) {
                    f.fill(
                        Rect {
                            x: r.x + 9.0,
                            y: y + lh * 0.5 - 2.5,
                            w: 5.0,
                            h: 5.0,
                        },
                        c,
                    );
                }
            }
            // Cursor.
            if ln == cursor.line && is_active_pane {
                let cx = x0 + (dcol(cursor.col) - left as f32) * cw;
                let focused = self.ws.focus == Focus::Editor
                    && self.ws.palette.is_none()
                    && self.ws.prompt.is_none();
                match mode {
                    Mode::Insert | Mode::Select => {
                        f.fill(
                            Rect {
                                x: cx - 1.0,
                                y: y + 2.0,
                                w: 2.0,
                                h: lh - 4.0,
                            },
                            t.cursor_insert,
                        );
                    }
                    _ if focused => {
                        let chr = line
                            .get(cursor.col..)
                            .and_then(|r| r.chars().next())
                            .filter(|c| *c != '\t');
                        f.round(
                            Rect {
                                x: cx,
                                y: y + 1.0,
                                w: cw,
                                h: lh - 2.0,
                            },
                            2.0,
                            t.cursor_normal,
                        );
                        if let Some(c) = chr {
                            let mut s = String::new();
                            s.push(c);
                            f.code(&s, cx, y, t.editor, false);
                        }
                    }
                    _ => f.outline(
                        Rect {
                            x: cx,
                            y: y + 1.0,
                            w: cw,
                            h: lh - 2.0,
                        },
                        2.0,
                        1.0,
                        t.cursor_normal,
                    ),
                }
            } else if ln == pane.cur.pos.line && !is_active_pane {
                let cx = x0 + (dcol(pane.cur.pos.col.min(line.len())) - left as f32) * cw;
                f.outline(
                    Rect {
                        x: cx,
                        y: y + 1.0,
                        w: cw,
                        h: lh - 2.0,
                    },
                    2.0,
                    1.0,
                    t.faint,
                );
            }
            f.clip(r);
            if said_lines < 80 && !line.trim().is_empty() {
                f.said.text(
                    Rect {
                        x: text_x,
                        y,
                        w: text_w,
                        h: lh,
                    },
                    line,
                );
                said_lines += 1;
            }
        }
        f.unclip();
        // Scroll position, quietly, at the right edge.
        if lines > rows {
            let track = Rect {
                x: r.right() - 5.0,
                y: r.y + 2.0,
                w: 3.0,
                h: r.h - 4.0,
            };
            let frac = (rows as f32 / lines as f32).clamp(0.05, 1.0);
            let pos = (pane.top / lines as f32).clamp(0.0, 1.0 - frac);
            f.round(
                Rect {
                    x: track.x,
                    y: track.y + pos * track.h,
                    w: 3.0,
                    h: frac * track.h,
                },
                1.5,
                t.line,
            );
        }
    }

    fn draw_panel(&mut self, f: &mut Frame, t: &Theme, r: Rect) {
        f.fill(r, t.chrome);
        f.fill(
            Rect {
                x: r.x,
                y: r.y,
                w: r.w,
                h: 1.0,
            },
            t.line,
        );
        f.panel_area = Some(r);
        let mut r = r;
        let head = r.take_top((f.m.ui * 2.4).round());
        let mut x = head.x + 10.0;
        let tabs = self.panel_tabs();
        for (tab, name) in tabs {
            let mut label = String::from(name);
            if tab == PanelTab::Problems && !self.ws.problems.is_empty() {
                label.push(' ');
                label.push_str(&text::num(self.ws.problems.len()));
            }
            let w = f.width(&label, f.m.ui, 600) + 20.0;
            let tr = Rect {
                x,
                y: head.y + 5.0,
                w,
                h: head.h - 10.0,
            };
            let on = self.ws.panel_tab == tab;
            if on {
                f.round(tr, 6.0, t.raised);
            }
            f.text_mid(
                &label,
                x + 10.0,
                tr,
                f.m.ui,
                if on { t.ink } else { t.muted },
                if on { 600 } else { 500 },
            );
            f.button(tr, &label, Hit::PanelTab(tab));
            x += w + 4.0;
        }
        let close = Rect {
            x: head.right() - 34.0,
            y: head.y + 5.0,
            w: 26.0,
            h: head.h - 10.0,
        };
        f.text_mid("x", close.x + 9.0, close, f.m.ui, t.muted, 500);
        f.button(close, "Close the panel", Hit::PanelClose);
        // A build or run going: what it is, and a Stop.
        if let Some(label) = self.ws.running_label() {
            let stop = Rect {
                x: close.x - 70.0,
                y: head.y + 5.0,
                w: 62.0,
                h: head.h - 10.0,
            };
            f.round(stop, 6.0, t.danger.with_alpha(0.18));
            f.text_mid("Stop", stop.x + 16.0, stop, f.m.small, t.danger, 700);
            f.button(stop, "Stop the build", Hit::Act(Action::Tool(Tool::Stop)));
            let lw = f.width(&label, f.m.small, 500);
            f.text_mid(&label, stop.x - lw - 12.0, head, f.m.small, t.muted, 500);
            f.said.text(head, &label);
        }
        let body = r.pad(12.0);
        f.clip(r);
        match self.ws.panel_tab {
            PanelTab::Problems => {
                if self.ws.problems.is_empty() {
                    let a = "No problems listed.";
                    let b = "Build (F7) and Check fill this list with cargo's errors and warnings, and rust-analyzer adds what it finds as you type. Enter or a click on one jumps to it.";
                    f.text(a, body.x, body.y, f.m.ui, t.ink, 500);
                    f.said.text(
                        Rect {
                            x: body.x,
                            y: body.y,
                            w: body.w,
                            h: f.m.ui_h,
                        },
                        a,
                    );
                    self.wrap(
                        f,
                        b,
                        body.x,
                        body.y + f.m.ui_h * 1.5,
                        body.w,
                        f.m.small,
                        t.muted,
                    );
                } else {
                    let rh = f.m.ui_h * 1.6;
                    for (k, d) in self.ws.problems.iter().enumerate() {
                        let y = body.y + k as f32 * rh;
                        if y > body.bottom() {
                            break;
                        }
                        let row = Rect {
                            x: r.x + 4.0,
                            y,
                            w: r.w - 8.0,
                            h: rh,
                        };
                        if k == self.ws.problem_selected {
                            f.round(row, 5.0, t.raised);
                        }
                        let (sev, c) = match d.severity {
                            Severity::Error => ("error", t.danger),
                            Severity::Warning => ("warning", t.warn),
                            Severity::Note => ("note", t.muted),
                        };
                        f.text_mid(sev, body.x, row, f.m.small, c, 600);
                        let mut place = d.file.clone();
                        place.push(':');
                        place.push_str(&text::num(d.line + 1));
                        let px = body.x + 70.0;
                        f.text_mid(&place, px, row, f.m.small, t.muted, 500);
                        let mx = px + f.width(&place, f.m.small, 500) + 14.0;
                        let shown = f.fit(&d.message, f.m.ui, 400, row.right() - mx - 8.0);
                        f.text_mid(&shown, mx, row, f.m.ui, t.ink, 400);
                        let mut said = String::from(sev);
                        said.push_str(" in ");
                        said.push_str(&place);
                        said.push_str(": ");
                        said.push_str(&d.message);
                        f.button(row, &said, Hit::Problem(k));
                    }
                }
            }
            PanelTab::Output => {
                f.unclip();
                self.draw_output(f, t, body);
            }
            PanelTab::Terminal => {
                f.unclip();
                self.draw_terminal_panel(f, t, r.pad(8.0));
            }
            PanelTab::Debug => {
                f.unclip();
                self.draw_debug_panel(f, t, body);
            }
        }
        f.unclip();
    }

    /// Word-wrapped text; returns the height used.
    fn wrap(&self, f: &mut Frame, s: &str, x: f32, y: f32, w: f32, size: f32, c: Rgba) -> f32 {
        let mut line = String::new();
        let mut yy = y;
        let lh = size * 1.45;
        for word in s.split(' ') {
            let mut probe = line.clone();
            if !probe.is_empty() {
                probe.push(' ');
            }
            probe.push_str(word);
            if f.width(&probe, size, 400) > w && !line.is_empty() {
                f.text(&line, x, yy, size, c, 400);
                f.said.text(Rect { x, y: yy, w, h: lh }, &line);
                yy += lh;
                line = String::from(word);
            } else {
                line = probe;
            }
        }
        if !line.is_empty() {
            f.text(&line, x, yy, size, c, 400);
            f.said.text(Rect { x, y: yy, w, h: lh }, &line);
            yy += lh;
        }
        yy - y
    }

    fn draw_status(&mut self, f: &mut Frame, t: &Theme, r: Rect, compact: bool) {
        f.fill(r, t.chrome);
        f.fill(
            Rect {
                x: r.x,
                y: r.y,
                w: r.w,
                h: 1.0,
            },
            t.line,
        );
        let mode = self.ws.vim.mode;
        let label = mode.label();
        let mc = mode_color(t, mode);
        let lw = f.width(label, f.m.small, 700) + 22.0;
        let chip = Rect {
            x: r.x + 6.0,
            y: r.y + 4.0,
            w: lw,
            h: r.h - 8.0,
        };
        f.round(chip, 5.0, mc);
        f.text_mid(label, chip.x + 11.0, chip, f.m.small, t.on_mode, 700);
        // Said with the mode's own name, then what pressing it does.
        let mut said = String::from(label);
        said.push_str(match mode {
            Mode::Normal => " mode. Press to start typing",
            Mode::Insert => " mode. Press for commands",
            _ => " mode",
        });
        f.button(chip, &said, Hit::ModeChip);
        let mut x = chip.right() + 12.0;
        if let Some(d) = self.ws.active_doc() {
            let mut name = String::from(&d.path);
            if d.buf.modified() {
                name.push_str(" [+]");
            }
            let room = if compact { r.w * 0.4 } else { r.w * 0.45 };
            let shown = f.fit(&name, f.m.small, 500, room);
            f.text_mid(&shown, x, r, f.m.small, t.ink, 500);
            f.said.text(
                Rect {
                    x,
                    y: r.y,
                    w: room,
                    h: r.h,
                },
                &name,
            );
            x += f.width(&shown, f.m.small, 500) + 14.0;
        }
        let busy = self.ws.running_label().or_else(|| {
            self.ws.lsp_busy.as_ref().map(|b| {
                let mut s = String::from("rust-analyzer: ");
                s.push_str(b);
                s
            })
        });
        if let Some(b) = busy {
            if !compact {
                let shown = f.fit(&b, f.m.small, 500, r.w * 0.22);
                f.round(
                    Rect {
                        x,
                        y: r.y + 8.0,
                        w: 7.0,
                        h: 7.0,
                    },
                    3.5,
                    t.accent,
                );
                f.text_mid(&shown, x + 13.0, r, f.m.small, t.muted, 500);
                f.said.text(
                    Rect {
                        x,
                        y: r.y,
                        w: r.w * 0.22,
                        h: r.h,
                    },
                    &b,
                );
            }
        }
        let _ = x;
        // Right side: keys being typed, position, language.
        let mut right = r.right() - 12.0;
        let mut parts: Vec<String> = Vec::new();
        if let (Some(d), Some(p)) = (self.ws.active_doc(), self.ws.panes.get(self.ws.active)) {
            let mut pos = String::from("Ln ");
            pos.push_str(&text::num(p.cur.pos.line + 1));
            pos.push_str(", Col ");
            pos.push_str(&text::num(
                text::display_col(d.buf.line(p.cur.pos.line), p.cur.pos.col, self.ws.vim.tab) + 1,
            ));
            parts.push(pos);
            if !compact {
                parts.push(String::from(if d.is_rust() { "Rust" } else { "Text" }));
                let mut sp = String::from("Spaces: ");
                sp.push_str(&text::num(self.ws.vim.shiftwidth));
                parts.push(sp);
            }
        }
        if let Some(p) = &self.ws.project {
            if !compact {
                parts.push(String::from(if p.fs.is_real() {
                    p.name.as_str()
                } else {
                    "tour, in memory"
                }));
            }
        }
        let pending = self.ws.vim.pending_keys();
        if !pending.is_empty() {
            parts.insert(0, String::from(pending));
        }
        for p in parts.iter().rev() {
            let w = f.width(p, f.m.small, 500);
            right -= w;
            f.text_mid(p, right, r, f.m.small, t.muted, 500);
            f.said.text(
                Rect {
                    x: right,
                    y: r.y,
                    w,
                    h: r.h,
                },
                p,
            );
            right -= 18.0;
        }
    }

    fn draw_cmdline(&mut self, f: &mut Frame, t: &Theme, r: Rect) {
        f.fill(r, t.editor);
        let mode = self.ws.vim.mode;
        let x = r.x + 12.0;
        let line = match mode {
            Mode::Command | Mode::Search { .. } => {
                let mut s = String::from(match mode {
                    Mode::Search { forward: false } => "?",
                    Mode::Search { .. } => "/",
                    _ => ":",
                });
                s.push_str(&self.ws.vim.cmdline);
                // A long command line shows its end, where the typing is.
                let mut shown = s.clone();
                while f.width(&shown, f.m.ui, 500) > r.w - 30.0 && shown.len() > 1 {
                    let mut cs = shown.chars();
                    cs.next();
                    shown = cs.collect();
                }
                let w = f.width(&shown, f.m.ui, 500);
                f.text_mid(&shown, x, r, f.m.ui, t.ink, 500);
                f.fill(
                    Rect {
                        x: x + w + 1.0,
                        y: r.y + 5.0,
                        w: 2.0,
                        h: r.h - 10.0,
                    },
                    t.cursor_insert,
                );
                s
            }
            _ => match self.ws.message() {
                Some((m, err)) => {
                    let shown = f.fit(m, f.m.ui, if err { 500 } else { 400 }, r.w - 24.0);
                    f.text_mid(
                        &shown,
                        x,
                        r,
                        f.m.ui,
                        if err { t.danger } else { t.ink },
                        if err { 500 } else { 400 },
                    );
                    String::from(m)
                }
                None => {
                    let hint = match (self.ws.focus, mode) {
                        (Focus::Tree, _) => "Files: j k move  Enter open  a new file  A new folder  r rename  d delete  Esc back",
                        (_, Mode::Insert) => "Typing. Esc for commands  Ctrl+S save  Ctrl+Shift+P all actions",
                        (_, Mode::Select) => "Selected. Type to replace it  Ctrl+C copy  Esc for commands",
                        (_, Mode::Visual) | (_, Mode::VisualLine) => "d delete  y copy  c change  > indent  Esc done",
                        _ if self.ws.docs.is_empty() => "Ctrl+Shift+P lists every action  : runs a command",
                        _ => "i type  v select  : command  / find  u undo  Ctrl+Shift+P all actions",
                    };
                    let shown = f.fit(hint, f.m.small, 400, r.w - 24.0);
                    f.text_mid(&shown, x, r, f.m.small, t.faint, 400);
                    String::from(hint)
                }
            },
        };
        f.said.text(r, &line);
    }

    fn overlay_card(&self, f: &mut Frame, t: &Theme, full: Rect, w: f32, h: f32, top: f32) -> Rect {
        f.fill(
            full,
            if t.dark {
                Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.45,
                }
            } else {
                Rgba {
                    r: 0.1,
                    g: 0.1,
                    b: 0.12,
                    a: 0.22,
                }
            },
        );
        let card = Rect {
            x: full.x + (full.w - w) * 0.5,
            y: top,
            w,
            h,
        };
        let _ = canvas2d::drop_shadow_round_rect(
            f.c,
            grect(Rect {
                y: card.y + 6.0,
                ..card
            }),
            radii(12.0),
            24.0,
            col(Rgba {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.35,
            }),
        );
        f.round(card, 12.0, t.raised);
        f.outline(card, 12.0, 1.0, t.line);
        card
    }

    fn draw_palette(&mut self, f: &mut Frame, t: &Theme, full: Rect) {
        let Some(st) = self.ws.palette.as_ref() else {
            return;
        };
        f.hit(full, Hit::PaletteBackdrop);
        let w = (full.w - 32.0).min(640.0);
        let rh = (f.m.ui * 2.3).round();
        let input_h = (f.m.ui * 3.2).round();
        let shown = st
            .items
            .len()
            .min(((full.h * 0.62 - input_h) / rh).floor().max(3.0) as usize)
            .max(1);
        let h = input_h + shown as f32 * rh + 12.0;
        let card = self.overlay_card(f, t, full, w, h, (full.h * 0.1).min(90.0));
        // Clicks inside the card are not the backdrop.
        f.hit(card, Hit::Card);
        let input = Rect {
            x: card.x + 12.0,
            y: card.y + 10.0,
            w: card.w - 24.0,
            h: input_h - 14.0,
        };
        f.round(input, 7.0, t.editor);
        f.outline(input, 7.0, 1.0, t.accent.with_alpha(0.7));
        let placeholder = match st.mode {
            PaletteMode::Commands => "Type to find an action",
            PaletteMode::Files => "Type to find a file",
            PaletteMode::Pick => st.title.as_str(),
        };
        let shown_q = if st.query.is_empty() {
            placeholder
        } else {
            st.query.as_str()
        };
        f.text_mid(
            shown_q,
            input.x + 12.0,
            input,
            f.m.ui,
            if st.query.is_empty() { t.faint } else { t.ink },
            400,
        );
        let qw = if st.query.is_empty() {
            0.0
        } else {
            f.width(&st.query, f.m.ui, 400)
        };
        f.fill(
            Rect {
                x: input.x + 12.0 + qw + 1.0,
                y: input.y + 7.0,
                w: 2.0,
                h: input.h - 14.0,
            },
            t.cursor_insert,
        );
        f.said.field(
            input,
            match st.mode {
                PaletteMode::Commands => "Find an action",
                PaletteMode::Files => "Find a file",
                PaletteMode::Pick => st.title.as_str(),
            },
            &st.query,
        );
        let top = st.selected.saturating_sub(shown.saturating_sub(1));
        let mut y = card.y + input_h;
        if st.items.is_empty() {
            let s = "Nothing matches";
            f.text_mid(
                s,
                card.x + 24.0,
                Rect {
                    x: card.x,
                    y,
                    w: card.w,
                    h: rh,
                },
                f.m.ui,
                t.muted,
                400,
            );
            f.said.text(
                Rect {
                    x: card.x,
                    y,
                    w: card.w,
                    h: rh,
                },
                s,
            );
        }
        for (k, &item) in st.items.iter().enumerate().skip(top).take(shown) {
            let row = Rect {
                x: card.x + 6.0,
                y,
                w: card.w - 12.0,
                h: rh,
            };
            if k == st.selected {
                f.round(row, 7.0, t.selection);
            }
            let (title, keys) = st.label(item);
            let needs = false;
            let shown = f.fit(title, f.m.ui, 600, row.w - 28.0);
            f.text_mid(
                &shown,
                row.x + 14.0,
                row,
                f.m.ui,
                t.ink,
                if k == st.selected { 600 } else { 400 },
            );
            let mut tail = String::from(keys);
            if needs {
                tail = String::from("coming");
            }
            if !tail.is_empty() {
                let kw = f.width(&tail, f.m.small, 500);
                let tw = f.width(title, f.m.ui, 600);
                if row.x + 14.0 + tw + 20.0 < row.right() - 14.0 - kw {
                    f.text_mid(
                        &tail,
                        row.right() - 14.0 - kw,
                        row,
                        f.m.small,
                        if needs { t.warn } else { t.muted },
                        500,
                    );
                }
            }
            f.button(row, title, Hit::PaletteRow(k));
            y += rh;
        }
    }

    fn draw_prompt(&mut self, f: &mut Frame, t: &Theme, full: Rect) {
        let Some(pr) = self.ws.prompt.clone() else {
            return;
        };
        let w = (full.w - 32.0).min(520.0);
        let bh = (f.m.ui * 2.6).round();
        let has_input = !pr.is_question();
        let title_lines = (f.width(&pr.title, f.m.ui, 400) / (w - 40.0))
            .ceil()
            .max(1.0);
        let title_h = title_lines * f.m.ui * 1.45;
        let mut h = 20.0 + title_h + 16.0 + bh + 20.0;
        if has_input {
            h += bh + 12.0;
        }
        if pr.error.is_some() {
            h += f.m.ui_h * 1.6;
        }
        let card = self.overlay_card(f, t, full, w, h, (full.h * 0.18).min(140.0));
        f.hit(full, Hit::PromptCancel);
        f.hit(card, Hit::Card);
        let mut y = card.y + 20.0;
        let used = self.wrap(f, &pr.title, card.x + 20.0, y, card.w - 40.0, f.m.ui, t.ink);
        y += used.max(title_h) + 16.0;
        if has_input {
            let input = Rect {
                x: card.x + 20.0,
                y,
                w: card.w - 40.0,
                h: bh,
            };
            f.round(input, 7.0, t.editor);
            f.outline(input, 7.0, 1.0, t.accent.with_alpha(0.7));
            let tw = f.width(&pr.text, f.m.ui, 400);
            f.clip(input);
            f.text_mid(&pr.text, input.x + 10.0, input, f.m.ui, t.ink, 400);
            f.fill(
                Rect {
                    x: input.x + 10.0 + tw + 1.0,
                    y: input.y + 7.0,
                    w: 2.0,
                    h: input.h - 14.0,
                },
                t.cursor_insert,
            );
            f.unclip();
            f.said.field(input, &pr.title, &pr.text);
            y += bh + 12.0;
        }
        if let Some(e) = &pr.error {
            f.text(e, card.x + 20.0, y, f.m.small, t.danger, 500);
            f.said.text(
                Rect {
                    x: card.x + 20.0,
                    y,
                    w: card.w - 40.0,
                    h: f.m.ui_h,
                },
                e,
            );
            y += f.m.ui_h * 1.6;
        }
        let (ok, alt, danger) = match pr.kind {
            PromptKind::Delete { .. } => ("Delete", None, true),
            PromptKind::CloseUnsaved => ("Save all", Some("Discard"), false),
            PromptKind::Rename { .. } => ("Rename", None, false),
            PromptKind::ProjectName => ("Make project", None, false),
            _ => ("Create", None, false),
        };
        let mut bx = card.right() - 20.0;
        let mut put = |f: &mut Frame, label: &str, hit: Hit, fill: Rgba, ink: Rgba| {
            let bw = f.width(label, f.m.ui, 600) + 32.0;
            bx -= bw;
            let b = Rect {
                x: bx,
                y,
                w: bw,
                h: bh,
            };
            f.round(b, 7.0, fill);
            f.text_mid(label, b.x + 16.0, b, f.m.ui, ink, 600);
            f.button(b, label, hit);
            bx -= 10.0;
        };
        put(
            f,
            ok,
            Hit::PromptOk,
            if danger { t.danger } else { t.accent },
            t.on_accent,
        );
        if let Some(a) = alt {
            put(f, a, Hit::PromptAlt, t.chrome, t.ink);
        }
        put(f, "Cancel", Hit::PromptCancel, t.chrome, t.ink);
    }

    fn draw_keys(&mut self, f: &mut Frame, t: &Theme, full: Rect) {
        let groups: [(&str, &[(&str, &str)]); 4] = [
            (
                "Modes",
                &[
                    ("i a o", "type before, after, on a new line"),
                    ("Esc", "back to commands, always"),
                    ("v V", "select by character, by line"),
                    (":", "a command: :w :q :e file :vsp"),
                    ("click", "start typing there (a setting)"),
                ],
            ),
            (
                "Move",
                &[
                    ("h j k l", "left down up right"),
                    ("w b e", "by words"),
                    ("0 ^ $", "line start, first letter, end"),
                    ("gg G 42G", "top, bottom, line 42"),
                    ("% f t", "matching bracket, to a letter"),
                ],
            ),
            (
                "Change",
                &[
                    ("d c y", "delete, change, copy + a motion"),
                    ("dd cc yy", "the whole line"),
                    ("x p P", "delete a letter, paste"),
                    ("u Ctrl+R", "undo, redo"),
                    (".", "repeat the last change"),
                ],
            ),
            (
                "Find",
                &[
                    ("/text n N", "find, next, previous"),
                    ("* #", "the word under the cursor"),
                    (":%s/a/b/g", "replace in the file"),
                    ("gd", "where this name is defined"),
                    ("Ctrl+Shift+P", "every action"),
                ],
            ),
        ];
        let w = (full.w - 32.0).min(760.0);
        let two = w > 560.0;
        let rh = f.m.ui_h * 1.45;
        let col_rows = if two { 12 } else { 24 };
        let h = (col_rows as f32 * rh + 90.0).min(full.h - 40.0);
        let card = self.overlay_card(f, t, full, w, h, 20.0f32.max((full.h - h) * 0.4));
        f.hit(full, Hit::KeysClose);
        f.text(
            "Keys",
            card.x + 22.0,
            card.y + 18.0,
            f.m.ui * 1.3,
            t.ink,
            650,
        );
        f.said.heading(
            Rect {
                x: card.x,
                y: card.y + 14.0,
                w: 200.0,
                h: 24.0,
            },
            "Keys",
        );
        let close = Rect {
            x: card.right() - 44.0,
            y: card.y + 12.0,
            w: 30.0,
            h: 28.0,
        };
        f.text_mid("x", close.x + 10.0, close, f.m.ui, t.muted, 500);
        f.button(close, "Close keys", Hit::KeysClose);
        let colw = if two { (w - 44.0) / 2.0 } else { w - 44.0 };
        f.clip(card);
        for (gi, (name, rows)) in groups.iter().enumerate() {
            let (cx, base) = if two {
                (
                    card.x + 22.0 + (gi % 2) as f32 * colw,
                    card.y + 56.0 + (gi / 2) as f32 * 6.0 * rh,
                )
            } else {
                (card.x + 22.0, card.y + 56.0 + gi as f32 * 6.0 * rh)
            };
            f.text(name, cx, base, f.m.small, t.accent, 700);
            for (k, (keys, what)) in rows.iter().enumerate() {
                let y = base + (k + 1) as f32 * rh;
                f.text(keys, cx, y, f.m.small, t.ink, 600);
                f.text(what, cx + colw * 0.36, y, f.m.small, t.muted, 400);
                let mut s = String::from(*keys);
                s.push_str(": ");
                s.push_str(what);
                f.said.text(
                    Rect {
                        x: cx,
                        y,
                        w: colw,
                        h: rh,
                    },
                    &s,
                );
            }
        }
        f.unclip();
    }

    // ---- input ---------------------------------------------------------------------

    fn pos_at(&self, x: f32, y: f32) -> Option<(usize, Pos)> {
        let g = self.panes.iter().find(|g| g.area.contains(x, y))?;
        let d = self.ws.panes.get(g.pane)?.doc?;
        let buf = &self.ws.docs.get(d)?.buf;
        let line =
            (g.first as f32 + ((y - g.text_y + g.offset) / self.lh).floor()).max(0.0) as usize;
        let line = line.min(buf.last_line());
        let colf = (x - g.text_x) / self.cw + g.left;
        let col = text::col_for_display_nearest(buf.line(line), colf.max(0.0), self.ws.vim.tab);
        Some((g.pane, Pos::new(line, col)))
    }

    fn press(&mut self, x: f32, y: f32, shift: bool) {
        let hit = self
            .hits
            .iter()
            .rev()
            .find(|(r, _)| r.contains(x, y))
            .map(|(_, h)| *h);
        if let Some(h) = hit {
            match h {
                Hit::Tab(i) => self.ws.click_tab(i),
                Hit::TabClose(i) => self.ws.close_tab(i),
                Hit::TreeRow(i) => {
                    self.ws.tree_click(i);
                    let is_file = self
                        .ws
                        .project
                        .as_ref()
                        .and_then(|p| p.tree.rows.get(i))
                        .is_some_and(|r| !r.is_dir);
                    if self.tree_sheet && is_file {
                        self.tree_sheet = false;
                        self.ws.focus = Focus::Editor;
                    }
                }
                Hit::TreeNewFile => {
                    self.ws.focus = Focus::Tree;
                    self.ws.run(Action::NewFile);
                }
                Hit::TreeNewFolder => {
                    self.ws.focus = Focus::Tree;
                    self.ws.run(Action::NewFolder);
                }
                Hit::TreeToggle => {
                    self.tree_sheet = !self.tree_sheet;
                    self.ws.settings.tree = true;
                    if !self.tree_sheet {
                        self.ws.focus = Focus::Editor;
                    }
                }
                Hit::PanelTab(tab) => {
                    self.ws.panel_tab = tab;
                    self.ws.focus = Focus::Panel;
                }
                Hit::PanelClose => self.ws.run(Action::TogglePanel),
                Hit::Problem(i) => self.ws.jump_to_problem(i),
                Hit::OpenPalette => self.ws.open_palette(PaletteMode::Commands),
                Hit::Card => {}
                Hit::PaletteRow(k) => self.ws.palette_choose(k),
                Hit::PaletteBackdrop => self.ws.palette = None,
                Hit::PromptOk => self.ws.prompt_confirm(),
                Hit::PromptAlt => self.ws.input(Input::Char('d')),
                Hit::PromptCancel => self.ws.prompt_cancel(),
                Hit::Act(a) => self.ws.run(a),
                Hit::ModeChip => self.ws.toggle_mode(),
                Hit::KeysClose => self.ws.show_keys = false,
                Hit::TermTab(i) => self.ws.select_terminal(i),
                Hit::TermClose(i) => self.ws.close_terminal(i),
                Hit::TermArea | Hit::OutputArea => {
                    let out = h == Hit::OutputArea;
                    let geo = if out { self.output_geo } else { self.term_geo };
                    self.ws.focus = Focus::Panel;
                    if let Some(g) = geo {
                        let c = g.cell(x, y);
                        self.ws.term_sel = Some((c, c));
                        self.sel_in_output = out;
                        self.grid_drag = true;
                    }
                }
                Hit::Frame(i) => self.ws.debug_select_frame(i),
                Hit::Var(i) => {
                    self.ws.focus = Focus::Panel;
                    self.ws.debug_toggle_var(i);
                }
                Hit::Completion(k) => {
                    if let Some(c) = self.ws.completion.as_mut() {
                        c.selected = k;
                    }
                    self.ws.input(Input::Key(Key::Tab));
                }
                Hit::Breakpoint(line) => {
                    if let Some((pane, _)) = self.pos_at(
                        self.panes
                            .iter()
                            .find(|g| g.area.contains(x, y))
                            .map(|g| g.text_x + 1.0)
                            .unwrap_or(x),
                        y,
                    ) {
                        self.ws.press_text(pane, Pos::new(line, 0), 1, false);
                        self.ws.release();
                        self.ws.run(Action::ToggleBreakpoint);
                    }
                }
                Hit::Gutter(line) => {
                    if let Some((pane, _)) = self.pos_at(
                        self.panes
                            .iter()
                            .find(|g| g.area.contains(x, y))
                            .map(|g| g.text_x + 1.0)
                            .unwrap_or(x),
                        y,
                    ) {
                        self.ws.press_text(pane, Pos::new(line, 0), 1, shift);
                    }
                }
            }
            return;
        }
        if let Some((pane, pos)) = self.pos_at(x, y) {
            let now = krate::time::now_millis();
            let (t0, lx, ly, n) = self.last_click;
            let clicks =
                if now.saturating_sub(t0) < 450 && (lx - x).abs() < 6.0 && (ly - y).abs() < 6.0 {
                    (n % 3) + 1
                } else {
                    1
                };
            self.last_click = (now, x, y, clicks);
            self.ws.press_text(pane, pos, clicks, shift);
        } else if self.panel_area.is_some_and(|r| r.contains(x, y)) {
            self.ws.focus = Focus::Panel;
        } else if self.tree_area.is_some_and(|r| r.contains(x, y)) {
            self.ws.focus = Focus::Tree;
        }
    }

    fn drag(&mut self) {
        if self.grid_drag {
            if !pointer::button_held(pointer::PointerButton::Primary) {
                self.grid_drag = false;
                return;
            }
            let geo = if self.sel_in_output {
                self.output_geo
            } else {
                self.term_geo
            };
            if let (Some((x, y)), Some(g), Some((a, _))) =
                (pointer::position(self.win), geo, self.ws.term_sel)
            {
                self.ws.term_sel = Some((a, g.cell(x, y)));
            }
            return;
        }
        if !self.ws.dragging() {
            return;
        }
        if !pointer::button_held(pointer::PointerButton::Primary) {
            self.ws.release();
            return;
        }
        let Some((x, y)) = pointer::position(self.win) else {
            return;
        };
        let Some(g) = self
            .panes
            .iter()
            .find(|g| g.pane == self.ws.active)
            .copied()
        else {
            return;
        };
        // Past the edge: scroll toward it.
        if y < g.area.y {
            self.ws.scroll(g.pane, -1.0, 0.0);
        } else if y > g.area.bottom() {
            self.ws.scroll(g.pane, 1.0, 0.0);
        }
        let cy = y.clamp(g.area.y + 1.0, g.area.bottom() - 1.0);
        let cx = x.clamp(g.text_x, g.area.right() - 1.0);
        if let Some((pane, pos)) = self.pos_at(cx, cy) {
            self.ws.drag_text(pane, pos);
        }
    }

    fn wheel(&mut self, x: f32, y: f32, dx: f32, dy: f32, shift: bool) {
        let (dx, dy) = if shift && dx == 0.0 {
            (dy, 0.0)
        } else {
            (dx, dy)
        };
        if let Some(st) = self.ws.palette.as_mut() {
            if dy > 0.0 && st.selected + 1 < st.items.len() {
                st.selected += 1;
            } else if dy < 0.0 {
                st.selected = st.selected.saturating_sub(1);
            }
            return;
        }
        if let Some(g) = self.term_geo.filter(|g| g.area.contains(x, y)) {
            let rows = (dy / g.ch).round() as isize;
            if rows == 0 {
                return;
            }
            let (c, r) = g.cell(x, y);
            let k = self.ws.term_active;
            let reply = self.ws.terminals.get(k).and_then(|tm| {
                tm.vt
                    .wheel_bytes(rows < 0, r, c.saturating_sub(g.first_abs))
            });
            match reply {
                Some(b) => {
                    let s = String::from_utf8_lossy(&b).into_owned();
                    self.ws.term_text(&s);
                }
                None => {
                    if let Some(tm) = self.ws.terminals.get_mut(k) {
                        tm.vt.scroll_view(-rows);
                    }
                }
            }
            return;
        }
        if let Some(g) = self.output_geo.filter(|g| g.area.contains(x, y)) {
            let rows = (dy / g.ch).round() as isize;
            self.ws.output.scroll_view(-rows);
            return;
        }
        if self.tree_area.is_some_and(|r| r.contains(x, y)) {
            if let Some(p) = self.ws.project.as_mut() {
                let n = p.tree.rows.len();
                let step = (dy / (self.lh.max(1.0))).round() as isize;
                let s = (p.tree.selected as isize + step).clamp(0, n.saturating_sub(1) as isize);
                p.tree.selected = s as usize;
            }
            return;
        }
        if let Some(g) = self.panes.iter().find(|g| g.area.contains(x, y)).copied() {
            self.ws
                .scroll(g.pane, dy / self.lh.max(1.0), dx / self.cw.max(1.0));
        }
    }
}

fn named_key(k: &str) -> Option<Key> {
    Some(match k {
        "Escape" => Key::Esc,
        "Enter" => Key::Enter,
        "Backspace" => Key::Backspace,
        "Delete" => Key::Delete,
        "Tab" => Key::Tab,
        "ArrowLeft" => Key::Left,
        "ArrowRight" => Key::Right,
        "ArrowUp" => Key::Up,
        "ArrowDown" => Key::Down,
        "Home" => Key::Home,
        "End" => Key::End,
        "PageUp" => Key::PageUp,
        "PageDown" => Key::PageDown,
        _ => return None,
    })
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

/// The quick run: a whole editing session with no window shown, then the
/// state, read back out of the editor.
fn quick(ui: &mut Ui) {
    let ws = &mut ui.ws;
    ws.open_tour();
    let mut out = String::new();
    let mut put = |k: &str, v: &str| {
        out.push_str(k);
        out.push(':');
        out.push_str(v);
        out.push('\n');
    };
    let keys = |ws: &mut Workspace, k: &str| {
        for i in vim::parse_keys(k) {
            ws.input(i);
        }
    };
    let lines0 = ws.active_doc().map(|d| d.buf.line_count()).unwrap_or(0);
    keys(ws, "ggdd");
    let after_dd = ws.active_doc().map(|d| d.buf.line_count()).unwrap_or(0);
    keys(ws, "u");
    let after_u = ws.active_doc().map(|d| d.buf.line_count()).unwrap_or(0);
    keys(ws, "/total_area<CR>");
    let found = ws.panes[ws.active].cur.pos.line + 1;
    keys(ws, ":%s/Scene/Stage/g<CR>");
    let replaced = ws
        .message()
        .map(|(m, _)| String::from(m))
        .unwrap_or_default();
    keys(ws, "ggO// edited in Krate Code<Esc>:w<CR>");
    let saved = ws.active_doc().is_some_and(|d| !d.buf.modified());
    keys(ws, "<C-w>v:e src/shapes.rs<CR>");
    ws.run(Action::Tool(Tool::Build));
    put("lines", &text::num(lines0));
    put("after_dd", &text::num(after_dd));
    put("after_undo", &text::num(after_u));
    put("found_line", &text::num(found));
    put("substitute", &replaced);
    put("saved", if saved { "yes" } else { "no" });
    put("build", ws.message().map(|(m, _)| m).unwrap_or(""));
    out.push_str(&ws.report());
    ui.draw();
    let _ = krate::io::stdio::write(out.as_bytes());
}

struct Component;

impl krate::Guest for Component {
    fn run() -> i32 {
        let Ok(win) = window::create(
            "Krate Code",
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
        let Ok(canvas) = canvas2d::bind(win, CANVAS_ID) else {
            let _ = window::close(win);
            return 33;
        };
        let mut ws = Workspace::new();
        if let Ok(Some(saved)) = krate::store::get_text(SETTINGS_KEY) {
            ws.apply_settings(Settings::from_text(&saved));
        }
        let mut ui = Ui {
            win,
            canvas,
            ws,
            hits: Vec::new(),
            panes: Vec::new(),
            tree_area: None,
            panel_area: None,
            lh: 20.0,
            cw: 8.0,
            tree_sheet: false,
            last_click: (0, 0.0, 0.0, 0),
            title: String::new(),
            term_geo: None,
            output_geo: None,
            sel_in_output: false,
            grid_drag: false,
            rest: (0.0, 0.0, 0, true),
            last_output: 0,
        };
        ui.ws.runner = Box::new(run::ProcessRunner::default());
        let raw = krate::io::args::raw();
        if raw.split('\n').next() == Some("quick") {
            quick(&mut ui);
            let _ = window::close(win);
            return 0;
        }
        // `open=.`: open the app's own folder as the project. For automated
        // runs, which cannot answer a folder picker; it needs the fs grant a
        // test manifest gives, which the shipped one does not ask for.
        if raw.split('\n').any(|a| a == "open=.") {
            ui.ws.now_ms = krate::time::now_millis();
            ui.ws.open_project("project", Box::new(disk::DiskFs::own()));
        }
        let _ = krate::ui::text_entry::start(win);
        ui.draw();

        // Keys pressed with Cmd/Ctrl: their letter may also come as text.
        let mut drop_text: Option<char> = None;
        let mut paste_next = false;
        let mut shift = false;
        loop {
            let now = krate::time::now_millis();
            ui.ws.now_ms = now;
            let timeout = if ui.ws.dragging() || ui.grid_drag {
                Some(16)
            } else if ui.ws.busy() {
                // Quick while a program prints, easier while it waits.
                if now.saturating_sub(ui.last_output) < 1500 {
                    Some(16)
                } else {
                    Some(60)
                }
            } else if ui.ws.project.as_ref().is_some_and(|p| p.fs.is_real()) {
                Some(300)
            } else {
                None
            };
            let mut ev = events::wait(timeout);
            ui.ws.now_ms = krate::time::now_millis();
            let mut changed = false;
            let mut close = false;
            while let Some(e) = ev {
                changed = true;
                match e {
                    types::Event::CloseRequested(_) => {
                        if ui.ws.close_requested() {
                            close = true;
                        }
                    }
                    types::Event::Key(k) => {
                        let m = k.modifiers;
                        shift = m.shift;
                        if !k.pressed {
                            ev = events::poll();
                            continue;
                        }
                        let primary = m.control || m.meta;
                        let single = {
                            let mut cs = k.key.chars();
                            match (cs.next(), cs.next()) {
                                (Some(c), None) => Some(c),
                                _ => None,
                            }
                        };
                        if ui.ws.terminal_focused() {
                            // The terminal has the keyboard. Cmd chords (and
                            // Ctrl+Shift+C/V) stay the editor's; Ctrl with a
                            // key is the terminal's own (Ctrl+C stops a
                            // program).
                            let lower = single.map(|c| c.to_ascii_lowercase());
                            let editor_chord = single.is_some()
                                && (m.meta
                                    || (m.control
                                        && m.shift
                                        && matches!(lower, Some('c' | 'v' | 'p'))));
                            if primary && lower == Some('`') {
                                drop_text = lower;
                                ui.ws.toggle_terminal();
                            } else if editor_chord {
                                let c = lower.unwrap_or(' ');
                                drop_text = Some(c);
                                match c {
                                    'c' => {
                                        ui.ws.term_copy();
                                    }
                                    'v' => paste_next = true,
                                    _ => {
                                        ui.ws.shortcut(c, m.shift && m.meta);
                                    }
                                }
                            } else if m.control && single.is_some() {
                                drop_text = lower;
                                ui.ws.term_key(&k.key, m.shift, true, m.alt);
                            } else if single.is_none() {
                                ui.ws.term_key(&k.key, m.shift, m.control, m.alt);
                            }
                            changed = true;
                            ev = events::poll();
                            continue;
                        }
                        if let Some(n) = k.key.strip_prefix('F').and_then(|r| r.parse::<u8>().ok())
                        {
                            ui.ws.function_key(n, m.shift);
                        } else if primary && single.is_some() {
                            let c = single.unwrap_or(' ').to_ascii_lowercase();
                            drop_text = Some(c);
                            if c == 'v' && !m.shift {
                                paste_next = true;
                            }
                            if !ui.ws.shortcut(c, m.shift) {
                                ui.ws.input(Input::Ctrl(c));
                            }
                        } else if primary && k.key == "Space" {
                            ui.ws.tool(Tool::Complete);
                        } else if let Some(nk) = named_key(&k.key) {
                            drop_text = None;
                            paste_next = false;
                            let shifted = m.shift
                                && matches!(
                                    nk,
                                    Key::Left
                                        | Key::Right
                                        | Key::Up
                                        | Key::Down
                                        | Key::Home
                                        | Key::End
                                );
                            ui.ws.input(if shifted {
                                Input::Shift(nk)
                            } else {
                                Input::Key(nk)
                            });
                        } else if single.is_some() || k.key == "Space" {
                            // A typed character: it arrives again as text.
                            if drop_text.is_some() && !primary {
                                drop_text = None;
                            }
                        }
                    }
                    types::Event::TextInput(t) if ui.ws.terminal_focused() => {
                        if drop_text.is_some_and(|d| {
                            t.chars().count() == 1 && t.to_lowercase().starts_with(d)
                        }) {
                            drop_text = None;
                        } else if paste_next {
                            paste_next = false;
                            drop_text = None;
                            ui.ws.term_paste(&t);
                        } else {
                            let clean: String = t.chars().filter(|c| !c.is_control()).collect();
                            if !clean.is_empty() {
                                ui.ws.term_text(&clean);
                            }
                        }
                    }
                    types::Event::TextInput(t) => {
                        let clean: String = t
                            .chars()
                            .filter(|c| !c.is_control() || *c == '\n')
                            .collect();
                        if clean.is_empty() {
                        } else if drop_text.is_some_and(|d| {
                            clean.len() == 1
                                && clean.eq_ignore_ascii_case(d.encode_utf8(&mut [0; 4]))
                        }) {
                            drop_text = None;
                        } else if paste_next {
                            paste_next = false;
                            drop_text = None;
                            ui.ws.input(Input::Paste(t.replace("\r\n", "\n")));
                        } else {
                            for c in clean.chars() {
                                if c == '\n' {
                                    ui.ws.input(Input::Key(Key::Enter));
                                } else {
                                    ui.ws.input(Input::Char(c));
                                }
                            }
                        }
                    }
                    types::Event::Pointer(p) => {
                        if p.button == Some(types::PointerButton::Primary) {
                            if p.pressed {
                                ui.press(p.x, p.y, p.modifiers.shift || shift);
                            } else {
                                ui.drag();
                                ui.ws.release();
                            }
                        }
                    }
                    types::Event::Wheel(w) => ui.wheel(w.x, w.y, w.dx, w.dy, w.modifiers.shift),
                    _ => {}
                }
                ev = events::poll();
            }
            if close {
                break;
            }
            if ui.ws.dragging() {
                ui.drag();
                changed = true;
            }
            if let Some(req) = ui.ws.host.take() {
                changed = true;
                match req {
                    HostRequest::Close => break,
                    HostRequest::PickFolder | HostRequest::PickFolderForNewProject => {
                        let title = if req == HostRequest::PickFolder {
                            "Open a project folder"
                        } else {
                            "Choose a folder for the new project"
                        };
                        match krate::ui::dialog::open_folder(win, title) {
                            Ok(Some(folder)) => {
                                let fs = Box::new(disk::DiskFs::new(&folder.token));
                                if req == HostRequest::PickFolder {
                                    ui.ws.open_project(&folder.name, fs);
                                } else {
                                    ui.ws.new_project_folder(&folder.name, fs);
                                }
                            }
                            Ok(None) => {}
                            Err(_) => {
                                ui.ws.status = Some((String::from("Krate did not let this app open a folder here (a browser tab cannot)"), true));
                            }
                        }
                    }
                }
            }
            if let Some(p) = ui.ws.project.as_mut() {
                if p.fs.changed() {
                    p.reload();
                    changed = true;
                }
            }
            if ui.ws.poll_runner() {
                changed = true;
                ui.last_output = krate::time::now_millis();
            }
            if let Some(text) = ui.ws.clipboard_out.take() {
                let _ = krate::ui::clipboard::write_text(&text);
            }
            // Resting the mouse over a name asks rust-analyzer about it.
            if ui.ws.lsp.is_some() && !ui.grid_drag && !ui.ws.dragging() {
                if let Some((px, py)) = pointer::position(win) {
                    let (rx, ry, since, asked) = ui.rest;
                    let now = krate::time::now_millis();
                    if (px - rx).abs() > 3.0 || (py - ry).abs() > 3.0 {
                        ui.rest = (px, py, now, false);
                        if ui.ws.hover.as_ref().is_some_and(|h| h.from_mouse) {
                            ui.ws.hover = None;
                            changed = true;
                        }
                    } else if !asked && now.saturating_sub(since) > 650 {
                        ui.rest.3 = true;
                        let over_card = ui.ws.completion.is_some() || ui.ws.palette.is_some();
                        if !over_card {
                            if let Some((pane, pos)) = ui.pos_at(px, py) {
                                ui.ws.hover_at(pane, pos);
                            }
                        }
                    }
                }
            }
            if ui.ws.settings_dirty {
                ui.ws.settings_dirty = false;
                if krate::store::set_text(SETTINGS_KEY, &ui.ws.settings.to_text()).is_err() {
                    ui.ws.status = Some((
                        String::from("Could not keep the settings for next time"),
                        true,
                    ));
                }
            }
            if changed {
                ui.draw();
            }
        }
        let _ = window::close(win);
        let _ = krate::io::stdio::write(ui.ws.report().as_bytes());
        0
    }
}

krate::export!(Component);
