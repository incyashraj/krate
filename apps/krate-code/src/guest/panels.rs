//! Drawing what the IDE half adds: a terminal grid (the Terminal and Output
//! panels both are one), the debugger's panel, and the popups over the code
//! -- completion, hover and signature help.

use super::{col, grect, style, Frame, Hit, Ui};
use crate::dap::DapState;
use crate::ide::Term;
use crate::text;
use crate::theme::{Rgba, Theme};
use crate::tools::Tool;
use crate::vt::{self, Attr, Color, Vt};
use crate::workspace::PanelTab;
use alloc::string::String;
use alloc::vec::Vec;
use krate::gfx::canvas2d;
use krate::gfx::types as gfx;
use krate::layout::Rect;
use krate::math::F32Ext;

/// Where a grid was drawn, for the mouse.
#[derive(Debug, Clone, Copy)]
pub struct GridGeo {
    pub area: Rect,
    pub cw: f32,
    pub ch: f32,
    /// Absolute row (from the first line of history) of the top visible row.
    pub first_abs: usize,
    pub rows: usize,
    pub cols: usize,
}

impl GridGeo {
    /// The cell under a point: (absolute row, column).
    pub fn cell(&self, x: f32, y: f32) -> (usize, usize) {
        let c = ((x - self.area.x) / self.cw).floor().max(0.0) as usize;
        let r = ((y - self.area.y) / self.ch).floor().max(0.0) as usize;
        (
            self.first_abs + r.min(self.rows.saturating_sub(1)),
            c.min(self.cols.saturating_sub(1)),
        )
    }
}

fn rgba(c: (u8, u8, u8)) -> Rgba {
    Rgba {
        r: c.0 as f32 / 255.0,
        g: c.1 as f32 / 255.0,
        b: c.2 as f32 / 255.0,
        a: 1.0,
    }
}

fn resolve(t: &Theme, c: Color, fallback: Rgba) -> Rgba {
    match c {
        Color::Default => fallback,
        Color::Idx(i) if i < 16 => t.ansi(i),
        Color::Idx(i) => rgba(vt::xterm_rgb(i)),
        Color::Rgb(r, g, b) => rgba((r, g, b)),
    }
}

/// The colours a cell is drawn in: (text, background if not the panel's).
fn colours(t: &Theme, a: &Attr, panel: Rgba) -> (Rgba, Option<Rgba>) {
    let mut fg = resolve(t, a.fg, t.ink);
    let mut bg = match a.bg {
        Color::Default => None,
        c => Some(resolve(t, c, panel)),
    };
    if a.inverse {
        let b = bg.unwrap_or(panel);
        bg = Some(fg);
        fg = b;
    }
    if a.dim {
        fg = fg.mix(bg.unwrap_or(panel), 0.45);
    }
    if a.hidden {
        fg = bg.unwrap_or(panel);
    }
    (fg, bg)
}

/// Draw a grid of cells into `area`. `cursor`: draw the cursor (filled when
/// the grid has the keyboard). Returns where it was drawn.
#[allow(clippy::too_many_arguments)]
pub fn draw_grid(
    f: &mut Frame,
    t: &Theme,
    v: &Vt,
    area: Rect,
    ch: f32,
    panel: Rgba,
    cursor: Option<bool>,
    sel: Option<((usize, usize), (usize, usize))>,
) -> GridGeo {
    let cw = f.m.cw;
    let rows = v.rows;
    let first_abs = v.scrollback.len() - v.view.min(v.scrollback.len());
    let size = f.m.code;
    let base = (ch - size * 1.2) * 0.5 + f.m.ascent;
    let sel = sel.map(|(a, b)| if a <= b { (a, b) } else { (b, a) });
    f.clip(area);
    for i in 0..rows {
        let y = area.y + i as f32 * ch;
        if y > area.bottom() {
            break;
        }
        let row = v.view_row(i);
        let abs = first_abs + i;
        // Backgrounds and selection first, then text over them.
        let mut x_cell = 0;
        while x_cell < row.len() {
            let c = &row[x_cell];
            let (_, bg) = colours(t, &c.a, panel);
            let in_sel = sel.is_some_and(|((r0, c0), (r1, c1))| {
                (abs > r0 || (abs == r0 && x_cell >= c0))
                    && (abs < r1 || (abs == r1 && x_cell <= c1))
            });
            let bg = if in_sel { Some(t.selection) } else { bg };
            if let Some(b) = bg {
                // Merge equal neighbours into one rectangle.
                let mut end = x_cell + 1;
                while end < row.len() {
                    let (_, nb) = colours(t, &row[end].a, panel);
                    let ns = sel.is_some_and(|((r0, c0), (r1, c1))| {
                        (abs > r0 || (abs == r0 && end >= c0))
                            && (abs < r1 || (abs == r1 && end <= c1))
                    });
                    let nb = if ns { Some(t.selection) } else { nb };
                    if nb != Some(b) {
                        break;
                    }
                    end += 1;
                }
                f.fill(
                    Rect {
                        x: area.x + x_cell as f32 * cw,
                        y,
                        w: (end - x_cell) as f32 * cw + 0.5,
                        h: ch,
                    },
                    b,
                );
                x_cell = end;
            } else {
                x_cell += 1;
            }
        }
        let mut said = String::new();
        let mut k = 0;
        while k < row.len() {
            let c = &row[k];
            if c.w == 0 || c.c == ' ' && !c.a.underline {
                if c.w != 0 {
                    said.push(' ');
                }
                k += 1;
                continue;
            }
            // A run of cells drawn alike; a wide character is its own run so
            // the columns stay true.
            let start = k;
            let mut s = String::new();
            let a = c.a;
            if c.w == 2 {
                s.push(c.c);
                k += 2;
            } else {
                while k < row.len() && row[k].a == a && row[k].w == 1 {
                    s.push(row[k].c);
                    k += 1;
                }
            }
            said.push_str(&s);
            let (fg, _) = colours(t, &a, panel);
            let x = area.x + start as f32 * cw;
            let _ = canvas2d::draw_text_styled(
                f.c,
                s.trim_end(),
                gfx::Point { x, y: y + base },
                size,
                col(fg),
                style(if a.bold { 700 } else { 400 }, true, a.italic),
            );
            if a.underline || a.strike {
                let n = s.chars().count() as f32;
                f.fill(
                    Rect {
                        x,
                        y: if a.underline {
                            y + ch - 3.0
                        } else {
                            y + ch * 0.5
                        },
                        w: n * cw,
                        h: 1.0,
                    },
                    fg,
                );
            }
        }
        let said = said.trim_end();
        if !said.is_empty() {
            f.said.text(
                Rect {
                    x: area.x,
                    y,
                    w: area.w,
                    h: ch,
                },
                said,
            );
        }
    }
    if let Some(focused) = cursor {
        if v.cursor_visible && v.view == 0 && v.y < rows {
            let r = Rect {
                x: area.x + v.x as f32 * cw,
                y: area.y + v.y as f32 * ch,
                w: cw,
                h: ch,
            };
            if focused {
                f.fill(r, t.cursor_normal.with_alpha(0.85));
                let cell = v.row(v.y).get(v.x).copied();
                if let Some(c) = cell.filter(|c| c.c != ' ' && c.w != 0) {
                    let mut s = String::new();
                    s.push(c.c);
                    let _ = canvas2d::draw_text_styled(
                        f.c,
                        &s,
                        gfx::Point {
                            x: r.x,
                            y: r.y + base,
                        },
                        size,
                        col(panel),
                        style(400, true, false),
                    );
                }
            } else {
                f.outline(r, 1.0, 1.0, t.cursor_normal);
            }
        }
    }
    f.unclip();
    GridGeo {
        area,
        cw,
        ch,
        first_abs,
        rows,
        cols: v.cols,
    }
}

impl Ui {
    /// The grid's row height: tighter than the editor's, like a terminal.
    pub(super) fn cell_h(f: &Frame) -> f32 {
        (f.m.code * 1.32).round()
    }

    pub(super) fn draw_output(&mut self, f: &mut Frame, t: &Theme, body: Rect) {
        let ch = Self::cell_h(f);
        let cols = ((body.w - 4.0) / f.m.cw).floor().max(20.0) as usize;
        let rows = ((body.h - 2.0) / ch).floor().max(2.0) as usize;
        self.ws.output.resize(cols, rows);
        if self.ws.output_lines().is_empty() {
            let s = "Builds, runs and tests print here as they go. F7 builds, F5 runs.";
            f.text(s, body.x, body.y, f.m.ui, t.muted, 400);
            f.said.text(
                Rect {
                    x: body.x,
                    y: body.y,
                    w: body.w,
                    h: f.m.ui_h,
                },
                s,
            );
            self.output_geo = None;
            return;
        }
        let sel = if self.sel_in_output {
            self.ws.term_sel
        } else {
            None
        };
        let g = draw_grid(f, t, &self.ws.output, body, ch, t.chrome, None, sel);
        f.hit(body, Hit::OutputArea);
        self.output_geo = Some(g);
    }

    pub(super) fn draw_terminal_panel(&mut self, f: &mut Frame, t: &Theme, body: Rect) {
        let mut body = body;
        let ch = Self::cell_h(f);
        // The tabs: one per terminal, and a button for another.
        let strip = body.take_top((f.m.ui * 1.9).round());
        let mut x = strip.x;
        let tabs: Vec<(usize, String, bool)> = self
            .ws
            .terminals
            .iter()
            .enumerate()
            .map(|(i, tm)| (i, term_title(tm), tm.ended))
            .collect();
        for (i, name, ended) in &tabs {
            let w = f.width(name, f.m.small, 600) + 40.0;
            let r = Rect {
                x,
                y: strip.y + 2.0,
                w,
                h: strip.h - 6.0,
            };
            let on = *i == self.ws.term_active;
            if on {
                f.round(r, 5.0, t.raised);
            }
            f.text_mid(
                name,
                r.x + 10.0,
                r,
                f.m.small,
                if *ended {
                    t.faint
                } else if on {
                    t.ink
                } else {
                    t.muted
                },
                600,
            );
            f.button(r, name, Hit::TermTab(*i));
            let close = Rect {
                x: r.right() - 22.0,
                y: r.y + 2.0,
                w: 18.0,
                h: r.h - 4.0,
            };
            f.text_mid("x", close.x + 5.0, close, f.m.small, t.faint, 500);
            let mut said = String::from("Close ");
            said.push_str(name);
            f.button(close, &said, Hit::TermClose(*i));
            x += w + 4.0;
        }
        let plus = Rect {
            x,
            y: strip.y + 2.0,
            w: f.width("+ New terminal", f.m.small, 500) + 18.0,
            h: strip.h - 6.0,
        };
        f.text_mid(
            "+ New terminal",
            plus.x + 9.0,
            plus,
            f.m.small,
            t.muted,
            500,
        );
        f.button(
            plus,
            "New terminal",
            Hit::Act(crate::palette::Action::Tool(Tool::Terminal)),
        );

        let cols = ((body.w - 4.0) / f.m.cw).floor().max(10.0) as u16;
        let rows = ((body.h - 2.0) / ch).floor().max(3.0) as u16;
        self.ws.term_resize(cols, rows);
        let focused = self.ws.terminal_focused();
        let k = self.ws.term_active;
        match self.ws.terminals.get(k) {
            None => {
                let a = "No terminal open.";
                let b = "A terminal runs your own shell in the project folder: Ctrl+` opens one, Esc twice comes back to the code.";
                f.text(a, body.x, body.y + 4.0, f.m.ui, t.ink, 500);
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
                    body.y + f.m.ui_h * 1.6,
                    body.w,
                    f.m.small,
                    t.muted,
                );
                self.term_geo = None;
            }
            Some(tm) => {
                let sel = if self.sel_in_output {
                    None
                } else {
                    self.ws.term_sel
                };
                let g = draw_grid(f, t, &tm.vt, body, ch, t.chrome, Some(focused), sel);
                f.hit(body, Hit::TermArea);
                self.term_geo = Some(g);
                if focused {
                    f.outline(body.pad(-2.0), 4.0, 1.0, t.accent.with_alpha(0.5));
                }
            }
        }
    }

    pub(super) fn draw_debug_panel(&mut self, f: &mut Frame, t: &Theme, body: Rect) {
        let mut body = body;
        let bar = body.take_top((f.m.ui * 2.1).round());
        let state = self.ws.dap.as_ref().map(|d| d.client.state);
        let mut x = bar.x;
        let buttons = [
            ("Continue  F5", Tool::Continue),
            ("Step over  F10", Tool::StepOver),
            ("Step into  F11", Tool::StepIn),
            ("Step out  Shift+F11", Tool::StepOut),
            ("Stop  Shift+F5", Tool::StopDebug),
        ];
        let stopped = state == Some(DapState::Stopped);
        for (label, tool) in buttons {
            let w = f.width(label, f.m.small, 600) + 20.0;
            let r = Rect {
                x,
                y: bar.y + 3.0,
                w,
                h: bar.h - 8.0,
            };
            let live = stopped || tool == Tool::StopDebug && state.is_some();
            f.round(r, 5.0, if live { t.raised } else { t.chrome });
            f.text_mid(
                label,
                r.x + 10.0,
                r,
                f.m.small,
                if live { t.ink } else { t.faint },
                600,
            );
            f.button(r, label, Hit::Act(crate::palette::Action::Tool(tool)));
            x += w + 6.0;
        }
        let words = match state {
            None => "Not debugging: Shift+F5 builds the project and starts it in the debugger; F9 sets a breakpoint",
            Some(DapState::Starting) => "Starting the debugger...",
            Some(DapState::Running) => "Running -- it stops at the next breakpoint",
            Some(DapState::Stopped) => "",
            Some(DapState::Ended) => "The program ended",
        };
        if !words.is_empty() {
            let shown = f.fit(words, f.m.small, 400, (bar.right() - x - 10.0).max(40.0));
            f.text_mid(&shown, x + 6.0, bar, f.m.small, t.muted, 400);
            f.said.text(bar, words);
        }
        let Some(d) = self.ws.dap.as_ref() else {
            return;
        };
        let left_w = (body.w * 0.38).clamp(160.0, 420.0);
        let mut right = body;
        let left = right.take_left(left_w);
        f.fill(
            Rect {
                x: left.right(),
                y: body.y,
                w: 1.0,
                h: body.h,
            },
            t.line,
        );
        let rh = f.m.ui_h * 1.35;
        f.text("CALL STACK", left.x, left.y, f.m.small, t.faint, 700);
        f.text(
            "VARIABLES",
            right.x + 12.0,
            right.y,
            f.m.small,
            t.faint,
            700,
        );
        let top = left.y + f.m.ui_h * 1.3;
        for (i, fr) in d.client.frames.iter().enumerate() {
            let y = top + i as f32 * rh;
            if y + rh > left.bottom() {
                break;
            }
            let row = Rect {
                x: left.x - 4.0,
                y,
                w: left.w - 4.0,
                h: rh,
            };
            if i == d.client.selected {
                f.round(row, 4.0, t.raised);
            }
            let mut label = fr.name.clone();
            if let Some(p) = &fr.path {
                label.push_str("  ");
                label.push_str(crate::project::file_name(p));
                label.push(':');
                label.push_str(&text::num(fr.line + 1));
            }
            let shown = f.fit(&label, f.m.small, 500, row.w - 12.0);
            f.text_mid(
                &shown,
                row.x + 6.0,
                row,
                f.m.small,
                if fr.path.is_some() { t.ink } else { t.muted },
                500,
            );
            f.button(row, &label, Hit::Frame(i));
        }
        let vx = right.x + 12.0;
        let top = right.y + f.m.ui_h * 1.3;
        for (i, v) in d.client.vars.iter().enumerate() {
            let y = top + i as f32 * rh;
            if y + rh > right.bottom() {
                break;
            }
            let row = Rect {
                x: vx - 4.0,
                y,
                w: right.right() - vx,
                h: rh,
            };
            if i == self.ws.var_selected && self.ws.focus == crate::workspace::Focus::Panel {
                f.round(row, 4.0, t.raised);
            }
            let ind = vx + v.depth as f32 * 16.0;
            if v.children != 0 {
                f.text_mid(
                    if v.open { "v" } else { ">" },
                    ind,
                    row,
                    f.m.small,
                    t.muted,
                    700,
                );
            }
            let nx = ind + 14.0;
            f.text_mid(&v.name, nx, row, f.m.small, t.keyword, 600);
            let mut vxp = nx + f.width(&v.name, f.m.small, 600) + 10.0;
            let mut said = v.name.clone();
            said.push_str(" = ");
            said.push_str(&v.value);
            let val = f.fit(
                &v.value,
                f.m.small,
                400,
                (row.right() - vxp - 100.0).max(40.0),
            );
            f.text_mid(&val, vxp, row, f.m.small, t.ink, 400);
            vxp += f.width(&val, f.m.small, 400) + 10.0;
            if !v.ty.is_empty() {
                let ty = f.fit(&v.ty, f.m.small, 400, (row.right() - vxp - 6.0).max(20.0));
                f.text_mid(&ty, vxp, row, f.m.small, t.faint, 400);
                said.push_str(": ");
                said.push_str(&v.ty);
            }
            f.button(row, &said, Hit::Var(i));
        }
        if d.client.vars.is_empty() && stopped {
            f.text("No local variables here", vx, top, f.m.small, t.muted, 400);
        }
    }

    /// The popups over the code: completion, hover, signature help.
    pub(super) fn draw_popups(&mut self, f: &mut Frame, t: &Theme, full: Rect) {
        let Some(g) = f.panes.iter().find(|g| g.pane == self.ws.active).copied() else {
            return;
        };
        let Some(doc) = self.ws.active_doc() else {
            return;
        };
        let cur = self.ws.panes[self.ws.active].cur.pos;
        let path = doc.path.clone();
        let place = |line: usize, colb: usize, f: &Frame| -> Option<(f32, f32)> {
            let doc = self.ws.active_doc()?;
            let lt = doc.buf.line(line.min(doc.buf.last_line()));
            let dc = text::display_col(lt, colb.min(lt.len()), self.ws.vim.tab) as f32;
            let x = g.text_x + (dc - g.left) * f.m.cw;
            let y = g.text_y + (line as f32 - g.first as f32) * f.m.lh - g.offset;
            if y < g.area.y - f.m.lh || y > g.area.bottom() {
                return None;
            }
            Some((x, y))
        };
        // Signature help sits above the line being typed.
        if let Some(s) = self.ws.signature.clone() {
            if s.path == path && s.line == cur.line {
                if let Some((x, y)) = place(cur.line, cur.col, f) {
                    let w = (s.label.chars().count() as f32 * f.m.cw + 20.0).min(full.w - 16.0);
                    let h = f.m.lh + 6.0;
                    let r = Rect {
                        x: x.min(full.right() - w - 8.0).max(8.0),
                        y: (y - h - 4.0).max(4.0),
                        w,
                        h,
                    };
                    self.card(f, t, r);
                    let ty = r.y + 3.0;
                    match s.active {
                        Some((a, b)) if b <= s.label.len() && a <= b => {
                            let pre = &s.label[..a];
                            let mid = &s.label[a..b];
                            let post = &s.label[b..];
                            let mut xx = r.x + 10.0;
                            f.code(pre, xx, ty, t.muted, false);
                            xx += pre.chars().count() as f32 * f.m.cw;
                            f.code(mid, xx, ty, t.accent, false);
                            f.fill(
                                Rect {
                                    x: xx,
                                    y: ty + f.m.lh - 4.0,
                                    w: mid.chars().count() as f32 * f.m.cw,
                                    h: 1.5,
                                },
                                t.accent,
                            );
                            xx += mid.chars().count() as f32 * f.m.cw;
                            f.code(post, xx, ty, t.muted, false);
                        }
                        _ => f.code(&s.label, r.x + 10.0, ty, t.muted, false),
                    }
                    f.said.text(r, &s.label);
                }
            }
        }
        // Completion below the cursor.
        if let Some(c) = self.ws.completion.clone() {
            if c.path == path {
                if let Some((x, y)) = place(cur.line, c.start, f) {
                    let n = c.shown.len().min(10);
                    let rh = (f.m.ui_h * 1.45).round();
                    let w = 440.0f32.min(full.w - 16.0);
                    let h = n as f32 * rh + 8.0;
                    let below = y + f.m.lh + 2.0;
                    let top = if below + h > full.bottom() - 8.0 {
                        (y - h - 2.0).max(4.0)
                    } else {
                        below
                    };
                    let r = Rect {
                        x: (x - 28.0).min(full.right() - w - 8.0).max(8.0),
                        y: top,
                        w,
                        h,
                    };
                    self.card(f, t, r);
                    // Keep the selected row in view.
                    let first = c.selected.saturating_sub(n.saturating_sub(1));
                    for (k, &i) in c.shown.iter().enumerate().skip(first).take(n) {
                        let it = &c.items[i];
                        let row = Rect {
                            x: r.x + 4.0,
                            y: r.y + 4.0 + (k - first) as f32 * rh,
                            w: r.w - 8.0,
                            h: rh,
                        };
                        if k == c.selected {
                            f.round(row, 5.0, t.selection);
                        }
                        let kind = it.kind_word();
                        f.text_mid(kind, row.x + 8.0, row, f.m.small, t.faint, 600);
                        let lx = row.x + 62.0;
                        let label = f.fit(&it.label, f.m.ui, 500, row.w * 0.55);
                        let _ = canvas2d::draw_text_styled(
                            f.c,
                            &label,
                            gfx::Point {
                                x: lx,
                                y: row.y + (row.h - f.m.code * 1.2) * 0.5 + f.m.ascent,
                            },
                            f.m.code,
                            col(if k == c.selected { t.ink } else { t.ty }),
                            style(if k == c.selected { 600 } else { 400 }, true, false),
                        );
                        let lw = label.chars().count() as f32 * f.m.cw;
                        if !it.detail.is_empty() {
                            let room = row.right() - (lx + lw + 16.0) - 8.0;
                            if room > 40.0 {
                                let d = f.fit(&it.detail, f.m.small, 400, room);
                                let dw = f.width(&d, f.m.small, 400);
                                f.text_mid(
                                    &d,
                                    row.right() - dw - 8.0,
                                    row,
                                    f.m.small,
                                    t.muted,
                                    400,
                                );
                            }
                        }
                        let mut said = String::from(&it.label);
                        if !it.detail.is_empty() {
                            said.push_str(", ");
                            said.push_str(&it.detail);
                        }
                        f.button(row, &said, Hit::Completion(k));
                    }
                }
            }
        }
        // Hover: the type and docs of what the cursor or mouse is on.
        if let Some(h) = self.ws.hover.clone() {
            if h.path == path {
                if let Some((x, y)) = place(h.line, h.col, f) {
                    let max_w = (full.w * 0.6).clamp(260.0, 640.0);
                    let size = f.m.small;
                    // Wrap by words to the box.
                    let mut lines: Vec<String> = Vec::new();
                    for para in h.text.lines() {
                        if para.trim().is_empty() {
                            if lines.last().is_some_and(|l| !l.is_empty()) {
                                lines.push(String::new());
                            }
                            continue;
                        }
                        let mut cur_l = String::new();
                        for word in para.split(' ') {
                            let mut probe = cur_l.clone();
                            if !probe.is_empty() {
                                probe.push(' ');
                            }
                            probe.push_str(word);
                            if f.width(&probe, size, 400) > max_w - 24.0 && !cur_l.is_empty() {
                                lines.push(core::mem::take(&mut cur_l));
                                cur_l = String::from(word);
                            } else {
                                cur_l = probe;
                            }
                        }
                        lines.push(cur_l);
                        if lines.len() > 16 {
                            break;
                        }
                    }
                    lines.truncate(16);
                    while lines.last().is_some_and(|l| l.is_empty()) {
                        lines.pop();
                    }
                    let lhh = size * 1.5;
                    let w = lines
                        .iter()
                        .map(|l| f.width(l, size, 400))
                        .fold(0.0f32, f32::max)
                        + 24.0;
                    let hh = lines.len() as f32 * lhh + 16.0;
                    let above = y - hh - 4.0;
                    let r = Rect {
                        x: x.min(full.right() - w - 8.0).max(8.0),
                        y: if above > full.y + 4.0 {
                            above
                        } else {
                            y + f.m.lh + 4.0
                        },
                        w,
                        h: hh,
                    };
                    self.card(f, t, r);
                    for (k, l) in lines.iter().enumerate() {
                        // The first block is the signature: code colours.
                        let c = if k == 0 { t.ty } else { t.ink };
                        f.text(
                            l,
                            r.x + 12.0,
                            r.y + 8.0 + k as f32 * lhh,
                            size,
                            c,
                            if k == 0 { 600 } else { 400 },
                        );
                    }
                    f.said.text(r, &h.text);
                }
            }
        }
    }

    fn card(&self, f: &mut Frame, t: &Theme, r: Rect) {
        f.round(
            Rect {
                x: r.x + 1.0,
                y: r.y + 3.0,
                w: r.w,
                h: r.h,
            },
            8.0,
            Rgba {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.25,
            },
        );
        f.round(r, 8.0, t.raised);
        f.outline(r, 8.0, 1.0, t.line);
        f.hit(r, Hit::Card);
    }

    /// The squiggle under a problem's place on a drawn line.
    pub(super) fn squiggle(f: &Frame, x0: f32, x1: f32, y: f32, c: Rgba) {
        let mut x = x0;
        let mut up = false;
        while x < x1 {
            let _ = canvas2d::fill_rect(
                f.c,
                grect(Rect {
                    x,
                    y: if up { y } else { y + 1.5 },
                    w: 2.0,
                    h: 1.5,
                }),
                col(c),
            );
            up = !up;
            x += 2.0;
        }
    }

    pub(super) fn panel_tabs(&self) -> Vec<(PanelTab, &'static str)> {
        let mut v = alloc::vec![
            (PanelTab::Problems, "Problems"),
            (PanelTab::Output, "Output"),
            (PanelTab::Terminal, "Terminal"),
        ];
        if self.ws.dap.is_some() || self.ws.panel_tab == PanelTab::Debug {
            v.push((PanelTab::Debug, "Debug"));
        }
        v
    }
}

fn term_title(t: &Term) -> String {
    let mut s = text::num(t.n);
    s.push_str(": ");
    let title = t.vt.title.trim();
    if title.is_empty() {
        s.push_str("shell");
    } else {
        let short: String = title.chars().take(24).collect();
        s.push_str(&short);
    }
    if t.ended {
        s.push_str(" (ended)");
    }
    s
}
