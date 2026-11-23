//! A base's list and board (kanban) views, as the notes app draws them.
//! The table and the cards are in views.rs.

use super::look::{self, fill, fit, icon, r, round, text, width, Icon};
use super::Ui;
use crate::app::Hit;
use crate::bases::{Base, View};
use alloc::string::String;
use alloc::vec::Vec;
use krate::gfx::canvas2d;
use krate::layout::Rect;
#[allow(unused_imports)]
use krate::prelude::*;

/// A cell's text as a list or a card shows it: dates the locale's way.
fn shown(v: &str) -> String {
    crate::props::format_date(v)
}

impl Ui {
    /// The list view: a bullet per note, its name as a link, then its
    /// other properties after commas; groups under their headings.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_base_list(
        &mut self,
        cols: &[String],
        rows: &[Vec<String>],
        groups: &[(String, Vec<String>)],
        w: &View,
        base: &Base,
        a: Rect,
        scroll: f32,
        focused: bool,
    ) {
        let t = self.t();
        let sz = self.sz;
        let k = sz.scale;
        let fsz = sz.ui;
        let line = 22.0 * k;
        let _ = canvas2d::set_clip(self.c, a.x, a.y, a.w, a.h);
        let mut y = a.y + 9.0 * k - scroll;
        let mut ri = 0usize;
        for (label, list) in groups {
            if w.group_by.is_some() {
                // The group's heading: the property, then the value.
                let gname = w
                    .group_by
                    .as_ref()
                    .map(|(p, _)| base.column_name(p))
                    .unwrap_or_default();
                let value = if label.is_empty() {
                    String::from("(none)")
                } else {
                    shown(label)
                };
                y += 8.0 * k;
                let nw = width(self.c, &gname, sz.small, 400);
                text(
                    self.c,
                    &gname,
                    a.x + 24.0 * k,
                    y + line * 0.5 + fsz * 0.36,
                    sz.small,
                    400,
                    t.muted,
                );
                text(
                    self.c,
                    &value,
                    a.x + 30.0 * k + nw,
                    y + line * 0.5 + fsz * 0.36,
                    fsz,
                    600,
                    t.text,
                );
                self.said.text(r(a.x, y, a.w, line), &value);
                y += line + 4.0 * k;
            }
            for _ in list {
                let Some(rv) = rows.get(ri) else {
                    break;
                };
                if y > a.bottom() {
                    break;
                }
                let base_y = y + line * 0.5 + fsz * 0.36;
                let _ = canvas2d::fill_circle(
                    self.c,
                    krate::gfx::types::Point {
                        x: a.x + 31.5 * k,
                        y: y + line * 0.5,
                    },
                    2.5 * k,
                    t.faint,
                );
                let mut x = a.x + 38.0 * k;
                for (i, v) in rv.iter().enumerate() {
                    let is_name = cols.get(i).is_some_and(|c| c == "file.name");
                    if i > 0 {
                        if v.is_empty() {
                            continue;
                        }
                        x += text_w(self, ", ", fsz);
                        text(
                            self.c,
                            ",",
                            x - text_w(self, ", ", fsz),
                            base_y,
                            fsz,
                            400,
                            t.text,
                        );
                    }
                    if is_name {
                        let lw = fit(
                            self.c,
                            v,
                            x,
                            base_y,
                            fsz,
                            400,
                            t.link,
                            a.right() - x - 8.0 * k,
                        );
                        fill(self.c, r(x, base_y + 2.0, lw, 1.0), t.link);
                        if focused {
                            self.button(r(x, y, lw, line), v, Hit::BaseRow(ri));
                        }
                        x += lw;
                    } else {
                        let s = shown(v);
                        let lw = fit(
                            self.c,
                            &s,
                            x,
                            base_y,
                            fsz,
                            400,
                            t.text,
                            a.right() - x - 8.0 * k,
                        );
                        x += lw;
                    }
                }
                self.said.text(r(a.x, y, a.w, line), &rv.join(", "));
                y += line;
                ri += 1;
            }
        }
        let _ = canvas2d::clear_clip(self.c);
    }

    /// The board (kanban) view: a column per group of the view's group-by
    /// property, a card per note with its name and properties.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_base_kanban(
        &mut self,
        cols: &[String],
        rows: &[Vec<String>],
        groups: &[(String, Vec<String>)],
        w: &View,
        base: &Base,
        a: Rect,
        scroll: f32,
        focused: bool,
    ) {
        let t = self.t();
        let sz = self.sz;
        let k = sz.scale;
        if w.group_by.is_none() {
            // As the notes app asks for one.
            let lines = ["Add columns", "Choose a property to group by."];
            let mut y = a.y + a.h * 0.4;
            for (i, l) in lines.iter().enumerate() {
                let size = if i == 0 { sz.medium } else { sz.ui };
                let weight = if i == 0 { 600 } else { 400 };
                let lw = width(self.c, l, size, weight);
                text(
                    self.c,
                    l,
                    a.x + (a.w - lw) * 0.5,
                    y,
                    size,
                    weight,
                    if i == 0 { t.text } else { t.muted },
                );
                self.said.text(r(a.x, y - size, a.w, size * 1.4), l);
                y += size * 1.8;
            }
            return;
        }
        let col_w = 279.0 * k;
        let gap = 9.0 * k;
        let top = a.y + 13.0 * k;
        let bottom = a.bottom() - 12.0 * k;
        let _ = canvas2d::set_clip(self.c, a.x, a.y, a.w, a.h);
        let mut ri = 0usize;
        for (gi, (label, list)) in groups.iter().enumerate() {
            let cx = a.x + 12.0 * k + gi as f32 * (col_w + gap);
            let col = r(cx, top, col_w, bottom - top);
            round(self.c, col, 8.0 * k, t.code_bg);
            look::outline(self.c, col, 8.0 * k, 1.0, t.border);
            // The column's head: the value in bold and how many.
            let name = if label.is_empty() {
                String::from("None")
            } else {
                shown(label)
            };
            let hy = top + 24.0 * k + sz.ui * 0.36;
            let nw = width(self.c, &name, sz.ui, 600);
            text(self.c, &name, cx + 13.0 * k, hy, sz.ui, 600, t.text);
            let n = alloc::format!("{}", list.len());
            text(self.c, &n, cx + 21.0 * k + nw, hy, sz.ui, 400, t.muted);
            self.said.heading(r(cx, top, col_w, 47.0 * k), &name);
            let _ = canvas2d::set_clip(
                self.c,
                cx,
                top + 47.0 * k,
                col_w,
                col.h - 47.0 * k - 40.0 * k,
            );
            let mut y = top + 48.0 * k - scroll;
            for _ in list {
                let Some(rv) = rows.get(ri) else {
                    break;
                };
                let props: Vec<(String, String)> = rv
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| cols.get(*i).is_some_and(|c| c != "file.name"))
                    .map(|(i, v)| (base.column_name(&cols[i]), shown(v)))
                    .collect();
                let title = rv
                    .iter()
                    .enumerate()
                    .find(|(i, _)| cols.get(*i).is_some_and(|c| c == "file.name"))
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default();
                let ch = 42.0 * k + props.len() as f32 * 48.0 * k;
                let card = r(cx + 8.0 * k, y, col_w - 16.0 * k, ch);
                round(self.c, card, 6.0 * k, t.bg);
                look::outline(self.c, card, 6.0 * k, 1.0, t.border);
                fit(
                    self.c,
                    &title,
                    card.x + 9.0 * k,
                    y + 19.5 * k + sz.medium * 0.36,
                    sz.medium,
                    600,
                    t.text,
                    card.w - 18.0 * k,
                );
                let mut py = y + 47.5 * k;
                for (pl, pv) in &props {
                    fit(
                        self.c,
                        pl,
                        card.x + 9.0 * k,
                        py + sz.ui * 0.36,
                        sz.ui,
                        400,
                        t.muted,
                        card.w - 18.0 * k,
                    );
                    fit(
                        self.c,
                        pv,
                        card.x + 9.0 * k,
                        py + 18.0 * k + sz.medium * 0.36,
                        sz.medium,
                        400,
                        t.text,
                        card.w - 18.0 * k,
                    );
                    py += 48.0 * k;
                }
                if focused {
                    self.button(card, &title, Hit::BaseRow(ri));
                }
                y += ch + 8.0 * k;
                ri += 1;
            }
            let _ = canvas2d::set_clip(self.c, a.x, a.y, a.w, a.h);
            // "+ New" at the column's foot.
            let ny = col.bottom() - 20.5 * k;
            icon(self.c, Icon::Plus, cx + 21.0 * k, ny, k, t.muted);
            text(
                self.c,
                "New",
                cx + 33.0 * k,
                ny + sz.medium * 0.36,
                sz.medium,
                400,
                t.muted,
            );
            let nb = r(cx + 8.0 * k, ny - 14.0 * k, 80.0 * k, 28.0 * k);
            if focused {
                self.button(nb, "New", Hit::BaseNew(gi));
            } else {
                self.said.text(nb, "New");
            }
        }
        let _ = canvas2d::clear_clip(self.c);
    }
}

fn text_w(me: &Ui, s: &str, size: f32) -> f32 {
    width(me.c, s, size, 400)
}
