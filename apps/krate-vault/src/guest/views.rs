//! The views that are not notes, drawn and driven: the graph, a canvas, a
//! base's table, a note's saved copies, and the settings window.

use super::look::{self, fill, fit, icon, r, round, text, width, Icon, Rgba};
use super::{Drag, Ui, Zone};
use crate::app::{Ask, AskFor, Cmd, Hit, Input, ListKind, Mods, Overlay, SettingsPage, View};
use crate::canvas::{self, CardKind, Side};
use crate::graph::{Kind, Raster};
use crate::workspace::Control;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use krate::gfx::{canvas2d, canvas_images, types as gfx};
use krate::layout::Rect;
#[allow(unused_imports)]
use krate::prelude::*;

/// What a press on a canvas started.
#[derive(Clone, PartialEq, Debug)]
pub enum CanvasDrag {
    Pan,
    Move(String),
    Resize(String),
    Connect(String, Side),
}

/// A group of pages in the settings list: its name and (page, label,
/// plugin) for each.
type NavGroup<'a> = (&'a str, &'a [(SettingsPage, &'a str, &'a str)]);

/// A control in the graph's settings panel.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Ctl {
    Section(u8),
    Toggle(u8),
    Slider(u8),
    Search,
    NewGroup,
    RemoveGroup(usize),
    Reset,
    Close,
    Open,
}

/// The graph's picture, kept by the host while nothing changes, and the
/// boxes of its controls; also what canvas file cards show.
#[derive(Default)]
pub struct GraphImage {
    key: u64,
    image: Option<u64>,
    controls: Vec<(Rect, Ctl)>,
    /// The slider tracks, by slider number: (x, width).
    sliders: Vec<(u8, f32, f32)>,
    /// Note text shown in canvas file cards, by path, with the vault
    /// generation it was read at.
    notes: BTreeMap<String, (u64, Vec<String>)>,
    last_click: (u64, f32, f32),
}

fn fnv(h: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *h ^= u64::from(*b);
        *h = h.wrapping_mul(0x0100_0000_01b3);
    }
}

fn col(c: u32) -> Rgba {
    look::hex(c)
}

/// The sliders: (name, min, max).
const SLIDERS: [(&str, f32, f32); 7] = [
    ("Text fade threshold", -3.0, 3.0),
    ("Node size", 0.1, 5.0),
    ("Link thickness", 0.1, 5.0),
    ("Center force", 0.0, 1.0),
    ("Repel force", 0.0, 20.0),
    ("Link force", 0.0, 1.0),
    ("Link distance", 30.0, 500.0),
];

fn slider_get(o: &crate::graph::Options, i: u8) -> f32 {
    match i {
        0 => o.text_fade,
        1 => o.node_size,
        2 => o.line_size,
        3 => o.center,
        4 => o.repel,
        5 => o.link,
        _ => o.distance,
    }
}

fn slider_set(o: &mut crate::graph::Options, i: u8, v: f32) {
    match i {
        0 => o.text_fade = v,
        1 => o.node_size = v,
        2 => o.line_size = v,
        3 => o.center = v,
        4 => o.repel = v,
        5 => o.link = v,
        _ => o.distance = v,
    }
}

impl Ui {
    pub(super) fn views_busy(&self) -> bool {
        matches!(self.app.view(), Some(View::Graph(g)) if g.graph.as_ref().is_none_or(|x| x.running()) || g.stale)
    }

    // ---- the graph ---------------------------------------------------------------

    pub(super) fn draw_graph(&mut self, pane: usize, a: Rect, focused: bool) {
        let t = self.t();
        let sz = self.sz;
        let k = sz.scale;
        self.view_areas.push((pane, a));
        if focused {
            self.hit(a, Zone::Graph(pane));
        }
        let (tabs, act) = self.app.pane_tabs(pane);
        let Some(View::Graph(g)) = tabs.get(act).and_then(|tb| tb.view.as_ref()) else {
            return;
        };
        let Some(gr) = g.graph.as_ref() else {
            text(
                self.c,
                "Laying out the graph\u{2026}",
                a.x + 24.0,
                a.y + 30.0,
                sz.ui,
                400,
                t.faint,
            );
            return;
        };
        let (cx, cy, zoom) = (g.cam.0, g.cam.1, g.zoom);
        let opt = g.opt.clone();
        let hover = g.hover;
        // The picture: lines and dots, drawn here.
        let dense = a.w * a.h < 700_000.0;
        let px = if dense { 2.0 } else { 1.0 };
        let (pw, ph) = ((a.w * px).max(1.0) as u32, (a.h * px).max(1.0) as u32);
        let mut key: u64 = 0xcbf2_9ce4_8422_2325;
        for v in [cx, cy, zoom, a.w, a.h, opt.node_size, opt.line_size] {
            fnv(&mut key, &v.to_bits().to_le_bytes());
        }
        fnv(&mut key, &(gr.nodes.len() as u64).to_le_bytes());
        fnv(&mut key, &gr.alpha.to_bits().to_le_bytes());
        fnv(&mut key, &[u8::from(t.dark), u8::from(opt.show_arrow)]);
        fnv(
            &mut key,
            &(hover.map_or(u64::MAX, |h| h as u64)).to_le_bytes(),
        );
        if let Some(n) = gr.nodes.first() {
            fnv(&mut key, &n.x.to_bits().to_le_bytes());
        }
        let to_screen = |x: f32, y: f32| ((x - cx) * zoom + a.w * 0.5, (y - cy) * zoom + a.h * 0.5);
        if key != self.graph_img.key || self.graph_img.image.is_none() {
            let mut ras = Raster::new(pw, ph);
            ras.clear(look::rgb(t.bg));
            let line = look::rgb(t.graph_line);
            let accent = look::rgb(t.link);
            let near: Vec<bool> = match hover {
                Some(hv) => {
                    let mut v = alloc::vec![false; gr.nodes.len()];
                    if let Some(x) = v.get_mut(hv) {
                        *x = true;
                    }
                    for &n in gr.adj.get(hv).map_or(&[][..], |x| x.as_slice()) {
                        if let Some(x) = v.get_mut(n as usize) {
                            *x = true;
                        }
                    }
                    v
                }
                None => Vec::new(),
            };
            let lw = opt.line_size.clamp(0.1, 5.0);
            for &(e0, e1) in &gr.edges {
                let (Some(na), Some(nb)) = (gr.nodes.get(e0 as usize), gr.nodes.get(e1 as usize))
                else {
                    continue;
                };
                let (x0, y0) = to_screen(na.x, na.y);
                let (x1, y1) = to_screen(nb.x, nb.y);
                let lit = hover.is_some_and(|h| h == e0 as usize || h == e1 as usize);
                let (c, al) = if lit {
                    (accent, 1.0)
                } else if hover.is_some() {
                    (line, 0.35)
                } else {
                    (line, 1.0)
                };
                let al = (al * lw.min(1.0)).min(1.0);
                ras.line(x0 * px, y0 * px, x1 * px, y1 * px, c, al);
                if lw > 1.5 || px > 1.0 && lw > 0.8 {
                    let off = 0.6 * px;
                    ras.line(
                        x0 * px + off,
                        y0 * px + off,
                        x1 * px + off,
                        y1 * px + off,
                        c,
                        al * 0.6,
                    );
                }
                if opt.show_arrow {
                    let dx = x1 - x0;
                    let dy = y1 - y0;
                    let l = (dx * dx + dy * dy).sqrt().max(0.01);
                    let rad = nb.radius(opt.node_size) * zoom;
                    let tip = (x1 - dx / l * rad, y1 - dy / l * rad);
                    for s in [-1.0f32, 1.0] {
                        let ax = tip.0 - dx / l * 6.0 - dy / l * 3.5 * s;
                        let ay = tip.1 - dy / l * 6.0 + dx / l * 3.5 * s;
                        ras.line(tip.0 * px, tip.1 * px, ax * px, ay * px, c, al);
                    }
                }
            }
            for (i, n) in gr.nodes.iter().enumerate() {
                let (x, y) = to_screen(n.x, n.y);
                let rad = (n.radius(opt.node_size) * zoom).max(1.2);
                let base = match (n.kind, n.color) {
                    (_, Some(c)) => c,
                    (Kind::Unresolved, _) => look::rgb(t.faint),
                    (Kind::Tag, _) => 0x44CF6E,
                    _ => look::rgb(t.muted),
                };
                let c = if hover == Some(i) { accent } else { base };
                let al = if hover.is_some() && !near.get(i).copied().unwrap_or(false) {
                    0.3
                } else {
                    1.0
                };
                ras.disc(x * px, y * px, rad * px, c, al);
            }
            if let Some(old) = self.graph_img.image.take() {
                canvas_images::forget(self.c, old);
            }
            self.graph_img.image = canvas_images::keep(self.c, pw, ph, &ras.px).ok();
            self.graph_img.key = key;
        }
        if let Some(img) = self.graph_img.image {
            let _ = canvas_images::draw(
                self.c,
                img,
                canvas_images::Rect {
                    x: a.x,
                    y: a.y,
                    width: a.w,
                    height: a.h,
                },
            );
        }
        // Names under the dots, when there is room to read them.
        let fade = 2.0f32.powf(-opt.text_fade);
        let show_all = zoom >= 0.4 * fade;
        let _ = canvas2d::set_clip(self.c, a.x, a.y, a.w, a.h);
        let mut shown = 0;
        let size = (12.0 * zoom.clamp(0.75, 1.4)) * k;
        for (i, n) in gr.nodes.iter().enumerate() {
            let near_hover = hover
                .is_some_and(|h| h == i || gr.adj.get(h).is_some_and(|x| x.contains(&(i as u32))));
            if !show_all && !near_hover {
                continue;
            }
            let (x, y) = to_screen(n.x, n.y);
            if x < -100.0 || y < -20.0 || x > a.w + 100.0 || y > a.h + 40.0 {
                continue;
            }
            if shown > 500 && !near_hover {
                continue;
            }
            shown += 1;
            let lw = width(self.c, &n.label, size, 400);
            let rad = n.radius(opt.node_size) * zoom;
            let colr = if hover.is_some() && !near_hover {
                look::alpha(t.text, 0.3)
            } else {
                t.text
            };
            text(
                self.c,
                &n.label,
                a.x + x - lw * 0.5,
                a.y + y + rad + size + 2.0,
                size,
                400,
                colr,
            );
        }
        let _ = canvas2d::clear_clip(self.c);
        if focused {
            for n in gr.nodes.iter().take(200) {
                let (x, y) = to_screen(n.x, n.y);
                if x >= 0.0 && y >= 0.0 && x <= a.w && y <= a.h {
                    self.said
                        .button(r(a.x + x - 6.0, a.y + y - 6.0, 12.0, 12.0), &n.label);
                }
            }
        }
        if g.local.is_none() && focused {
            self.draw_graph_panel(a);
        }
    }

    fn draw_graph_panel(&mut self, a: Rect) {
        let t = self.t();
        let sz = self.sz;
        let k = sz.scale;
        self.graph_img.controls.clear();
        self.graph_img.sliders.clear();
        let (opt, open) = match self.app.view() {
            Some(View::Graph(g)) => (g.opt.clone(), g.controls),
            _ => return,
        };
        let pw = 240.0 * k;
        let x = a.right() - pw - 12.0 * k;
        let y0 = a.y + 12.0 * k;
        if !open {
            let b = r(a.right() - 44.0 * k, y0, 32.0 * k, 32.0 * k);
            round(self.c, b, 6.0, t.side);
            icon(
                self.c,
                Icon::Gear,
                b.x + b.w * 0.5,
                b.y + b.h * 0.5,
                k,
                t.muted,
            );
            self.graph_img.controls.push((b, Ctl::Open));
            self.hit(b, Zone::GraphPanel);
            self.said.button(b, "Open graph settings");
            return;
        }
        // Each section's rows, to size the panel.
        let sec_h = 40.0 * k;
        let row_h = 32.0 * k;
        let rows_of = |s: u8| -> usize {
            match s {
                0 => 5,
                1 => opt.groups.len() + 1,
                2 => 4,
                _ => 4,
            }
        };
        let opens = [
            opt.open_filters,
            opt.open_groups,
            opt.open_display,
            opt.open_forces,
        ];
        let mut h = 0.0;
        for s in 0..4u8 {
            h += sec_h;
            if opens[s as usize] {
                h += rows_of(s) as f32 * row_h + 8.0 * k;
            }
        }
        let panel = r(x, y0, pw, h);
        let _ = canvas2d::drop_shadow_round_rect(
            self.c,
            look::g(panel),
            look::radii(8.0),
            12.0,
            look::alpha(look::hex(0), 0.25),
        );
        round(self.c, panel, 8.0, t.side);
        look::outline(self.c, panel, 8.0, 1.0, t.border);
        self.hit(panel, Zone::GraphPanel);
        let titles = ["Filters", "Groups", "Display", "Forces"];
        let mut y = y0;
        for s in 0..4u8 {
            if s > 0 {
                fill(self.c, r(x, y, pw, 1.0), t.border);
            }
            let hr = r(x, y, pw, sec_h);
            look::chevron(
                self.c,
                x + 16.0 * k,
                y + sec_h * 0.5,
                opens[s as usize],
                k,
                t.muted,
            );
            text(
                self.c,
                titles[s as usize],
                x + 28.0 * k,
                y + sec_h * 0.5 + sz.medium * 0.36,
                sz.medium,
                600,
                t.text,
            );
            self.graph_img
                .controls
                .push((r(hr.x, hr.y, hr.w - 70.0 * k, hr.h), Ctl::Section(s)));
            self.said.button(hr, titles[s as usize]);
            if s == 0 {
                let rb = r(x + pw - 64.0 * k, y + 8.0 * k, 24.0 * k, 24.0 * k);
                icon(
                    self.c,
                    Icon::Refresh,
                    rb.x + 12.0 * k,
                    rb.y + 12.0 * k,
                    k * 0.9,
                    t.muted,
                );
                self.graph_img.controls.push((rb, Ctl::Reset));
                self.said.button(rb, "Restore default settings");
                let cb = r(x + pw - 34.0 * k, y + 8.0 * k, 24.0 * k, 24.0 * k);
                icon(
                    self.c,
                    Icon::Close,
                    cb.x + 12.0 * k,
                    cb.y + 12.0 * k,
                    k * 0.9,
                    t.muted,
                );
                self.graph_img.controls.push((cb, Ctl::Close));
                self.said.button(cb, "Close");
            }
            y += sec_h;
            if !opens[s as usize] {
                continue;
            }
            match s {
                0 => {
                    let fb = r(x + 12.0 * k, y + 2.0 * k, pw - 24.0 * k, 28.0 * k);
                    round(self.c, fb, 5.0, t.field);
                    icon(
                        self.c,
                        Icon::Search,
                        fb.x + 13.0 * k,
                        fb.y + fb.h * 0.5,
                        k * 0.8,
                        t.faint,
                    );
                    let (s_txt, c) = if opt.search.is_empty() {
                        ("Search files...", t.faint)
                    } else {
                        (opt.search.as_str(), t.text)
                    };
                    fit(
                        self.c,
                        s_txt,
                        fb.x + 26.0 * k,
                        fb.y + fb.h * 0.5 + sz.ui * 0.36,
                        sz.ui,
                        400,
                        c,
                        fb.w - 32.0 * k,
                    );
                    self.graph_img.controls.push((fb, Ctl::Search));
                    self.said.field(fb, "Search files", &opt.search);
                    y += row_h;
                    let toggles = [
                        ("Tags", opt.show_tags),
                        ("Attachments", opt.show_attachments),
                        ("Existing files only", opt.hide_unresolved),
                        ("Orphans", opt.show_orphans),
                    ];
                    for (i, (name, on)) in toggles.iter().enumerate() {
                        let rr = r(x, y, pw, row_h);
                        text(
                            self.c,
                            name,
                            x + 16.0 * k,
                            y + row_h * 0.5 + sz.ui * 0.36,
                            sz.ui,
                            400,
                            t.text,
                        );
                        self.toggle(
                            r(
                                x + pw - 52.0 * k,
                                y + (row_h - 20.0 * k) * 0.5,
                                36.0 * k,
                                20.0 * k,
                            ),
                            *on,
                        );
                        self.graph_img.controls.push((rr, Ctl::Toggle(i as u8)));
                        self.said.button(rr, name);
                        y += row_h;
                    }
                }
                1 => {
                    for (i, (q, c)) in opt.groups.iter().enumerate() {
                        let _ = canvas2d::fill_circle(
                            self.c,
                            gfx::Point {
                                x: x + 22.0 * k,
                                y: y + row_h * 0.5,
                            },
                            6.0 * k,
                            col(*c),
                        );
                        fit(
                            self.c,
                            q,
                            x + 36.0 * k,
                            y + row_h * 0.5 + sz.ui * 0.36,
                            sz.ui,
                            400,
                            t.text,
                            pw - 80.0 * k,
                        );
                        let cb = r(x + pw - 34.0 * k, y + 4.0 * k, 24.0 * k, 24.0 * k);
                        icon(
                            self.c,
                            Icon::Close,
                            cb.x + 12.0 * k,
                            cb.y + 12.0 * k,
                            k * 0.8,
                            t.muted,
                        );
                        self.graph_img.controls.push((cb, Ctl::RemoveGroup(i)));
                        self.said.button(cb, "Remove group");
                        y += row_h;
                    }
                    let b = r(x + (pw - 110.0 * k) * 0.5, y + 3.0 * k, 110.0 * k, 26.0 * k);
                    round(self.c, b, 5.0, t.accent);
                    let lw = width(self.c, "New group", sz.ui, 500);
                    text(
                        self.c,
                        "New group",
                        b.x + (b.w - lw) * 0.5,
                        b.y + b.h * 0.5 + sz.ui * 0.36,
                        sz.ui,
                        500,
                        t.on_accent,
                    );
                    self.graph_img.controls.push((b, Ctl::NewGroup));
                    self.said.button(b, "New group");
                    y += row_h;
                }
                _ => {
                    let list: &[u8] = if s == 2 {
                        &[255, 0, 1, 2]
                    } else {
                        &[3, 4, 5, 6]
                    };
                    for &i in list {
                        if i == 255 {
                            let rr = r(x, y, pw, row_h);
                            text(
                                self.c,
                                "Arrows",
                                x + 16.0 * k,
                                y + row_h * 0.5 + sz.ui * 0.36,
                                sz.ui,
                                400,
                                t.text,
                            );
                            self.toggle(
                                r(
                                    x + pw - 52.0 * k,
                                    y + (row_h - 20.0 * k) * 0.5,
                                    36.0 * k,
                                    20.0 * k,
                                ),
                                opt.show_arrow,
                            );
                            self.graph_img.controls.push((rr, Ctl::Toggle(4)));
                            self.said.button(rr, "Arrows");
                            y += row_h;
                            continue;
                        }
                        let (name, lo, hi) = SLIDERS[i as usize];
                        text(
                            self.c,
                            name,
                            x + 16.0 * k,
                            y + 13.0 * k,
                            sz.small,
                            400,
                            t.muted,
                        );
                        let tx = x + 16.0 * k;
                        let tw = pw - 32.0 * k;
                        let ty = y + 22.0 * k;
                        let f = ((slider_get(&opt, i) - lo) / (hi - lo)).clamp(0.0, 1.0);
                        round(self.c, r(tx, ty - 2.0, tw, 4.0), 2.0, t.border_hover);
                        round(self.c, r(tx, ty - 2.0, tw * f, 4.0), 2.0, t.accent);
                        let _ = canvas2d::fill_circle(
                            self.c,
                            gfx::Point {
                                x: tx + tw * f,
                                y: ty,
                            },
                            7.0 * k,
                            look::hex(0xFFFFFF),
                        );
                        let _ = canvas2d::stroke_circle(
                            self.c,
                            gfx::Point {
                                x: tx + tw * f,
                                y: ty,
                            },
                            7.0 * k,
                            1.0,
                            t.border_focus,
                        );
                        let sr = r(tx - 6.0, y + 12.0 * k, tw + 12.0, 20.0 * k);
                        self.graph_img.controls.push((sr, Ctl::Slider(i)));
                        self.graph_img.sliders.push((i, tx, tw));
                        self.hit(sr, Zone::GraphSlider(i));
                        self.said.button(sr, name);
                        y += row_h;
                    }
                }
            }
            y += 8.0 * k;
        }
    }

    /// A switch, as the settings draw them.
    fn toggle(&mut self, b: Rect, on: bool) {
        let t = self.t();
        round(
            self.c,
            b,
            b.h * 0.5,
            if on { t.accent } else { t.border_hover },
        );
        let d = b.h - 4.0;
        let kx = if on { b.right() - d - 2.0 } else { b.x + 2.0 };
        let _ = canvas2d::fill_circle(
            self.c,
            gfx::Point {
                x: kx + d * 0.5,
                y: b.y + b.h * 0.5,
            },
            d * 0.5,
            look::hex(0xFFFFFF),
        );
    }

    /// Fit the graph or canvas in view the first time it is drawn.
    fn fit_views(&mut self) {
        let areas: Vec<(usize, Rect)> = self.view_areas.clone();
        for (p, a) in areas {
            if p != self.app.pane {
                continue;
            }
            match self.app.view_mut() {
                Some(View::Graph(g)) if !g.fitted => {
                    if let Some(gr) = g.graph.as_ref() {
                        if gr.nodes.is_empty() {
                            continue;
                        }
                        // Centred on the middle the layout pulls to, as near
                        // full size as fits, the far edges allowed to run off.
                        let mut xs: Vec<f32> = gr.nodes.iter().map(|n| n.x.abs()).collect();
                        let mut ys: Vec<f32> = gr.nodes.iter().map(|n| n.y.abs()).collect();
                        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
                        ys.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
                        let q =
                            |v: &[f32]| v.get(v.len() * 95 / 100).copied().unwrap_or(1.0).max(1.0);
                        let (hx, hy) = (q(&xs) + 40.0, q(&ys) + 40.0);
                        g.zoom = (a.w * 0.5 / hx).min(a.h * 0.5 / hy).clamp(0.02, 1.0);
                        g.cam = (0.0, 0.0);
                        if !gr.running() {
                            g.fitted = true;
                        }
                    }
                }
                Some(View::Canvas(c)) if !c.fitted => {
                    if let Some((x, y, w, h)) = c.canvas.bounds() {
                        c.zoom = ((a.w - 80.0) / w).min((a.h - 80.0) / h).clamp(0.1, 1.0);
                        c.cam = (x + w * 0.5, y + h * 0.5);
                    }
                    c.fitted = true;
                }
                _ => {}
            }
        }
    }

    /// The pointer over a graph: which dot it is on. True when that changed.
    pub(super) fn graph_hover(&mut self, px: f32, py: f32) -> bool {
        let Some((_, a)) = self
            .view_areas
            .iter()
            .find(|(p, a)| *p == self.app.pane && a.contains(px, py))
            .copied()
        else {
            return false;
        };
        let Some(View::Graph(g)) = self.app.view_mut() else {
            return false;
        };
        let Some(gr) = g.graph.as_ref() else {
            return false;
        };
        let wx = (px - a.x - a.w * 0.5) / g.zoom + g.cam.0;
        let wy = (py - a.y - a.h * 0.5) / g.zoom + g.cam.1;
        let h = gr.hit(wx, wy, 4.0 / g.zoom, g.opt.node_size);
        if h != g.hover {
            g.hover = h;
            return true;
        }
        false
    }

    // ---- a canvas ---------------------------------------------------------------------

    pub(super) fn draw_canvas(&mut self, pane: usize, a: Rect, focused: bool) {
        let t = self.t();
        let sz = self.sz;
        let k = sz.scale;
        self.view_areas.push((pane, a));
        if focused {
            self.hit(a, Zone::Canvas(pane));
        }
        let gen = self.app.vault.as_ref().map_or(0, |v| v.generation);
        // The text of notes shown in file cards, read once per change.
        let files: Vec<String> = {
            let (tabs, act) = self.app.pane_tabs(pane);
            match tabs.get(act).and_then(|tb| tb.view.as_ref()) {
                Some(View::Canvas(c)) => c
                    .canvas
                    .nodes
                    .iter()
                    .filter_map(|n| n.file.clone())
                    .collect(),
                _ => return,
            }
        };
        for f in files {
            let fresh = self.graph_img.notes.get(&f).is_some_and(|(g, _)| *g == gen);
            if !fresh {
                let text = self
                    .app
                    .open_text(&f)
                    .or_else(|| self.app.vault.as_mut().and_then(|v| v.fs.read(&f).ok()))
                    .unwrap_or_default();
                let lines: Vec<String> = text.split('\n').take(60).map(String::from).collect();
                self.graph_img.notes.insert(f, (gen, lines));
            }
        }
        let (tabs, act) = self.app.pane_tabs(pane);
        let Some(View::Canvas(cv)) = tabs.get(act).and_then(|tb| tb.view.as_ref()) else {
            return;
        };
        let (cx, cy, z) = (cv.cam.0, cv.cam.1, cv.zoom);
        let sel = cv.selected.clone();
        let editing = cv.editing.clone();
        let board = cv.canvas.clone();
        let to = |x: f32, y: f32| {
            (
                a.x + a.w * 0.5 + (x - cx) * z,
                a.y + a.h * 0.5 + (y - cy) * z,
            )
        };
        let _ = canvas2d::set_clip(self.c, a.x, a.y, a.w, a.h);
        // The dot grid.
        let step = 20.0 * z;
        if step >= 7.0 {
            let ox = (a.w * 0.5 - cx * z).rem_euclid(step);
            let oy = (a.h * 0.5 - cy * z).rem_euclid(step);
            let mut n = 0;
            let mut y = a.y + oy;
            while y < a.bottom() && n < 4000 {
                let mut x = a.x + ox;
                while x < a.right() {
                    fill(self.c, r(x - 0.75, y - 0.75, 1.5, 1.5), t.border);
                    x += step;
                    n += 1;
                }
                y += step;
            }
        }
        let fs = 16.0 * z * k;
        // Groups under everything, then arrows, then cards.
        for pass in 0..2 {
            for n in &board.nodes {
                if (n.kind == CardKind::Group) != (pass == 0) {
                    continue;
                }
                let (x0, y0) = to(n.x, n.y);
                let (w, h) = (n.w * z, n.h * z);
                if x0 > a.right() || y0 > a.bottom() || x0 + w < a.x || y0 + h < a.y - 40.0 {
                    continue;
                }
                let rc = r(x0, y0, w, h);
                let tint = n.color.as_deref().and_then(|c| canvas::color_of(c, t.dark));
                let border = tint.map_or(
                    if n.kind == CardKind::Group {
                        t.border_hover
                    } else {
                        t.canvas_edge
                    },
                    col,
                );
                let is_sel = sel.as_deref() == Some(n.id.as_str());
                if n.kind == CardKind::Group {
                    if let Some(c) = tint {
                        round(self.c, rc, 8.0 * z, look::alpha(col(c), 0.07));
                    } else {
                        round(self.c, rc, 8.0 * z, look::alpha(t.text, 0.03));
                    }
                    look::outline(
                        self.c,
                        rc,
                        8.0 * z,
                        (2.0 * z).max(1.0),
                        if is_sel { t.accent } else { border },
                    );
                    if let Some(label) = &n.label {
                        let ls = 24.0 * z * k;
                        let lw = width(self.c, label, ls, 400);
                        round(
                            self.c,
                            r(x0, y0 - ls * 1.6, lw + 12.0 * z, ls * 1.45),
                            6.0 * z,
                            look::alpha(t.text, 0.05),
                        );
                        text(
                            self.c,
                            label,
                            x0 + 6.0 * z,
                            y0 - ls * 0.45,
                            ls,
                            400,
                            t.muted,
                        );
                    }
                    continue;
                }
                round(self.c, rc, 8.0 * z, t.bg);
                if let Some(c) = tint {
                    round(self.c, rc, 8.0 * z, look::alpha(col(c), 0.1));
                }
                look::outline(
                    self.c,
                    rc,
                    8.0 * z,
                    if is_sel { (2.0 * z).max(1.5) } else { 1.0 },
                    if is_sel { t.accent } else { border },
                );
                // What the card shows.
                let pad = 24.0 * z;
                let lines: Vec<String> = match n.kind {
                    CardKind::Text => {
                        let body = match &editing {
                            Some((id, inp)) if *id == n.id => inp.text.clone(),
                            _ => n.text.clone().unwrap_or_default(),
                        };
                        body.split('\n').map(String::from).collect()
                    }
                    CardKind::File => {
                        let f = n.file.clone().unwrap_or_default();
                        let ls = 14.0 * z * k;
                        let name = String::from(crate::app::strip_ext(crate::vfs::file_name(&f)));
                        text(self.c, &name, x0, y0 - ls * 0.6, ls, 400, t.faint);
                        let mut v = alloc::vec![{
                            let mut s = String::from("# ");
                            s.push_str(&name);
                            s
                        }];
                        v.extend(
                            self.graph_img
                                .notes
                                .get(&f)
                                .map(|(_, l)| l.clone())
                                .unwrap_or_default(),
                        );
                        v
                    }
                    CardKind::Link => {
                        let u = n.url.clone().unwrap_or_default();
                        let ls = 14.0 * z * k;
                        fit(self.c, &u, x0, y0 - ls * 0.6, ls, 400, t.faint, w);
                        alloc::vec![String::from("Web page"), u]
                    }
                    CardKind::Group => Vec::new(),
                };
                let _ = canvas2d::set_clip(
                    self.c,
                    rc.x.max(a.x),
                    rc.y.max(a.y),
                    (rc.right().min(a.right()) - rc.x.max(a.x)).max(0.0),
                    (rc.bottom().min(a.bottom()) - rc.y.max(a.y)).max(0.0),
                );
                let mut ty = y0 + pad * 0.75;
                for line in &lines {
                    if ty > rc.bottom() {
                        break;
                    }
                    let (size, weight, body) = if let Some(hd) = line.strip_prefix("# ") {
                        (fs * 1.6, 700, String::from(hd))
                    } else if let Some(hd) = line.strip_prefix("## ") {
                        (fs * 1.4, 650, String::from(hd))
                    } else if line.starts_with("---") || line.trim().is_empty() {
                        ty += fs * 0.6;
                        continue;
                    } else {
                        (fs, 400, crate::md::plain(line, 0))
                    };
                    for wl in self.wrap(&body, size, w - pad * 2.0).into_iter().take(40) {
                        ty += size * 1.25;
                        if ty > rc.bottom() + size {
                            break;
                        }
                        text(self.c, &wl, x0 + pad, ty, size, weight, t.text);
                    }
                    ty += size * 0.45;
                }
                if editing.as_ref().is_some_and(|(id, _)| *id == n.id) {
                    // The caret at the end of the text being typed.
                    let last = lines.last().cloned().unwrap_or_default();
                    let lw = width(self.c, &crate::md::plain(&last, 0), fs, 400);
                    fill(
                        self.c,
                        r(
                            x0 + pad + lw.min(w - pad * 2.0),
                            ty - fs * 0.95,
                            1.5,
                            fs * 1.2,
                        ),
                        t.text,
                    );
                }
                let _ = canvas2d::set_clip(self.c, a.x, a.y, a.w, a.h);
                if is_sel {
                    // The resize corner and the four places arrows start.
                    let hs = 8.0;
                    round(
                        self.c,
                        r(x0 + w - hs, y0 + h - hs, hs * 2.0, hs * 2.0),
                        3.0,
                        look::alpha(t.accent, 0.0),
                    );
                    for side in [Side::Top, Side::Right, Side::Bottom, Side::Left] {
                        let (sx, sy) = n.anchor(side);
                        let (sx, sy) = to(sx, sy);
                        let _ =
                            canvas2d::fill_circle(self.c, gfx::Point { x: sx, y: sy }, 5.0, t.bg);
                        let _ = canvas2d::stroke_circle(
                            self.c,
                            gfx::Point { x: sx, y: sy },
                            5.0,
                            1.5,
                            t.accent,
                        );
                    }
                }
                self.said
                    .button(rc, lines.first().map_or("Card", String::as_str));
            }
            if pass == 0 {
                // The arrows.
                for e in &board.edges {
                    let Some((p0, p1)) = board.ends(e) else {
                        continue;
                    };
                    let pts = canvas::curve(p0, p1, 16);
                    let ec = e
                        .color
                        .as_deref()
                        .and_then(|c| canvas::color_of(c, t.dark))
                        .map_or(t.canvas_edge, col);
                    let lw = (2.0 * z).max(1.0);
                    let sp: Vec<(f32, f32)> = pts.iter().map(|&(x, y)| to(x, y)).collect();
                    for pair in sp.windows(2) {
                        look::line(self.c, pair[0].0, pair[0].1, pair[1].0, pair[1].1, lw, ec);
                    }
                    let head = |tip: (f32, f32), from: (f32, f32)| {
                        let dx = tip.0 - from.0;
                        let dy = tip.1 - from.1;
                        let l = (dx * dx + dy * dy).sqrt().max(0.01);
                        let (ux, uy) = (dx / l, dy / l);
                        let s = 10.0 * z.max(0.5);
                        for side in [-1.0f32, 1.0] {
                            look::stroke(
                                self.c,
                                tip.0,
                                tip.1,
                                tip.0 - ux * s - uy * s * 0.55 * side,
                                tip.1 - uy * s + ux * s * 0.55 * side,
                                lw * 1.2,
                                ec,
                            );
                        }
                    };
                    if e.to_arrow {
                        if let (Some(&tip), Some(&from)) =
                            (sp.last(), sp.get(sp.len().saturating_sub(3)))
                        {
                            head(tip, from);
                        }
                    }
                    if e.from_arrow {
                        if let (Some(&tip), Some(&from)) = (sp.first(), sp.get(2)) {
                            head(tip, from);
                        }
                    }
                    if let Some(label) = &e.label {
                        let m = sp[sp.len() / 2];
                        let ls = fs * 1.1;
                        let lw = width(self.c, label, ls, 400);
                        round(
                            self.c,
                            r(m.0 - lw * 0.5 - 6.0, m.1 - ls * 0.8, lw + 12.0, ls * 1.5),
                            4.0,
                            t.bg,
                        );
                        text(
                            self.c,
                            label,
                            m.0 - lw * 0.5,
                            m.1 + ls * 0.35,
                            ls,
                            400,
                            t.text,
                        );
                    }
                }
            }
        }
        // An arrow being drawn.
        if let Drag::Canvas {
            what: CanvasDrag::Connect(ref id, side),
            ..
        } = self.drag
        {
            if let (Some(n), Some((px, py))) =
                (board.card(id), krate::ui::pointer::position(self.win))
            {
                let (sx, sy) = n.anchor(side);
                let (sx, sy) = to(sx, sy);
                look::line(self.c, sx, sy, px, py, 2.0, t.accent);
            }
        }
        let _ = canvas2d::clear_clip(self.c);
        if !focused {
            return;
        }
        // The controls down the right side and the toolbar at the bottom.
        let bx = a.right() - 36.0 * k;
        let mut by = a.y + 12.0 * k;
        let controls: [(Icon, Cmd, &str); 6] = [
            (Icon::Plus, Cmd::Back, "Zoom in"),
            (Icon::Refresh, Cmd::Back, "Reset zoom"),
            (Icon::Maximize, Cmd::Back, "Zoom to fit"),
            (Icon::Minus, Cmd::Back, "Zoom out"),
            (Icon::Undo, Cmd::Undo, "Undo"),
            (Icon::Redo, Cmd::Redo, "Redo"),
        ];
        let group = r(bx - 2.0, by - 2.0, 32.0 * k, 32.0 * k * 4.0 + 4.0);
        round(self.c, group, 6.0, t.side);
        look::outline(self.c, group, 6.0, 1.0, t.border);
        for (i, (ic, _, label)) in controls.iter().enumerate() {
            if i == 4 {
                by += 12.0 * k;
                let g2 = r(bx - 2.0, by - 2.0, 32.0 * k, 32.0 * k * 2.0 + 4.0);
                round(self.c, g2, 6.0, t.side);
                look::outline(self.c, g2, 6.0, 1.0, t.border);
            }
            let b = r(bx, by, 28.0 * k, 28.0 * k);
            let z = Zone::App(Hit::Command(match i {
                4 => Cmd::Undo,
                5 => Cmd::Redo,
                _ => Cmd::Help,
            }));
            if self.hovered(&z) && i >= 4 {
                round(self.c, b, 4.0, t.hover);
            }
            icon(self.c, *ic, b.x + b.w * 0.5, b.y + b.h * 0.5, k, t.muted);
            self.graph_img
                .controls
                .push((b, Ctl::Section(100 + i as u8)));
            self.hit(b, Zone::Canvas(pane));
            self.said.button(b, label);
            by += 32.0 * k;
        }
        let tw = 3.0 * 48.0 * k + 8.0 * k;
        let tb = r(a.x + (a.w - tw) * 0.5, a.bottom() - 60.0 * k, tw, 48.0 * k);
        round(self.c, tb, 8.0, t.side);
        look::outline(self.c, tb, 8.0, 1.0, t.border);
        for (i, (ic, label)) in [
            (Icon::File, "Drag to add card"),
            (Icon::FileText, "Drag to add note from vault"),
            (Icon::Image, "Drag to add media from vault"),
        ]
        .iter()
        .enumerate()
        {
            let b = r(
                tb.x + 4.0 * k + i as f32 * 48.0 * k,
                tb.y + 4.0 * k,
                44.0 * k,
                40.0 * k,
            );
            icon(
                self.c,
                *ic,
                b.x + b.w * 0.5,
                b.y + b.h * 0.5,
                k * 1.3,
                t.muted,
            );
            self.graph_img
                .controls
                .push((b, Ctl::Section(110 + i as u8)));
            self.hit(b, Zone::Canvas(pane));
            self.said.button(b, label);
        }
    }

    pub(super) fn canvas_context(&mut self, px: f32, py: f32) {
        let Some(a) = self
            .view_areas
            .iter()
            .find(|(p, _)| *p == self.app.pane)
            .map(|(_, a)| *a)
        else {
            return;
        };
        let Some(View::Canvas(c)) = self.app.view() else {
            return;
        };
        let wx = (px - a.x - a.w * 0.5) / c.zoom + c.cam.0;
        let wy = (py - a.y - a.h * 0.5) / c.zoom + c.cam.1;
        let id = c
            .canvas
            .hit(wx, wy)
            .and_then(|i| c.canvas.nodes.get(i))
            .map(|n| n.id.clone());
        if let Some(id) = id {
            self.app.card_menu(&id, px, py);
        }
    }

    // ---- a base -------------------------------------------------------------------------

    pub(super) fn draw_base(&mut self, pane: usize, a: Rect, focused: bool) {
        let t = self.t();
        let sz = self.sz;
        let k = sz.scale;
        let (tabs, act) = self.app.pane_tabs(pane);
        let Some(View::Base(b)) = tabs.get(act).and_then(|tb| tb.view.as_ref()) else {
            return;
        };
        let view = b.view;
        let rows = b.rows.clone();
        let base = b.base.clone();
        let choosing = b.choosing_view;
        let scroll = b.scroll;
        let Some(w) = base.views.get(view).cloned() else {
            return;
        };
        // The toolbar.
        let bar = r(a.x, a.y, a.w, 40.0 * k);
        let cy = bar.y + bar.h * 0.5;
        icon(self.c, Icon::Table, bar.x + 20.0 * k, cy, k, t.text);
        let nw = width(self.c, &w.name, sz.medium, 500);
        text(
            self.c,
            &w.name,
            bar.x + 34.0 * k,
            cy + sz.medium * 0.36,
            sz.medium,
            500,
            t.text,
        );
        icon(
            self.c,
            Icon::ChevronsUpDown,
            bar.x + 46.0 * k + nw,
            cy,
            k * 0.85,
            t.muted,
        );
        let vb = r(
            bar.x + 8.0 * k,
            bar.y + 4.0 * k,
            nw + 56.0 * k,
            bar.h - 8.0 * k,
        );
        if focused {
            self.button(vb, &w.name, Hit::BaseViews);
        }
        let count = crate::text::count(rows.len(), "result", "results");
        text(
            self.c,
            &count,
            vb.right() + 12.0 * k,
            cy + sz.ui * 0.36,
            sz.ui,
            400,
            t.muted,
        );
        let actions = [
            (Icon::Sort, "Sort", !w.sort.is_empty()),
            (Icon::LayoutList, "Group", false),
            (
                Icon::Filter,
                "Filter",
                w.filters.is_some() || base.filters.is_some(),
            ),
            (Icon::List, "Properties", false),
            (Icon::Search, "Search", false),
            (Icon::Plus, "New", false),
        ];
        let mut x = bar.right() - 8.0 * k;
        for (ic, label, on) in actions.iter().rev() {
            let lw = width(self.c, label, sz.ui, 400);
            x -= lw + 28.0 * k;
            let c = if *on { t.link } else { t.muted };
            icon(self.c, *ic, x + 9.0 * k, cy, k, c);
            text(
                self.c,
                label,
                x + 22.0 * k,
                cy + sz.ui * 0.36,
                sz.ui,
                400,
                c,
            );
            self.said.text(r(x, bar.y, lw + 28.0 * k, bar.h), label);
            x -= 6.0 * k;
        }
        // The table.
        let cols: Vec<String> = if w.order.is_empty() {
            alloc::vec![String::from("file.name")]
        } else {
            w.order.clone()
        };
        let vault = self.app.vault.as_ref();
        let cell = |p: &str, c: &str| -> String {
            let Some(v) = vault else {
                return String::new();
            };
            let m = v.meta.get(p);
            let none = Vec::new();
            let nolinks = Vec::new();
            let row = crate::bases::Row {
                path: p,
                tags: m.map_or(&none, |m| &m.tags),
                links: m.map_or(&nolinks, |m| &m.links),
                props: m.and_then(|m| m.props.as_ref()),
            };
            crate::bases::show(&crate::bases::value(c, &row))
        };
        let fsz = sz.ui;
        let mut widths: Vec<f32> = cols
            .iter()
            .map(|c| width(self.c, &base.column_name(c), fsz, 400) + 44.0 * k)
            .collect();
        let rows_vals: Vec<Vec<String>> = rows
            .iter()
            .take(400)
            .map(|p| cols.iter().map(|c| cell(p, c)).collect())
            .collect();
        for rv in &rows_vals {
            for (i, v) in rv.iter().enumerate() {
                let w = width(self.c, v, fsz, 400) + 24.0 * k;
                if let Some(x) = widths.get_mut(i) {
                    *x = x.max(w).clamp(60.0 * k, 300.0 * k);
                }
            }
        }
        let rh = 30.0 * k;
        let top = bar.bottom();
        let tbl = r(a.x, top, a.w, a.bottom() - top);
        let _ = canvas2d::set_clip(self.c, tbl.x, tbl.y, tbl.w, tbl.h);
        fill(self.c, r(tbl.x, top, tbl.w, 1.0), t.border);
        let mut x = tbl.x;
        let ys = top - scroll;
        for (i, c) in cols.iter().enumerate() {
            let cw = widths[i];
            let name = base.column_name(c);
            let ic = if c == "file.name" {
                Icon::Info
            } else if rows_vals.iter().any(|r| {
                r.get(i)
                    .is_some_and(|v| v.len() == 10 && v.as_bytes().get(4) == Some(&b'-'))
            }) {
                Icon::Calendar
            } else if rows_vals.iter().any(|r| {
                r.get(i)
                    .is_some_and(|v| !v.is_empty() && v.parse::<f64>().is_ok())
            }) {
                Icon::Hash
            } else {
                Icon::Text
            };
            icon(self.c, ic, x + 16.0 * k, ys + rh * 0.5, k * 0.9, t.muted);
            text(
                self.c,
                &name,
                x + 30.0 * k,
                ys + rh * 0.5 + fsz * 0.36,
                fsz,
                400,
                t.muted,
            );
            if let Some((_, desc)) = w.sort.iter().find(|(p, _)| p == c) {
                let nwid = width(self.c, &name, fsz, 400);
                icon(
                    self.c,
                    if *desc {
                        Icon::ChevronDown
                    } else {
                        Icon::ArrowLeft
                    },
                    x + 40.0 * k + nwid,
                    ys + rh * 0.5,
                    k * 0.75,
                    t.muted,
                );
            }
            let hc = r(x, ys, cw, rh);
            if focused {
                self.button(hc, &name, Hit::BaseSort(i));
            }
            x += cw;
            fill(
                self.c,
                r(x, ys, 1.0, rh * (rows_vals.len() + 1) as f32),
                t.border,
            );
        }
        fill(self.c, r(tbl.x, ys + rh, tbl.w, 1.0), t.border);
        for (ri, rv) in rows_vals.iter().enumerate() {
            let y = ys + rh * (ri + 1) as f32;
            if y > tbl.bottom() {
                break;
            }
            let mut x = tbl.x;
            for (i, v) in rv.iter().enumerate() {
                let cw = widths[i];
                let base_y = y + rh * 0.5 + fsz * 0.36;
                let is_name = cols.get(i).is_some_and(|c| c == "file.name");
                let numeric = !v.is_empty() && v.parse::<f64>().is_ok();
                if is_name {
                    let lw = fit(
                        self.c,
                        v,
                        x + 16.0 * k,
                        base_y,
                        fsz,
                        400,
                        t.link,
                        cw - 24.0 * k,
                    );
                    fill(self.c, r(x + 16.0 * k, base_y + 2.0, lw, 1.0), t.link);
                    if focused {
                        self.button(r(x, y, cw, rh), v, Hit::BaseRow(ri));
                    }
                } else if numeric {
                    let vw = width(self.c, v, fsz, 400);
                    text(self.c, v, x + cw - vw - 14.0 * k, base_y, fsz, 400, t.text);
                    self.said.text(r(x, y, cw, rh), v);
                } else {
                    fit(
                        self.c,
                        v,
                        x + 16.0 * k,
                        base_y,
                        fsz,
                        400,
                        t.text,
                        cw - 24.0 * k,
                    );
                    self.said.text(r(x, y, cw, rh), v);
                }
                x += cw;
            }
            fill(self.c, r(tbl.x, y + rh, tbl.w, 1.0), t.border);
        }
        let _ = canvas2d::clear_clip(self.c);
        if choosing && focused {
            let mh = base.views.len() as f32 * 30.0 * k + 12.0;
            let card = r(vb.x, vb.bottom() + 4.0, 220.0 * k, mh);
            round(self.c, card, 8.0, t.side);
            look::outline(self.c, card, 8.0, 1.0, t.border_hover);
            for (i, v) in base.views.iter().enumerate() {
                let ir = r(
                    card.x + 6.0,
                    card.y + 6.0 + i as f32 * 30.0 * k,
                    card.w - 12.0,
                    30.0 * k,
                );
                if i == view {
                    round(self.c, ir, 5.0, t.hover);
                }
                icon(
                    self.c,
                    Icon::Table,
                    ir.x + 14.0 * k,
                    ir.y + ir.h * 0.5,
                    k * 0.9,
                    t.muted,
                );
                text(
                    self.c,
                    &v.name,
                    ir.x + 28.0 * k,
                    ir.y + ir.h * 0.5 + sz.ui * 0.36,
                    sz.ui,
                    400,
                    t.text,
                );
                self.button(ir, &v.name, Hit::BaseView(i));
            }
        }
    }

    // ---- a note's saved copies ------------------------------------------------------------

    pub(super) fn draw_recovery(&mut self, pane: usize, a: Rect, focused: bool) {
        let t = self.t();
        let sz = self.sz;
        let k = sz.scale;
        let (tabs, act) = self.app.pane_tabs(pane);
        let Some(View::Recovery(rv)) = tabs.get(act).and_then(|tb| tb.view.as_ref()) else {
            return;
        };
        let path = rv.path.clone();
        let pick = rv.pick;
        let snaps: Vec<(u64, String)> = self
            .app
            .recovery
            .snapshots(&path)
            .iter()
            .map(|s| (s.at, s.text.clone()))
            .collect();
        let list = r(a.x, a.y, (a.w * 0.3).clamp(180.0, 260.0), a.h);
        fill(self.c, list, t.side);
        fill(self.c, r(list.right() - 1.0, a.y, 1.0, a.h), t.border);
        let mut title = String::from(crate::app::strip_ext(crate::vfs::file_name(&path)));
        title.push_str(": snapshots");
        fit(
            self.c,
            &title,
            list.x + 16.0 * k,
            a.y + 26.0 * k,
            sz.ui,
            600,
            t.text,
            list.w - 24.0,
        );
        if snaps.is_empty() {
            text(
                self.c,
                "No snapshots yet.",
                list.x + 16.0 * k,
                a.y + 56.0 * k,
                sz.ui,
                400,
                t.faint,
            );
        }
        let mut y = a.y + 44.0 * k;
        for (i, (at, _)) in snaps.iter().enumerate().rev() {
            if y > a.bottom() {
                break;
            }
            let lt = krate::locale::local_time(*at);
            let when = crate::dates::When {
                year: lt.year,
                month: lt.month,
                day: lt.day,
                hour: lt.hour,
                minute: lt.minute,
                second: lt.second,
            }
            .format("YYYY-MM-DD HH:mm");
            let rr = r(list.x + 8.0 * k, y, list.w - 16.0 * k, sz.row);
            if i == pick {
                round(self.c, rr, 6.0, t.active);
            }
            text(
                self.c,
                &when,
                rr.x + 10.0 * k,
                y + sz.row * 0.5 + sz.ui * 0.36,
                sz.ui,
                400,
                t.text,
            );
            if focused {
                self.button(rr, &when, Hit::RecoveryPick(i));
            }
            y += sz.row;
        }
        let body = r(list.right(), a.y, a.right() - list.right(), a.h);
        if let Some((_, txt)) = snaps.get(pick) {
            let mut y = body.y + 56.0 * k;
            for line in txt.split('\n') {
                if y > body.bottom() {
                    break;
                }
                fit(
                    self.c,
                    line,
                    body.x + 24.0 * k,
                    y,
                    sz.body,
                    400,
                    t.text,
                    body.w - 48.0 * k,
                );
                y += sz.body * 1.5;
            }
            let b = r(
                body.right() - 110.0 * k,
                body.y + 12.0 * k,
                96.0 * k,
                30.0 * k,
            );
            round(self.c, b, 5.0, t.accent);
            let lw = width(self.c, "Restore", sz.ui, 500);
            text(
                self.c,
                "Restore",
                b.x + (b.w - lw) * 0.5,
                b.y + b.h * 0.5 + sz.ui * 0.36,
                sz.ui,
                500,
                t.on_accent,
            );
            if focused {
                self.button(b, "Restore", Hit::RecoveryRestore);
            }
        }
    }

    // ---- the settings window -----------------------------------------------------------------

    pub(super) fn draw_settings(&mut self, w: f32, h: f32) {
        let t = self.t();
        let sz = self.sz;
        let k = sz.scale;
        let page = match self.app.overlay {
            Some(Overlay::Settings(p)) => p,
            _ => return,
        };
        let mw = (w * 0.9).min(1100.0 * k).min(w - 16.0);
        let mh = (h * 0.85).min(1000.0 * k);
        let card = r((w - mw) * 0.5, (h - mh) * 0.5, mw, mh);
        round(self.c, card, 12.0, t.bg);
        look::outline(self.c, card, 12.0, 1.0, t.border_hover);
        self.settings_area = Some(card);
        self.hit(card, Zone::Settings);
        // The list of pages on the left.
        let side = r(card.x, card.y, (mw * 0.25).clamp(180.0, 240.0 * k), mh);
        let _ = canvas2d::fill_round_rect(
            self.c,
            look::g(side),
            gfx::CornerRadii {
                top_left: 12.0,
                top_right: 0.0,
                bottom_right: 0.0,
                bottom_left: 12.0,
            },
            t.side,
        );
        fill(self.c, r(side.right() - 1.0, side.y, 1.0, side.h), t.border);
        let groups: [NavGroup; 2] = [
            (
                "Options",
                &[
                    (SettingsPage::General, "General", ""),
                    (SettingsPage::Editor, "Editor", ""),
                    (SettingsPage::Files, "Files and links", ""),
                    (SettingsPage::Appearance, "Appearance", ""),
                    (SettingsPage::Hotkeys, "Hotkeys", ""),
                    (SettingsPage::CorePlugins, "Core plugins", ""),
                ],
            ),
            (
                "Core plugins",
                &[
                    (SettingsPage::DailyNotes, "Daily notes", "daily-notes"),
                    (SettingsPage::FileRecovery, "File recovery", "file-recovery"),
                    (SettingsPage::Templates, "Templates", "templates"),
                ],
            ),
        ];
        let mut y = side.y + 24.0 * k;
        for (gname, items) in groups {
            text(
                self.c,
                gname,
                side.x + 20.0 * k,
                y + 8.0 * k,
                sz.small,
                600,
                t.faint,
            );
            y += 24.0 * k;
            for (p, label, plugin) in items {
                if !plugin.is_empty() && !self.app.settings.plugin_on(plugin) {
                    continue;
                }
                let ir = r(side.x + 10.0 * k, y, side.w - 20.0 * k, 28.0 * k);
                let z = Zone::App(Hit::SettingsNav(*p));
                if *p == page {
                    round(self.c, ir, 6.0, t.active);
                } else if self.hovered(&z) {
                    round(self.c, ir, 6.0, t.hover);
                }
                text(
                    self.c,
                    label,
                    ir.x + 10.0 * k,
                    ir.y + ir.h * 0.5 + sz.ui * 0.36,
                    sz.ui,
                    400,
                    if *p == page { t.text } else { t.muted },
                );
                self.button(ir, label, Hit::SettingsNav(*p));
                y += 30.0 * k;
            }
            y += 12.0 * k;
        }
        // The close button.
        let cb = r(
            card.right() - 40.0 * k,
            card.y + 10.0 * k,
            28.0 * k,
            28.0 * k,
        );
        icon(
            self.c,
            Icon::Close,
            cb.x + cb.w * 0.5,
            cb.y + cb.h * 0.5,
            k,
            t.muted,
        );
        self.button(cb, "Close", Hit::SettingsClose);
        // The page.
        let body = r(
            side.right() + 40.0 * k,
            card.y,
            card.right() - side.right() - 80.0 * k,
            mh,
        );
        let title = match page {
            SettingsPage::General => "General",
            SettingsPage::Editor => "Editor",
            SettingsPage::Files => "Files and links",
            SettingsPage::Appearance => "Appearance",
            SettingsPage::Hotkeys => "Hotkeys",
            SettingsPage::CorePlugins => "Core plugins",
            SettingsPage::DailyNotes => "Daily notes",
            SettingsPage::FileRecovery => "File recovery",
            SettingsPage::Templates => "Templates",
        };
        let rows = self.app.settings_rows();
        let _ = canvas2d::set_clip(self.c, body.x - 8.0, card.y + 1.0, body.w + 16.0, mh - 2.0);
        let mut y = card.y + 40.0 * k - self.app.settings_scroll;
        text(self.c, title, body.x, y + 10.0 * k, 20.0 * k, 600, t.text);
        self.said.heading(r(body.x, y - 10.0, body.w, 30.0), title);
        y += 36.0 * k;
        let edit = self.app.setting_edit.clone();
        for (i, row) in rows.iter().enumerate() {
            if row.control == Control::Heading {
                y += 14.0 * k;
                text(
                    self.c,
                    &row.name,
                    body.x,
                    y + 16.0 * k,
                    sz.medium,
                    600,
                    t.text,
                );
                y += 30.0 * k;
                continue;
            }
            let ctl_w = match &row.control {
                Control::Text(_) => 200.0 * k,
                Control::Choice(_) => 220.0 * k,
                Control::Keys(_) => 140.0 * k,
                Control::Button(_) => 90.0 * k,
                Control::Toggle(_) => 40.0 * k,
                _ => 0.0,
            };
            let room = body.w - ctl_w - 24.0 * k;
            let desc_lines = if row.desc.is_empty() {
                Vec::new()
            } else {
                self.wrap(&row.desc, sz.small, room)
            };
            let rh = (18.0 * k + desc_lines.len() as f32 * sz.small * 1.4 + 22.0 * k).max(46.0 * k);
            fill(self.c, r(body.x, y, body.w, 1.0), t.border);
            let mut ty = y + 12.0 * k + sz.medium * 0.9;
            fit(
                self.c,
                &row.name,
                body.x,
                ty,
                sz.ui + 1.0,
                400,
                t.text,
                room,
            );
            for dl in &desc_lines {
                ty += sz.small * 1.4;
                text(self.c, dl, body.x, ty + 2.0, sz.small, 400, t.muted);
            }
            let cy = y + rh * 0.5;
            let cx = body.right() - ctl_w;
            let ctl = r(cx, cy - 14.0 * k, ctl_w, 28.0 * k);
            match &row.control {
                Control::Toggle(on) => self.toggle(r(cx, cy - 10.0 * k, 40.0 * k, 20.0 * k), *on),
                Control::Choice(v) => {
                    round(self.c, ctl, 5.0, t.button);
                    fit(
                        self.c,
                        v,
                        ctl.x + 10.0 * k,
                        cy + sz.ui * 0.36,
                        sz.ui,
                        400,
                        t.text,
                        ctl.w - 34.0 * k,
                    );
                    icon(
                        self.c,
                        Icon::ChevronsUpDown,
                        ctl.right() - 14.0 * k,
                        cy,
                        k * 0.8,
                        t.muted,
                    );
                }
                Control::Text(v) => {
                    round(self.c, ctl, 5.0, t.field);
                    let editing = edit
                        .as_ref()
                        .filter(|(r_, _)| *r_ == i)
                        .map(|(_, inp)| inp.clone());
                    look::outline(
                        self.c,
                        ctl,
                        5.0,
                        1.0,
                        if editing.is_some() {
                            t.border_focus
                        } else {
                            t.border
                        },
                    );
                    match editing {
                        Some(inp) => self.draw_input(
                            &inp,
                            ctl.x + 8.0 * k,
                            cy + sz.ui * 0.36,
                            sz.ui,
                            t.text,
                            ctl.right() - 8.0,
                            true,
                        ),
                        None => {
                            fit(
                                self.c,
                                v,
                                ctl.x + 8.0 * k,
                                cy + sz.ui * 0.36,
                                sz.ui,
                                400,
                                if v.is_empty() { t.faint } else { t.text },
                                ctl.w - 16.0 * k,
                            );
                        }
                    }
                    self.said.field(ctl, &row.name, v);
                }
                Control::Button(label) => {
                    round(self.c, ctl, 5.0, t.button);
                    let lw = width(self.c, label, sz.ui, 400);
                    text(
                        self.c,
                        label,
                        ctl.x + (ctl.w - lw) * 0.5,
                        cy + sz.ui * 0.36,
                        sz.ui,
                        400,
                        t.text,
                    );
                }
                Control::Keys(keys) => {
                    let kw = width(self.c, keys, sz.small, 400) + 16.0 * k;
                    let kb = r(body.right() - kw, cy - 11.0 * k, kw, 22.0 * k);
                    round(self.c, kb, 11.0 * k, t.hover);
                    text(
                        self.c,
                        keys,
                        kb.x + 8.0 * k,
                        cy + sz.small * 0.36,
                        sz.small,
                        400,
                        if keys == "Blank" { t.faint } else { t.text },
                    );
                }
                Control::None | Control::Heading => {}
            }
            self.button(r(body.x, y, body.w, rh), &row.name, Hit::SettingsRow(i));
            y += rh;
        }
        let _ = canvas2d::clear_clip(self.c);
    }

    // ---- pointer on the views -------------------------------------------------------------------

    pub(super) fn view_press(&mut self, z: Zone, px: f32, py: f32, m: Mods) {
        match z {
            Zone::GraphPanel | Zone::GraphSlider(_) => {
                let ctl = self
                    .graph_img
                    .controls
                    .iter()
                    .rev()
                    .find(|(rc, _)| rc.contains(px, py))
                    .map(|(_, c)| *c);
                let Some(ctl) = ctl else {
                    return;
                };
                self.graph_control(ctl, px);
            }
            Zone::Graph(p) => {
                self.app.focus_pane(p);
                let hover = match self.app.view() {
                    Some(View::Graph(g)) => g.hover,
                    _ => None,
                };
                self.drag = Drag::Graph {
                    x: px,
                    y: py,
                    node: hover,
                    moved: false,
                };
                self.drag_mods = m;
            }
            Zone::Canvas(p) => {
                self.app.focus_pane(p);
                self.app.focus = crate::app::Focus::Editor;
                // The controls and the toolbar.
                let ctl = self
                    .graph_img
                    .controls
                    .iter()
                    .rev()
                    .find(|(rc, _)| rc.contains(px, py))
                    .map(|(_, c)| *c);
                if let Some(Ctl::Section(n)) = ctl {
                    if n >= 100 {
                        self.canvas_control(n);
                        return;
                    }
                }
                self.canvas_press(px, py, m);
            }
            Zone::Settings => {}
            _ => {}
        }
    }

    fn graph_control(&mut self, ctl: Ctl, px: f32) {
        let mut ask = None;
        if let Some(View::Graph(g)) = self.app.view_mut() {
            let o = &mut g.opt;
            match ctl {
                Ctl::Section(0) => o.open_filters = !o.open_filters,
                Ctl::Section(1) => o.open_groups = !o.open_groups,
                Ctl::Section(2) => o.open_display = !o.open_display,
                Ctl::Section(_) => o.open_forces = !o.open_forces,
                Ctl::Toggle(0) => o.show_tags = !o.show_tags,
                Ctl::Toggle(1) => o.show_attachments = !o.show_attachments,
                Ctl::Toggle(2) => o.hide_unresolved = !o.hide_unresolved,
                Ctl::Toggle(3) => o.show_orphans = !o.show_orphans,
                Ctl::Toggle(_) => o.show_arrow = !o.show_arrow,
                Ctl::Slider(i) => {
                    self.drag = Drag::Slider(i);
                    let _ = px;
                }
                Ctl::Search => ask = Some(AskFor::GraphSearch),
                Ctl::NewGroup => ask = Some(AskFor::GraphGroup),
                Ctl::RemoveGroup(i) => {
                    if i < o.groups.len() {
                        o.groups.remove(i);
                    }
                }
                Ctl::Reset => {
                    let keep = (o.open_filters, o.open_groups, o.open_display, o.open_forces);
                    *o = crate::graph::Options::default();
                    o.open_filters = keep.0;
                    o.open_groups = keep.1;
                    o.open_display = keep.2;
                    o.open_forces = keep.3;
                }
                Ctl::Close => g.controls = false,
                Ctl::Open => g.controls = true,
            }
            if matches!(ctl, Ctl::Toggle(0..=3) | Ctl::RemoveGroup(_) | Ctl::Reset) {
                g.stale = true;
            }
            if matches!(ctl, Ctl::Slider(3..=6) | Ctl::Reset) {
                if let Some(gr) = g.graph.as_mut() {
                    gr.reheat(0.5);
                }
            }
        }
        if matches!(ctl, Ctl::Slider(_)) {
            self.view_drag(px, 0.0);
        }
        if let Some(purpose) = ask {
            let current = match (self.app.view(), &purpose) {
                (Some(View::Graph(g)), AskFor::GraphSearch) => g.opt.search.clone(),
                _ => String::new(),
            };
            let mut input = Input::with(&current);
            input.select_all();
            self.app.overlay = Some(Overlay::Ask(Ask {
                title: String::from(if purpose == AskFor::GraphSearch {
                    "Search files"
                } else {
                    "New group: search for its notes (path:, tag:#, file:)"
                }),
                input,
                purpose,
            }));
        }
        self.app.save_graph_options();
    }

    fn canvas_area(&self) -> Option<Rect> {
        self.view_areas
            .iter()
            .find(|(p, _)| *p == self.app.pane)
            .map(|(_, a)| *a)
    }

    fn canvas_control(&mut self, n: u8) {
        let area = self.canvas_area();
        match n {
            110 => self.app.canvas_add(CardKind::Text, ""),
            111 => self.app.open_list(ListKind::CanvasNote),
            112 => {
                self.app.overlay = Some(Overlay::Ask(Ask {
                    title: String::from("Add web page"),
                    input: Input::with("https://"),
                    purpose: AskFor::CanvasLink,
                }));
            }
            104 => {
                let _ = self.app.key(
                    "z",
                    Mods {
                        primary: true,
                        ..Mods::default()
                    },
                );
            }
            105 => {
                let _ = self.app.key(
                    "z",
                    Mods {
                        primary: true,
                        shift: true,
                        ..Mods::default()
                    },
                );
            }
            _ => {
                if let Some(View::Canvas(c)) = self.app.view_mut() {
                    match n {
                        100 => c.zoom = (c.zoom * 1.25).min(4.0),
                        101 => c.zoom = 1.0,
                        102 => c.fitted = false,
                        103 => c.zoom = (c.zoom / 1.25).max(0.1),
                        _ => {}
                    }
                }
                let _ = area;
            }
        }
    }

    fn canvas_press(&mut self, px: f32, py: f32, m: Mods) {
        let Some(a) = self.canvas_area() else {
            return;
        };
        let now = super::ms();
        let (lt, lx, ly) = self.graph_img.last_click;
        let double = now.saturating_sub(lt) < 450 && (px - lx).abs() < 5.0 && (py - ly).abs() < 5.0;
        self.graph_img.last_click = (now, px, py);
        self.app.canvas_commit();
        let seed = self.app.now;
        let Some(View::Canvas(c)) = self.app.view_mut() else {
            return;
        };
        let z = c.zoom;
        let wx = (px - a.x - a.w * 0.5) / z + c.cam.0;
        let wy = (py - a.y - a.h * 0.5) / z + c.cam.1;
        let before = c.canvas.to_text();
        // A side handle of the selected card starts an arrow; its corner
        // resizes it.
        if let Some(sel) = c
            .selected
            .clone()
            .and_then(|id| c.canvas.card(&id).cloned())
        {
            for side in [Side::Top, Side::Right, Side::Bottom, Side::Left] {
                let (sx, sy) = sel.anchor(side);
                if ((sx - wx) * z).abs() < 8.0 && ((sy - wy) * z).abs() < 8.0 {
                    self.drag = Drag::Canvas {
                        x: px,
                        y: py,
                        what: CanvasDrag::Connect(sel.id.clone(), side),
                        moved: false,
                        before,
                    };
                    return;
                }
            }
            if ((sel.x + sel.w - wx) * z).abs() < 10.0 && ((sel.y + sel.h - wy) * z).abs() < 10.0 {
                self.drag = Drag::Canvas {
                    x: px,
                    y: py,
                    what: CanvasDrag::Resize(sel.id.clone()),
                    moved: false,
                    before,
                };
                return;
            }
        }
        let hit = c
            .canvas
            .hit(wx, wy)
            .and_then(|i| c.canvas.nodes.get(i))
            .map(|n| (n.id.clone(), n.kind, n.text.clone(), n.file.clone()));
        match hit {
            Some((id, kind, text, file)) => {
                c.selected = Some(id.clone());
                if double {
                    match kind {
                        CardKind::Text => {
                            c.editing = Some((id, Input::with(&text.unwrap_or_default())));
                        }
                        CardKind::File => {
                            if let Some(f) = file {
                                self.app.open_file(&f, true);
                            }
                        }
                        _ => {}
                    }
                    return;
                }
                let _ = m;
                self.drag = Drag::Canvas {
                    x: px,
                    y: py,
                    what: CanvasDrag::Move(id),
                    moved: false,
                    before,
                };
            }
            None => {
                c.selected = None;
                if double {
                    // A double-click on the board makes a card there.
                    let id =
                        c.canvas
                            .add(CardKind::Text, wx - 125.0, wy - 30.0, 250.0, 60.0, "", seed);
                    c.selected = Some(id.clone());
                    c.editing = Some((id, Input::default()));
                    self.app.canvas_changed(before);
                    return;
                }
                self.drag = Drag::Canvas {
                    x: px,
                    y: py,
                    what: CanvasDrag::Pan,
                    moved: false,
                    before,
                };
            }
        }
    }

    pub(super) fn view_drag(&mut self, px: f32, py: f32) {
        match self.drag.clone() {
            Drag::Slider(i) => {
                let Some(&(_, tx, tw)) = self.graph_img.sliders.iter().find(|(n, _, _)| *n == i)
                else {
                    return;
                };
                let (_, lo, hi) = SLIDERS[i as usize];
                let f = ((px - tx) / tw.max(1.0)).clamp(0.0, 1.0);
                if let Some(View::Graph(g)) = self.app.view_mut() {
                    slider_set(&mut g.opt, i, lo + (hi - lo) * f);
                    if i >= 3 {
                        if let Some(gr) = g.graph.as_mut() {
                            gr.reheat(0.3);
                        }
                    }
                }
            }
            Drag::Graph { x, y, node, moved } => {
                let (dx, dy) = (px - x, py - y);
                if !moved && dx.abs() < 3.0 && dy.abs() < 3.0 {
                    return;
                }
                self.drag = Drag::Graph {
                    x: px,
                    y: py,
                    node,
                    moved: true,
                };
                if let Some(View::Graph(g)) = self.app.view_mut() {
                    let z = g.zoom;
                    match (node, g.graph.as_mut()) {
                        (Some(n), Some(gr)) => {
                            if let Some(nd) = gr.nodes.get_mut(n) {
                                nd.x += dx / z;
                                nd.y += dy / z;
                                nd.fixed = true;
                            }
                            gr.reheat(0.3);
                        }
                        _ => {
                            g.cam.0 -= dx / z;
                            g.cam.1 -= dy / z;
                            g.fitted = true;
                        }
                    }
                }
            }
            Drag::Canvas {
                x,
                y,
                what,
                moved,
                before,
            } => {
                let (dx, dy) = (px - x, py - y);
                if !moved && dx.abs() < 3.0 && dy.abs() < 3.0 {
                    return;
                }
                self.drag = Drag::Canvas {
                    x: px,
                    y: py,
                    what: what.clone(),
                    moved: true,
                    before,
                };
                let Some(View::Canvas(c)) = self.app.view_mut() else {
                    return;
                };
                let z = c.zoom;
                match what {
                    CanvasDrag::Pan => {
                        c.cam.0 -= dx / z;
                        c.cam.1 -= dy / z;
                    }
                    CanvasDrag::Move(id) => {
                        let mut ids = c.canvas.inside(&id);
                        ids.push(id);
                        for i in ids {
                            if let Some(n) = c.canvas.card_mut(&i) {
                                n.x += dx / z;
                                n.y += dy / z;
                            }
                        }
                    }
                    CanvasDrag::Resize(id) => {
                        if let Some(n) = c.canvas.card_mut(&id) {
                            n.w = (n.w + dx / z).max(60.0);
                            n.h = (n.h + dy / z).max(40.0);
                        }
                    }
                    CanvasDrag::Connect(..) => {}
                }
            }
            _ => {}
        }
    }

    pub(super) fn view_release(&mut self, drag: Drag, px: f32, py: f32) {
        match drag {
            Drag::Graph { node, moved, .. } => {
                if let Some(View::Graph(g)) = self.app.view_mut() {
                    if let (Some(n), Some(gr)) = (node, g.graph.as_mut()) {
                        if let Some(nd) = gr.nodes.get_mut(n) {
                            nd.fixed = false;
                        }
                    }
                }
                if !moved {
                    // A click on a dot opens its note.
                    let path = match (self.app.view(), node) {
                        (Some(View::Graph(g)), Some(n)) => g
                            .graph
                            .as_ref()
                            .and_then(|gr| gr.nodes.get(n))
                            .and_then(|nd| nd.path.clone()),
                        _ => None,
                    };
                    if let Some(p) = path {
                        let new_tab = self.drag_mods.primary;
                        let local =
                            matches!(self.app.view(), Some(View::Graph(g)) if g.local.is_some());
                        if local {
                            // A local graph opens the note beside it.
                            let me = self.app.pane;
                            let other = self.app.pane_order().into_iter().find(|q| *q != me);
                            if let Some(o) = other {
                                self.app.focus_pane(o);
                            }
                            self.app.open_file(&p, new_tab);
                        } else {
                            self.app.open_file(&p, new_tab);
                        }
                    }
                }
            }
            Drag::Slider(_) => self.app.save_graph_options(),
            Drag::Canvas {
                what,
                moved,
                before,
                ..
            } => match what {
                CanvasDrag::Connect(from, side) => {
                    if let Some(a) = self.canvas_area() {
                        let seed = self.app.now;
                        if let Some(View::Canvas(c)) = self.app.view_mut() {
                            let wx = (px - a.x - a.w * 0.5) / c.zoom + c.cam.0;
                            let wy = (py - a.y - a.h * 0.5) / c.zoom + c.cam.1;
                            let target = c
                                .canvas
                                .hit(wx, wy)
                                .and_then(|i| c.canvas.nodes.get(i))
                                .map(|n| n.id.clone());
                            if let (Some(to), Some(fc)) = (target, c.canvas.card(&from).cloned()) {
                                let to_side = c
                                    .canvas
                                    .card(&to)
                                    .map_or(Side::Left, |tc| canvas::facing(tc, &fc));
                                if c.canvas.connect(&from, side, &to, to_side, seed).is_some() {
                                    self.app.canvas_changed(before);
                                }
                            }
                        }
                    }
                }
                CanvasDrag::Pan => {}
                _ => {
                    if moved {
                        self.app.canvas_changed(before);
                    }
                }
            },
            _ => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn view_wheel(
        &mut self,
        pane: usize,
        a: Rect,
        x: f32,
        y: f32,
        dx: f32,
        dy: f32,
        m: Mods,
    ) {
        if pane != self.app.pane {
            self.app.focus_pane(pane);
        }
        match self.app.view_mut() {
            Some(View::Graph(g)) => {
                // The wheel zooms the graph, at the pointer.
                let f = (-dy * 0.002).exp();
                let wx = (x - a.x - a.w * 0.5) / g.zoom + g.cam.0;
                let wy = (y - a.y - a.h * 0.5) / g.zoom + g.cam.1;
                g.zoom = (g.zoom * f).clamp(0.02, 8.0);
                g.cam.0 = wx - (x - a.x - a.w * 0.5) / g.zoom;
                g.cam.1 = wy - (y - a.y - a.h * 0.5) / g.zoom;
                g.fitted = true;
                let _ = dx;
            }
            Some(View::Canvas(c)) => {
                if m.primary || m.ctrl {
                    let f = (-dy * 0.004).exp();
                    let wx = (x - a.x - a.w * 0.5) / c.zoom + c.cam.0;
                    let wy = (y - a.y - a.h * 0.5) / c.zoom + c.cam.1;
                    c.zoom = (c.zoom * f).clamp(0.1, 4.0);
                    c.cam.0 = wx - (x - a.x - a.w * 0.5) / c.zoom;
                    c.cam.1 = wy - (y - a.y - a.h * 0.5) / c.zoom;
                } else {
                    c.cam.0 += dx / c.zoom;
                    c.cam.1 += dy / c.zoom;
                }
                c.fitted = true;
            }
            _ => {}
        }
    }

    /// Called once a frame after drawing set the view areas.
    pub(super) fn after_draw(&mut self) {
        self.fit_views();
    }
}
