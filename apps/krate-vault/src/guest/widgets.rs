//! The blocks a note draws whole: tables, callouts, `$$` math, the
//! Properties block, embedded notes and pictures, and reading view's
//! footnotes -- plus the find bar and the page preview that floats over a
//! link. Each is laid out once into a [`Widget`] cached like a line (by the
//! text of every line it covers), drawn from that, and answers presses.
//!
//! The measurements follow Obsidian's default theme as seen side by side:
//! table cells 8 by 3 px of padding with 1 px borders, callouts 12 px of
//! padding (24 at the left) on a tenth of their colour, the Properties
//! block's 148 px key column, display math centred.

use super::editor::{
    self, draw_line, draw_math, fnv, layout, Act, Ctx, Deco, FindHit, Lay, LayCache, LineDraw,
    Shown, View,
};
use super::look::{self, fill, r, round, Rgba, Sizes, Theme};
use crate::blocks::{self, Align, CalloutIcon};
use crate::buffer::Pos;
use crate::editor::{Doc, Mode, Span, SpanKind};
use crate::math::{self, MathBox};
use crate::md::{self, Block};
use crate::props::{self, Kind, Value};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use krate::gfx::canvas2d;
use krate::layout::Rect;
#[allow(unused_imports)]
use krate::prelude::*;

// ---- what blocks need from outside the note ---------------------------------------------------

pub enum NoteText {
    Missing,
    Text(Rc<String>),
}

pub enum Img {
    Ready { handle: u64, w: u32, h: u32 },
    Failed(String),
}

/// Embedded notes and pictures, read from the vault between frames.
#[derive(Default)]
pub struct Env {
    notes: BTreeMap<String, NoteText>,
    images: BTreeMap<String, Img>,
    want_notes: BTreeSet<String>,
    want_images: BTreeSet<String>,
    generation: u64,
}

impl Env {
    /// A note's text by link name; `None` while it is being read.
    pub fn note(&mut self, name: &str) -> Option<&NoteText> {
        let k = crate::text::lower(name.trim());
        if !self.notes.contains_key(&k) {
            self.want_notes.insert(k.clone());
            return None;
        }
        self.notes.get(&k)
    }

    /// A picture by its name as written and the folder of the note that
    /// embeds it; `None` while it is being read.
    pub fn image(&mut self, target: &str, folder: &str) -> Option<&Img> {
        let mut k = String::from(target.trim());
        k.push('\n');
        k.push_str(folder);
        if !self.images.contains_key(&k) {
            self.want_images.insert(k.clone());
            return None;
        }
        self.images.get(&k)
    }

    fn image_ready(&self, target: &str, folder: &str) -> Option<&Img> {
        let mut k = String::from(target.trim());
        k.push('\n');
        k.push_str(folder);
        self.images.get(&k)
    }
}

/// Read what the last frame asked for. True when something arrived (the
/// note should be drawn again).
pub fn fetch(cache: &mut LayCache, app: &mut crate::app::App, c: u64) -> bool {
    let env = &mut cache.env;
    let Some(gen) = app.vault.as_ref().map(|v| v.generation) else {
        return false;
    };
    let idle = app.vault.as_ref().is_some_and(|v| v.index_pending() == 0);
    if gen != env.generation && idle {
        // A note changed on disk or was saved: embeds read it again.
        env.generation = gen;
        if !env.notes.is_empty() {
            env.notes.clear();
            cache.clear();
            return true;
        }
    }
    let env = &mut cache.env;
    if env.want_notes.is_empty() && env.want_images.is_empty() {
        return false;
    }
    let mut got = false;
    for name in core::mem::take(&mut env.want_notes) {
        let path = app.vault.as_ref().and_then(|v| {
            v.resolve(&name)
                .and_then(|i| v.notes.get(i))
                .map(|n| n.path.clone())
        });
        let text = match path {
            Some(p) => match app.open_text(&p) {
                Some(t) => NoteText::Text(Rc::new(t)),
                None => match app.vault.as_mut().map(|v| crate::app::read_note(v, &p)) {
                    Some(Ok(t)) => NoteText::Text(Rc::new(t)),
                    _ => NoteText::Missing,
                },
            },
            None => NoteText::Missing,
        };
        env.notes.insert(name, text);
        got = true;
    }
    for key in core::mem::take(&mut env.want_images) {
        let (target, folder) = key.split_once('\n').unwrap_or((key.as_str(), ""));
        let img = match app.vault.as_mut() {
            Some(v) => load_image(v, target, folder, c),
            None => Img::Failed(String::from("No vault is open")),
        };
        env.images.insert(key.clone(), img);
        got = true;
    }
    if got {
        cache.clear();
    }
    got
}

fn load_image(v: &mut crate::vault::Vault, target: &str, folder: &str, c: u64) -> Img {
    if target.starts_with("http://") || target.starts_with("https://") {
        return Img::Failed(String::from("Pictures from the web are not loaded"));
    }
    let target = target.trim_start_matches("./").trim_start_matches('/');
    // Where Obsidian looks: next to the note, at the vault's root, then in
    // any folder (the attachment folder a vault sets is one of them).
    let mut tries: Vec<String> = Vec::new();
    tries.push(crate::vfs::join(folder, target));
    tries.push(String::from(target));
    for f in v.folder_paths() {
        tries.push(crate::vfs::join(&f, target));
    }
    for p in tries {
        if !v.fs.exists(&p) {
            continue;
        }
        return match v.fs.read_bytes(&p).and_then(|b| crate::image::decode(&b)) {
            Ok(pic) => match krate::gfx::canvas_images::keep(c, pic.width, pic.height, &pic.rgba) {
                Ok(handle) => Img::Ready {
                    handle,
                    w: pic.width,
                    h: pic.height,
                },
                Err(_) => Img::Failed(String::from("Krate could not keep the picture")),
            },
            Err(why) => Img::Failed(why),
        };
    }
    let mut s = String::from("\"");
    s.push_str(target);
    s.push_str("\" could not be found.");
    Img::Failed(s)
}

// ---- widgets ----------------------------------------------------------------------------------

/// A piece of Markdown laid out inside a block: a table cell, a callout's
/// line, a line of an embedded note.
pub struct Sub {
    pub text: String,
    pub lay: Rc<Lay>,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    /// The note line it came from, and the byte its text starts at there.
    pub line: Option<usize>,
    pub col0: usize,
}

pub struct PropRow {
    pub key: String,
    pub kind: Kind,
    pub y: f32,
    pub line: usize,
    pub value_col: usize,
    pub value: Value,
}

pub enum Widget {
    Table {
        cells: Vec<Sub>,
        cols: Vec<(f32, f32)>,
        rows: Vec<(f32, f32)>,
        w: f32,
        top: f32,
    },
    Callout {
        rgb: (u8, u8, u8),
        icon: CalloutIcon,
        title: String,
        title_w: f32,
        body: Vec<Sub>,
        fold: Option<bool>,
        head: usize,
        h: f32,
        top: f32,
        title_col: usize,
    },
    Math {
        b: MathBox,
        x: f32,
        base: f32,
        line: usize,
    },
    Props {
        rows: Vec<PropRow>,
        add_y: f32,
    },
    Image {
        target: String,
        folder: String,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        note: Option<String>,
        line: usize,
    },
    Note {
        subs: Vec<Sub>,
        h: f32,
        top: f32,
        missing: Option<String>,
        line: usize,
        title: Option<String>,
    },
    Footnotes {
        subs: Vec<(String, Sub)>,
        top: f32,
    },
    Diagram {
        d: crate::mermaid::Diagram,
        x: f32,
        y: f32,
        line: usize,
    },
}

impl Widget {
    /// What a screen reader (and check-app) is told is here.
    pub fn said(&self) -> String {
        let mut s = String::new();
        let mut add = |t: &str| {
            if !s.is_empty() && !t.is_empty() {
                s.push(' ');
            }
            s.push_str(t);
        };
        match self {
            Widget::Table { cells, .. } => {
                for c in cells {
                    add(&md::plain(&c.text, 0));
                }
            }
            Widget::Callout { title, body, .. } => {
                add(title);
                for b in body {
                    add(&md::plain(&b.text, 0));
                }
            }
            Widget::Props { rows, .. } => {
                add("Properties");
                for p in rows {
                    add(&p.key);
                    add(&p.value.shown());
                }
            }
            Widget::Note { subs, .. } => {
                for b in subs {
                    add(&md::plain(&b.text, 0));
                }
            }
            Widget::Footnotes { subs, .. } => {
                for (_, b) in subs {
                    add(&md::plain(&b.text, 0));
                }
            }
            Widget::Image { note: Some(n), .. } => add(n),
            Widget::Diagram { d, .. } => {
                for n in &d.nodes {
                    add(&n.label);
                }
            }
            Widget::Math { .. } | Widget::Image { .. } => {}
        }
        s
    }
}

fn sub(
    c: u64,
    text: String,
    block: Block,
    mode: Mode,
    width: f32,
    body: f32,
    code: f32,
) -> Rc<Lay> {
    let ctx = Ctx {
        block,
        mode,
        active: false,
        width,
        body,
        code,
        gap: false,
        comment: false,
        fold: 0,
        struck: false,
    };
    Rc::new(layout(c, &text, ctx, "", &[]))
}

fn widget_lay(w: Widget, h: f32, size: f32) -> Rc<Lay> {
    let mut l = Lay::hidden(size);
    l.h = h;
    if let Some(rl) = l.rows.first_mut() {
        rl.h = h;
    }
    l.deco = Deco::Widget;
    l.widget = Some(Rc::new(w));
    Rc::new(l)
}

/// The layout of a block drawn whole, from its first line.
pub fn block(cache: &mut LayCache, c: u64, doc: &Doc, span: Span, v: &View) -> Rc<Lay> {
    let folder = String::from(crate::vfs::parent(&doc.path));
    let mut h: u64 = 0x9e37_79b9_7f4a_7c15;
    fnv(
        &mut h,
        &[span.kind as u8, u8::from(doc.mode == Mode::Reading)],
    );
    for n in span.first..=span.last {
        let l = doc.line(n);
        fnv(&mut h, l.as_bytes());
        fnv(&mut h, b"\n");
    }
    fnv(&mut h, &v.width.to_bits().to_le_bytes());
    fnv(&mut h, &v.sz.body.to_bits().to_le_bytes());
    fnv(&mut h, &(span.first as u64).to_le_bytes());
    if span.kind == SpanKind::Callout {
        fnv(&mut h, &[u8::from(doc.callout_folded(span.first))]);
    }
    // An embed's look depends on what it embeds having arrived.
    let embed = if span.kind == SpanKind::Embed {
        blocks::embed(doc.line(span.first))
    } else {
        None
    };
    if let Some(e) = embed.as_ref() {
        if e.is_image() {
            let state: u8 = match cache.env.image(&e.target, &folder) {
                None => 0,
                Some(Img::Ready { w, .. }) => 1 + (*w % 200) as u8,
                Some(Img::Failed(_)) => 255,
            };
            fnv(&mut h, &[state]);
        } else {
            let state: u64 = match cache.env.note(&e.target) {
                None => 0,
                Some(NoteText::Missing) => 1,
                Some(NoteText::Text(t)) => {
                    let mut x: u64 = 7;
                    fnv(&mut x, t.as_bytes());
                    x
                }
            };
            fnv(&mut h, &state.to_le_bytes());
        }
    }
    if let Some(l) = cache.cached(h) {
        return l;
    }
    let sz = v.sz;
    let lay = match span.kind {
        SpanKind::Table => table(c, doc, span, v.width, &sz),
        SpanKind::Callout => callout(c, doc, span, v.width, &sz),
        SpanKind::Math => math_block(c, doc, span, v.width, &sz),
        SpanKind::Front => properties(c, doc, span, v.width, &sz),
        SpanKind::Diagram => diagram(c, doc, span, v.width, &sz),
        SpanKind::Embed => match embed {
            Some(e) if e.is_image() => image(&mut cache.env, &e, &folder, span.first, v.width, &sz),
            Some(e) => note_embed(c, &mut cache.env, &e, &folder, span.first, v.width, &sz),
            None => Rc::new(Lay::hidden(sz.body)),
        },
    };
    cache.put(h, lay.clone());
    lay
}

fn table(c: u64, doc: &Doc, span: Span, width: f32, sz: &Sizes) -> Rc<Lay> {
    let k = sz.body / 16.0;
    let (px, py) = (8.0 * k, 3.0 * k);
    let aligns = blocks::delimiter_row(doc.line(span.first + 1)).unwrap_or_default();
    let ncols = aligns.len().max(1);
    let mut grid: Vec<(usize, Vec<(usize, usize)>)> = Vec::new();
    for n in span.first..=span.last {
        if n == span.first + 1 {
            continue;
        }
        grid.push((n, blocks::split_row(doc.line(n))));
    }
    // Natural column widths: each cell on one line.
    let mut nat = alloc::vec![40.0 * k; ncols];
    for (ri, (n, cells)) in grid.iter().enumerate() {
        let line = doc.line(*n);
        for (ci, (a, z)) in cells.iter().enumerate().take(ncols) {
            let raw = line.get(*a..*z).unwrap_or("");
            let text = if ri == 0 && !raw.is_empty() {
                let mut s = String::from("**");
                s.push_str(raw);
                s.push_str("**");
                s
            } else {
                String::from(raw)
            };
            let l = sub(
                c,
                text,
                Block::Para,
                Mode::Live,
                100_000.0,
                sz.body,
                sz.code,
            );
            let w = l.end_x() + 2.0;
            if let Some(slot) = nat.get_mut(ci) {
                *slot = slot.max(w + px * 2.0);
            }
        }
    }
    let total: f32 = nat.iter().sum();
    let room = width - 2.0;
    let widths: Vec<f32> = if total <= room {
        nat
    } else {
        // Share the room: narrow columns keep their width, wide ones wrap.
        let fair = room / ncols as f32;
        let small: f32 = nat.iter().filter(|w| **w <= fair).sum();
        let wide: f32 = nat.iter().filter(|w| **w > fair).sum();
        let left = (room - small).max(ncols as f32 * 40.0);
        nat.iter()
            .map(|w| {
                if *w <= fair {
                    *w
                } else {
                    (w / wide * left).max(60.0 * k)
                }
            })
            .collect()
    };
    let mut cols = Vec::new();
    let mut x = 0.0f32;
    for w in &widths {
        cols.push((x, *w));
        x += w;
    }
    let tw = x;
    let top = 4.0 * k;
    let mut y = top;
    let mut rows = Vec::new();
    let mut cells = Vec::new();
    for (ri, (n, spans)) in grid.iter().enumerate() {
        let line = doc.line(*n);
        let mut row_h = sz.body * 1.5 + py * 2.0;
        let mut row_cells = Vec::new();
        for ci in 0..ncols {
            let (cx, cw) = cols.get(ci).copied().unwrap_or((0.0, 40.0));
            let (a, z) = spans.get(ci).copied().unwrap_or((line.len(), line.len()));
            let mut text = String::from(line.get(a..z).unwrap_or(""));
            if ri == 0 && !text.is_empty() {
                // The header is bold.
                text = {
                    let mut s = String::from("**");
                    s.push_str(&text);
                    s.push_str("**");
                    s
                };
            }
            let l = sub(
                c,
                text.clone(),
                Block::Para,
                Mode::Live,
                cw - px * 2.0,
                sz.body,
                sz.code,
            );
            row_h = row_h.max(l.h + py * 2.0);
            let al = aligns.get(ci).copied().unwrap_or(Align::Default);
            let used = l.end_x();
            let dx = match al {
                Align::Center => ((cw - px * 2.0) - used) * 0.5,
                Align::Right => (cw - px * 2.0) - used,
                _ => 0.0,
            }
            .max(0.0);
            row_cells.push(Sub {
                text,
                lay: l,
                x: cx + px + dx,
                y: 0.0,
                w: cw - px * 2.0,
                line: Some(*n),
                col0: if ri == 0 { a.saturating_sub(2) } else { a },
            });
        }
        for mut s in row_cells {
            s.y = y + py;
            cells.push(s);
        }
        rows.push((y, row_h));
        y += row_h;
    }
    let h = y + top * 2.0;
    widget_lay(
        Widget::Table {
            cells,
            cols,
            rows,
            w: tw,
            top,
        },
        h,
        sz.body,
    )
}

fn callout(c: u64, doc: &Doc, span: Span, width: f32, sz: &Sizes) -> Rc<Lay> {
    let k = sz.body / 16.0;
    let head_line = doc.line(span.first);
    let head = blocks::callout_head(head_line).unwrap_or(blocks::CalloutHead {
        kind: String::from("note"),
        fold: None,
        title: (0, 0),
    });
    let (icon, rgb, _) = blocks::callout_style(&head.kind);
    let title_raw = head_line.get(head.title.0..head.title.1).unwrap_or("");
    let title = if title_raw.trim().is_empty() {
        blocks::callout_default_title(&head.kind)
    } else {
        md::plain(title_raw, 0)
    };
    let folded = doc.callout_folded(span.first);
    let (pt, pl, pr, pb) = (12.0 * k, 24.0 * k, 12.0 * k, 12.0 * k);
    let title_h = sz.body * 1.5;
    let inner = width - pl - pr;
    let mut body = Vec::new();
    let mut y = pt + title_h;
    if !folded && span.last > span.first {
        y += 12.0 * k;
        let mut in_code = false;
        let mut prev_blank = true;
        for n in span.first + 1..=span.last {
            let line = doc.line(n);
            let (text, at) = blocks::strip_quote(line).unwrap_or((line, 0));
            let fence = md::is_fence(text);
            let block = md::classify(text, in_code && !fence);
            if fence {
                in_code = !in_code;
            }
            if block == Block::Blank {
                if !prev_blank {
                    y += 12.0 * k;
                }
                prev_blank = true;
                continue;
            }
            prev_blank = false;
            let l = sub(
                c,
                String::from(text),
                block,
                Mode::Live,
                inner,
                sz.body,
                sz.code,
            );
            let lh = l.h;
            body.push(Sub {
                text: String::from(text),
                lay: l,
                x: pl,
                y,
                w: inner,
                line: Some(n),
                col0: at,
            });
            y += lh;
        }
        y += 4.0 * k;
    }
    let h = y + pb;
    let title_w = look::width(c, &title, sz.body, 600);
    // Reading view's paragraph gap is the space between callouts there.
    let top = if doc.mode == Mode::Reading {
        0.0
    } else {
        4.0 * k
    };
    widget_lay(
        Widget::Callout {
            rgb,
            icon,
            title,
            title_w,
            body,
            fold: head.fold.map(|_| !folded),
            head: span.first,
            h,
            top,
            title_col: head.title.0,
        },
        h + top * 2.0,
        sz.body,
    )
}

fn math_block(c: u64, doc: &Doc, span: Span, width: f32, sz: &Sizes) -> Rc<Lay> {
    let k = sz.body / 16.0;
    let mut src = String::new();
    for n in span.first..=span.last {
        if !src.is_empty() {
            src.push('\n');
        }
        src.push_str(doc.line(n));
    }
    let tex = src.trim().trim_start_matches("$$").trim_end_matches("$$");
    let b = math::layout(tex, sz.body * 1.21, true, &editor::Cm(c));
    let pad = 10.0 * k;
    let x = ((width - b.w) * 0.5).max(0.0);
    let base = pad + b.asc;
    let h = base + b.desc + pad;
    widget_lay(
        Widget::Math {
            b,
            x,
            base,
            line: span.first,
        },
        h,
        sz.body,
    )
}

fn diagram(c: u64, doc: &Doc, span: Span, width: f32, sz: &Sizes) -> Rc<Lay> {
    let mut src = String::new();
    for n in span.first + 1..span.last {
        src.push_str(doc.line(n));
        src.push('\n');
    }
    let size = 14.0 * sz.body / 16.0;
    let measure = |s: &str| look::width(c, s, size, 400);
    match crate::mermaid::flowchart(&src, &measure, size) {
        Some(d) => {
            let pad = 12.0 * sz.body / 16.0;
            // Obsidian sets a diagram at the left of the column.
            let x = if d.w > width { 0.0 } else { 2.0 };
            let h = d.h + pad * 2.0;
            widget_lay(
                Widget::Diagram {
                    d,
                    x,
                    y: pad,
                    line: span.first,
                },
                h,
                sz.body,
            )
        }
        None => Rc::new(Lay::hidden(sz.body)),
    }
}

fn properties(_c: u64, doc: &Doc, span: Span, _width: f32, sz: &Sizes) -> Rc<Lay> {
    let k = sz.body / 16.0;
    let lines: Vec<&str> = (span.first + 1..span.last).map(|n| doc.line(n)).collect();
    let parsed = props::parse(&lines, span.first + 1);
    let mut rows = Vec::new();
    let mut y = 40.0 * k;
    for p in parsed {
        let line = doc.line(p.line);
        let value_col = line.find(':').map_or(line.len(), |i| {
            let rest = line.get(i + 1..).unwrap_or("");
            i + 1 + (rest.len() - rest.trim_start().len())
        });
        rows.push(PropRow {
            key: p.key,
            kind: p.kind,
            y,
            line: p.line,
            value_col,
            value: p.value,
        });
        y += 30.0 * k;
    }
    let add_y = y + 4.0 * k;
    let h = add_y + 30.0 * k + 22.0 * k;
    widget_lay(Widget::Props { rows, add_y }, h, sz.body)
}

fn image(
    env: &mut Env,
    e: &blocks::Embed,
    folder: &str,
    line: usize,
    width: f32,
    sz: &Sizes,
) -> Rc<Lay> {
    let k = sz.body / 16.0;
    let pad = 4.0 * k;
    let (w, h, note) = match env.image(&e.target, folder) {
        Some(Img::Ready { w, h, .. }) => {
            let (iw, ih) = (*w as f32, *h as f32);
            let want_w = e.width.map_or(iw, |x| x as f32);
            let ww = want_w.min(width).max(1.0);
            let hh = match (e.width, e.height) {
                (Some(_), Some(hh)) => hh as f32 * (ww / want_w),
                _ => ih * ww / iw.max(1.0),
            };
            (ww, hh, None)
        }
        Some(Img::Failed(why)) => (width, sz.body * 1.5, Some(why.clone())),
        None => (width, sz.body * 1.5, Some(String::from("Loading picture"))),
    };
    widget_lay(
        Widget::Image {
            target: e.target.clone(),
            folder: String::from(folder),
            x: 0.0,
            y: pad,
            w,
            h,
            note,
            line,
        },
        h + pad * 2.0,
        sz.body,
    )
}

/// The lines of a note's text laid out as reading view does, `width` wide,
/// for an embed or a preview: its tables, callouts and maths drawn whole
/// too. Returns them and the height they take.
#[allow(clippy::too_many_arguments)]
fn note_lines(
    c: u64,
    env: &mut Env,
    folder: &str,
    text: &str,
    anchor: Option<&str>,
    width: f32,
    sz: &Sizes,
    x: f32,
    top: f32,
    max_lines: usize,
) -> (Vec<Sub>, f32) {
    let all: Vec<&str> = text.split('\n').collect();
    let (from, to) = match anchor {
        Some(a) if a.starts_with('^') => blocks::block_lines(&all, a).unwrap_or((0, 0)),
        Some(a) => blocks::heading_section(&all, a).unwrap_or((0, all.len().saturating_sub(1))),
        None => {
            let skip = blocks::front_matter_end(|n| all.get(n).copied().unwrap_or(""), all.len())
                .map_or(0, |e| e + 1);
            (skip, all.len().saturating_sub(1))
        }
    };
    let mut section = String::new();
    for (i, l) in all.iter().enumerate().take(to + 1).skip(from) {
        if i > from {
            section.push('\n');
        }
        // A block id names the block; it is not shown.
        section.push_str(l);
    }
    let mut d = Doc::new("embed.md", "", &section);
    d.mode = Mode::Reading;
    d.cur = Pos::new(usize::MAX / 2, 0);
    let d = d;
    let k = sz.body / 16.0;
    let mut subs = Vec::new();
    let mut y = top;
    let mut prev_blank = true;
    let mut n = 0usize;
    let count = d.buf.line_count();
    while n < count && subs.len() < max_lines {
        let line = d.line(n);
        if let Some(span) = d.span(n) {
            let lay = match span.kind {
                SpanKind::Table => Some(table(c, &d, span, width, sz)),
                SpanKind::Callout => Some(callout(c, &d, span, width, sz)),
                SpanKind::Math => Some(math_block(c, &d, span, width, sz)),
                SpanKind::Embed => match blocks::embed(line) {
                    Some(e) if e.is_image() => Some(image(env, &e, folder, n, width, sz)),
                    _ => None,
                },
                SpanKind::Diagram => Some(diagram(c, &d, span, width, sz)),
                SpanKind::Front => None,
            };
            if let Some(l) = lay {
                let lh = l.h;
                subs.push(Sub {
                    text: String::new(),
                    lay: l,
                    x,
                    y,
                    w: width,
                    line: None,
                    col0: 0,
                });
                y += lh;
                prev_blank = false;
                n = span.last + 1;
                continue;
            }
        }
        let block = d.block(n);
        if block == Block::Blank || d.in_comment(n) || md::reading_empty(line) {
            if block == Block::Blank && !prev_blank {
                y += 16.0 * k;
                prev_blank = true;
            }
            n += 1;
            continue;
        }
        prev_blank = false;
        // A nested embed shows as its link: no embed inside an embed.
        let shown = if line.trim_start().starts_with("![[") {
            String::from(line.trim_start().trim_start_matches('!'))
        } else {
            String::from(line)
        };
        let l = sub(
            c,
            shown.clone(),
            block,
            Mode::Reading,
            width,
            sz.body,
            sz.code,
        );
        let lh = l.h;
        subs.push(Sub {
            text: shown,
            lay: l,
            x,
            y,
            w: width,
            line: None,
            col0: n,
        });
        y += lh;
        n += 1;
    }
    (subs, y)
}

fn note_embed(
    c: u64,
    env: &mut Env,
    e: &blocks::Embed,
    folder: &str,
    line: usize,
    width: f32,
    sz: &Sizes,
) -> Rc<Lay> {
    let k = sz.body / 16.0;
    let pad = 8.0 * k;
    let inset = 18.0 * k;
    let (subs, h, missing) = match env.note(&e.target) {
        None => (
            Vec::new(),
            pad * 2.0 + sz.body * 1.5,
            Some(String::from("Loading")),
        ),
        Some(NoteText::Missing) => {
            let mut s = String::from("\"");
            s.push_str(&e.target);
            s.push_str("\" is not created yet. Click to create it.");
            (Vec::new(), pad * 2.0 + sz.body * 1.5, Some(s))
        }
        Some(NoteText::Text(t)) => {
            let t = t.clone();
            // A whole note is headed by its name, a section is not.
            let head = if e.anchor.is_none() { 26.0 * k } else { 0.0 };
            let (subs, y) = note_lines(
                c,
                env,
                folder,
                &t,
                e.anchor.as_deref(),
                width - inset - 8.0 * k,
                sz,
                inset,
                pad + head,
                400,
            );
            (subs, y + pad, None)
        }
    };
    widget_lay(
        Widget::Note {
            subs,
            h,
            top: 0.0,
            missing,
            line,
            title: e
                .anchor
                .is_none()
                .then(|| String::from(crate::vfs::file_name(&e.target))),
        },
        h + 8.0 * k,
        sz.body,
    )
}

/// Reading view's footnotes, after the last line.
pub fn footnotes_row(cache: &mut LayCache, c: u64, doc: &Doc, v: &View) -> Rc<Lay> {
    let f = doc.footnotes();
    let mut h: u64 = 0xF007;
    for n in &f.notes {
        fnv(&mut h, n.text.as_bytes());
        fnv(&mut h, b"\n");
    }
    fnv(&mut h, &v.width.to_bits().to_le_bytes());
    if let Some(l) = cache.cached(h) {
        return l;
    }
    let sz = v.sz;
    let k = sz.body / 16.0;
    let small = (sz.body * 0.875).round();
    let top = 24.0 * k;
    let mut y = top + 18.0 * k;
    let mut subs = Vec::new();
    for (i, n) in f.notes.iter().enumerate() {
        let mut num = String::new();
        crate::text::push_num(&mut num, i + 1);
        num.push('.');
        let l = sub(
            c,
            n.text.clone(),
            Block::Para,
            Mode::Reading,
            v.width - 28.0 * k,
            small,
            sz.code,
        );
        let lh = l.h;
        subs.push((
            num,
            Sub {
                text: n.text.clone(),
                lay: l,
                x: 28.0 * k,
                y,
                w: v.width - 28.0 * k,
                line: None,
                col0: 0,
            },
        ));
        y += lh + 2.0 * k;
    }
    let lay = widget_lay(Widget::Footnotes { subs, top }, y + 24.0 * k, sz.body);
    cache.put(h, lay.clone());
    lay
}

// ---- drawing ----------------------------------------------------------------------------------

fn rgb(c: (u8, u8, u8), a: f32) -> Rgba {
    look::alpha(
        look::hex((u32::from(c.0) << 16) | (u32::from(c.1) << 8) | u32::from(c.2)),
        a,
    )
}

fn draw_sub(
    c: u64,
    s: &Sub,
    ox: f32,
    oy: f32,
    t: &Theme,
    resolved: &dyn Fn(&str) -> bool,
    env: &Env,
) {
    draw_line(
        c,
        &s.lay,
        &s.text,
        ox + s.x,
        oy + s.y,
        s.w,
        t,
        resolved,
        &LineDraw {
            opens_fence: false,
            lang: "",
            hovered: false,
            env,
        },
    );
}

#[allow(clippy::too_many_arguments)]
pub fn draw(
    c: u64,
    w: &Widget,
    ox: f32,
    oy: f32,
    width: f32,
    t: &Theme,
    resolved: &dyn Fn(&str) -> bool,
    env: &Env,
) {
    match w {
        Widget::Table {
            cells,
            cols,
            rows,
            w: tw,
            top,
            ..
        } => {
            let y0 = oy + top;
            let th: f32 = rows.iter().map(|r| r.1).sum();
            for (x, _) in cols.iter().skip(1) {
                fill(c, r(ox + x, y0, 1.0, th), t.border);
            }
            for (y, _) in rows.iter().skip(1) {
                fill(c, r(ox, oy + y, *tw, 1.0), t.border);
            }
            look::outline(c, r(ox, y0, *tw, th), 0.0, 1.0, t.border);
            for s in cells {
                draw_sub(c, s, ox, oy, t, resolved, env);
            }
        }
        Widget::Callout {
            rgb: col,
            icon,
            title,
            body,
            fold,
            h,
            top,
            title_w,
            ..
        } => {
            let k = 1.0;
            let y0 = oy + top;
            round(c, r(ox, y0, width, *h), 4.0, rgb(*col, 0.1));
            let ink = rgb(*col, 1.0);
            let cy = y0 + 12.0 + 12.0;
            callout_icon(c, *icon, ox + 24.0 + 8.0, cy, ink);
            let tx = ox + 24.0 + 22.0;
            look::text(c, title, tx, cy + 5.5, 16.0, 600, ink);
            if let Some(open) = fold {
                look::chevron(c, tx + title_w + 12.0, cy, *open, 1.0 * k, ink);
            }
            for s in body {
                draw_sub(c, s, ox, y0, t, resolved, env);
            }
        }
        Widget::Math { b, x, base, .. } => {
            draw_math(c, b, ox + x, oy + base, t.text, t);
        }
        Widget::Props { rows, add_y } => {
            look::text(c, "Properties", ox, oy + 26.0, 16.0, 600, t.text);
            for p in rows {
                let cy = oy + p.y + 15.0;
                prop_icon(c, p.kind, ox + 8.0, cy, t.muted);
                look::fit(c, &p.key, ox + 26.0, cy + 5.0, 15.0, 400, t.muted, 112.0);
                draw_value(c, p, ox + 148.0, cy, width - 148.0, t);
            }
            let ay = oy + add_y + 15.0;
            look::line(c, ox + 4.0, ay, ox + 14.0, ay, 1.4, t.muted);
            look::line(c, ox + 9.0, ay - 5.0, ox + 9.0, ay + 5.0, 1.4, t.muted);
            look::text(c, "Add property", ox + 22.0, ay + 5.0, 15.0, 400, t.muted);
        }
        Widget::Image {
            target,
            folder,
            x,
            y,
            w,
            h,
            note,
            ..
        } => match (note, env.image_ready(target, folder)) {
            (None, Some(Img::Ready { handle, .. })) => {
                let _ = krate::gfx::canvas_images::draw_round(
                    c,
                    *handle,
                    look::g(r(ox + x, oy + y, *w, *h)),
                    look::radii(4.0),
                );
            }
            (Some(n), _) => {
                look::fit(c, n, ox + x, oy + y + h * 0.65, 14.0, 400, t.faint, *w);
            }
            _ => {}
        },
        Widget::Note {
            subs,
            h,
            top,
            missing,
            title,
            ..
        } => {
            let y0 = oy + top;
            // Obsidian's embed: the note's text behind a bar in the accent,
            // a whole note headed by its name.
            fill(c, r(ox, y0, 2.0, *h), t.accent);
            if let Some(title) = title {
                look::text(c, title, ox + 18.0, y0 + 26.0, 14.0, 600, t.text);
            }
            if let Some(m) = missing {
                look::fit(
                    c,
                    m,
                    ox + 18.0,
                    y0 + h * 0.6,
                    15.0,
                    400,
                    t.faint,
                    width - 24.0,
                );
            }
            for s in subs {
                draw_sub(c, s, ox, y0, t, resolved, env);
            }
        }
        Widget::Diagram { d, x, y, .. } => {
            let (bx, by) = (ox + x, oy + y);
            let size = 14.0;
            let edge_ink = t.muted;
            for e in &d.edges {
                let (x0, y0, x1, y1) = (bx + e.x0, by + e.y0, bx + e.x1, by + e.y1);
                let w = if e.thick { 2.6 } else { 1.4 };
                if e.dotted {
                    let (dx, dy) = (x1 - x0, y1 - y0);
                    let len = (dx * dx + dy * dy).sqrt().max(1.0);
                    let steps = (len / 6.0) as usize;
                    for i in (0..steps).step_by(2) {
                        let a = i as f32 / steps as f32;
                        let b = ((i + 1) as f32 / steps as f32).min(1.0);
                        look::line(
                            c,
                            x0 + dx * a,
                            y0 + dy * a,
                            x0 + dx * b,
                            y0 + dy * b,
                            w,
                            edge_ink,
                        );
                    }
                } else {
                    look::line(c, x0, y0, x1, y1, w, edge_ink);
                }
                if e.arrow {
                    let (dx, dy) = (x1 - x0, y1 - y0);
                    let len = (dx * dx + dy * dy).sqrt().max(1.0);
                    let (ux, uy) = (dx / len, dy / len);
                    let (hx, hy) = (x1 - ux * 8.0, y1 - uy * 8.0);
                    look::line(c, x1, y1, hx - uy * 4.5, hy + ux * 4.5, 1.6, edge_ink);
                    look::line(c, x1, y1, hx + uy * 4.5, hy - ux * 4.5, 1.6, edge_ink);
                }
                if !e.label.is_empty() {
                    let lw = look::width(c, &e.label, 12.0, 400);
                    let (mx, my) = ((x0 + x1) * 0.5, (y0 + y1) * 0.5);
                    round(
                        c,
                        r(mx - lw * 0.5 - 4.0, my - 9.0, lw + 8.0, 18.0),
                        3.0,
                        t.code_bg,
                    );
                    look::text(c, &e.label, mx - lw * 0.5, my + 4.5, 12.0, 400, t.text);
                }
            }
            for n in &d.nodes {
                let rc = r(bx + n.x, by + n.y, n.w, n.h);
                let fillc = t.code_bg;
                let line = t.muted;
                match n.shape {
                    crate::mermaid::Shape::Circle => {
                        let cx = rc.x + rc.w * 0.5;
                        let cy = rc.y + rc.h * 0.5;
                        let p = krate::gfx::types::Point { x: cx, y: cy };
                        let _ = canvas2d::fill_circle(c, p, rc.w * 0.5, fillc);
                        let _ = canvas2d::stroke_circle(c, p, rc.w * 0.5, 1.2, line);
                    }
                    crate::mermaid::Shape::Diamond => {
                        let (cx, cy) = (rc.x + rc.w * 0.5, rc.y + rc.h * 0.5);
                        let steps = (rc.h * 0.5) as usize;
                        for i in 0..steps {
                            let f = i as f32 / steps.max(1) as f32;
                            let half = rc.w * 0.5 * (1.0 - f);
                            fill(
                                c,
                                r(cx - half, cy - rc.h * 0.5 * f - 1.0, half * 2.0, 1.0),
                                fillc,
                            );
                            fill(c, r(cx - half, cy + rc.h * 0.5 * f, half * 2.0, 1.0), fillc);
                        }
                        look::line(c, cx, rc.y, rc.right(), cy, 1.2, line);
                        look::line(c, rc.right(), cy, cx, rc.bottom(), 1.2, line);
                        look::line(c, cx, rc.bottom(), rc.x, cy, 1.2, line);
                        look::line(c, rc.x, cy, cx, rc.y, 1.2, line);
                    }
                    shape => {
                        let rad = match shape {
                            crate::mermaid::Shape::Stadium => rc.h * 0.5,
                            crate::mermaid::Shape::Round => 8.0,
                            _ => 3.0,
                        };
                        round(c, rc, rad, fillc);
                        look::outline(c, rc, rad, 1.2, line);
                    }
                }
                let lw = look::width(c, &n.label, size, 400);
                look::text(
                    c,
                    &n.label,
                    rc.x + (rc.w - lw) * 0.5,
                    rc.y + rc.h * 0.5 + size * 0.36,
                    size,
                    400,
                    t.text,
                );
            }
        }
        Widget::Footnotes { subs, top } => {
            fill(c, r(ox, oy + top, width, 1.0), t.border);
            for (num, s) in subs {
                let base = oy + s.y + s.lay.rows.first().map_or(16.0, |rl| rl.base);
                look::text(c, num, ox + 6.0, base, 14.0, 400, t.muted);
                draw_sub(c, s, ox, oy, t, resolved, env);
                let end = ox + s.x + s.lay.end_x() + 6.0;
                let last_base = oy + s.y + s.lay.rows.last().map_or(16.0, |rl| rl.base);
                // The way back to the reference: a hooked arrow.
                let (ax, ay) = (end + 2.0, last_base - 5.0);
                look::line(c, ax + 9.0, ay - 6.0, ax + 9.0, ay, 1.2, t.faint);
                look::line(c, ax + 9.0, ay, ax + 1.0, ay, 1.2, t.faint);
                look::line(c, ax + 1.0, ay, ax + 4.0, ay - 3.0, 1.2, t.faint);
                look::line(c, ax + 1.0, ay, ax + 4.0, ay + 3.0, 1.2, t.faint);
            }
        }
    }
}

fn draw_value(c: u64, p: &PropRow, x: f32, cy: f32, room: f32, t: &Theme) {
    let base = cy + 5.0;
    match (&p.value, p.kind) {
        (Value::Bool(b), _) => {
            let s = 16.0;
            let bx = x;
            let by = cy - s * 0.5;
            if *b {
                round(c, r(bx, by, s, s), 4.0, t.accent);
                look::line(c, bx + 4.0, by + 8.5, bx + 7.0, by + 11.5, 1.8, t.on_accent);
                look::line(
                    c,
                    bx + 7.0,
                    by + 11.5,
                    bx + 12.0,
                    by + 5.0,
                    1.8,
                    t.on_accent,
                );
            } else {
                look::outline(c, r(bx, by, s, s), 4.0, 1.2, t.muted);
            }
        }
        (Value::List(items), Kind::Tags | Kind::Aliases | Kind::List) => {
            let mut px = x;
            for it in items {
                let label = if p.kind == Kind::Tags {
                    it.trim_start_matches('#')
                } else {
                    it.as_str()
                };
                let w = look::width(c, label, 15.0, 400);
                if px + w + 30.0 > x + room {
                    break;
                }
                if p.kind == Kind::Tags {
                    round(c, r(px, cy - 12.0, w + 34.0, 24.0), 12.0, t.tag_bg);
                    look::text(c, label, px + 10.0, base, 15.0, 400, t.link);
                    look::text(c, "×", px + w + 14.0, base, 15.0, 400, t.link);
                } else {
                    // Aliases and lists are plain words with a cross, no pill.
                    look::text(c, label, px, base, 15.0, 400, t.text);
                    look::text(c, "×", px + w + 4.0, base, 15.0, 400, t.muted);
                }
                px += w + 42.0;
            }
        }
        (Value::Date(d), _) => {
            prop_icon(c, Kind::Date, x + 6.0, cy, t.muted);
            look::text(c, &props::format_date(d), x + 18.0, base, 15.0, 400, t.text);
        }
        (v, _) => {
            look::fit(c, &v.shown(), x, base, 15.0, 400, t.text, room);
        }
    }
}

/// Stroke a Lucide icon at `size` px centred on (x, y): its 2-unit lines
/// scaled, round-ended (`look::line` draws round dots).
pub fn lucide(c: u64, elements: &str, x: f32, y: f32, size: f32, col: Rgba) {
    let w = (2.0 * size / 24.0).max(1.0);
    for line in crate::svg::icon_lines(elements, x, y, size) {
        if line.len() == 1
            || line
                .windows(2)
                .all(|p| (p[0].0 - p[1].0).abs() + (p[0].1 - p[1].1).abs() < 0.05)
        {
            if let Some(p) = line.first() {
                let _ = canvas2d::fill_circle(
                    c,
                    krate::gfx::types::Point { x: p.0, y: p.1 },
                    w * 0.5,
                    col,
                );
            }
            continue;
        }
        for seg in line.windows(2) {
            look::line(c, seg[0].0, seg[0].1, seg[1].0, seg[1].1, w, col);
        }
    }
}

/// The Lucide icon Obsidian shows for a property's type.
fn prop_icon(c: u64, kind: Kind, x: f32, y: f32, col: Rgba) {
    let el = match kind {
        Kind::Text => crate::lucide::TEXT,
        Kind::Tags => crate::lucide::TAGS,
        Kind::Aliases => crate::lucide::FORWARD,
        Kind::Number => crate::lucide::BINARY,
        Kind::Checkbox => crate::lucide::SQUARE_CHECK,
        Kind::Date => crate::lucide::CALENDAR,
        Kind::DateTime => crate::lucide::CLOCK,
        Kind::List => crate::lucide::LIST,
    };
    lucide(c, el, x, y, 16.0, col);
}

/// A callout type's Lucide icon, 16 px, in its colour.
fn callout_icon(c: u64, icon: CalloutIcon, x: f32, y: f32, col: Rgba) {
    let el = match icon {
        CalloutIcon::Pencil => crate::lucide::PENCIL,
        CalloutIcon::Clipboard => crate::lucide::CLIPBOARD_LIST,
        CalloutIcon::Info => crate::lucide::INFO,
        CalloutIcon::Check => crate::lucide::CIRCLE_CHECK,
        CalloutIcon::Flame => crate::lucide::FLAME,
        CalloutIcon::CheckCircle => crate::lucide::CHECK,
        CalloutIcon::Help => crate::lucide::CIRCLE_HELP,
        CalloutIcon::Warning => crate::lucide::TRIANGLE_ALERT,
        CalloutIcon::Cross => crate::lucide::X,
        CalloutIcon::Zap => crate::lucide::ZAP,
        CalloutIcon::Bug => crate::lucide::BUG,
        CalloutIcon::List => crate::lucide::LIST,
        CalloutIcon::Quote => crate::lucide::QUOTE,
    };
    lucide(c, el, x, y, 16.0, col);
}

// ---- presses ------------------------------------------------------------------------------------

fn sub_press(c: u64, s: &Sub, lx: f32, ly: f32) -> Option<Act> {
    let x = lx - s.x;
    let y = ly - s.y;
    if y < 0.0 || y > s.lay.h {
        return None;
    }
    let row = s.lay.row_at(y);
    if let (Deco::Task(_), Some(n)) = (s.lay.deco, s.line) {
        if x < s.lay.x0 && x >= s.lay.marker - 12.0 && row == 0 {
            return Some(Act::Task(n));
        }
    }
    if let Some(run) = s.lay.run_at(row, x) {
        let a = match run.target {
            Some(md::Target::Wiki(a, b)) => {
                let rest = s.text.get(b..).unwrap_or("");
                let end = rest.find(['|', ']']).unwrap_or(rest.len());
                let mut full = String::from(s.text.get(a..b).unwrap_or(""));
                full.push_str(rest.get(..end).unwrap_or(""));
                Some(Act::Follow(full))
            }
            Some(md::Target::Url(a, b)) => {
                Some(Act::Url(String::from(s.text.get(a..b).unwrap_or(""))))
            }
            Some(md::Target::Tag(a, b)) => {
                Some(Act::Tag(String::from(s.text.get(a + 1..b).unwrap_or(""))))
            }
            _ => None,
        };
        if a.is_some() {
            return a;
        }
    }
    let n = s.line?;
    let col = s.lay.col_at(c, &s.text, row, x);
    Some(Act::Place(Pos::new(n, s.col0 + col)))
}

/// What a press at (lx, ly) inside a block lands on.
pub fn press(c: u64, w: &Widget, lx: f32, ly: f32, _width: f32, line: usize) -> Option<Act> {
    match w {
        Widget::Table { cells, .. } => {
            for s in cells {
                if lx >= s.x - 8.0
                    && lx <= s.x + s.w + 8.0
                    && ly >= s.y - 3.0
                    && ly <= s.y + s.lay.h.max(24.0) + 3.0
                {
                    return sub_press(c, s, lx, ly)
                        .or(Some(Act::Place(Pos::new(s.line.unwrap_or(line), s.col0))));
                }
            }
            Some(Act::Place(Pos::new(line, 0)))
        }
        Widget::Callout {
            body,
            fold,
            head,
            top,
            title_w,
            title_col,
            ..
        } => {
            let ly = ly - top;
            if ly < 36.0 {
                if fold.is_some() && lx < 24.0 + 22.0 + title_w + 30.0 {
                    return Some(Act::Callout(*head));
                }
                return Some(Act::Place(Pos::new(*head, *title_col)));
            }
            for s in body {
                if ly >= s.y && ly <= s.y + s.lay.h {
                    return sub_press(c, s, lx, ly);
                }
            }
            Some(Act::Place(Pos::new(*head, *title_col)))
        }
        Widget::Math { line, .. } => Some(Act::Place(Pos::new(line + 1, 0))),
        Widget::Diagram { line, .. } => Some(Act::Place(Pos::new(line + 1, 0))),
        Widget::Props { rows, add_y } => {
            if ly >= *add_y && ly < add_y + 30.0 && lx < 140.0 {
                return Some(Act::AddProperty);
            }
            for p in rows {
                if ly >= p.y && ly < p.y + 30.0 {
                    if matches!(p.value, Value::Bool(_)) && (140.0..176.0).contains(&lx) {
                        return Some(Act::Property(p.line));
                    }
                    return Some(Act::Place(Pos::new(p.line, p.value_col)));
                }
            }
            None
        }
        Widget::Image { line, .. } => Some(Act::Place(Pos::new(*line, 0))),
        Widget::Note {
            subs,
            line,
            missing,
            ..
        } => {
            for s in subs {
                if ly >= s.y && ly <= s.y + s.lay.h {
                    if let Some(a @ (Act::Follow(_) | Act::Url(_) | Act::Tag(_))) =
                        sub_press(c, s, lx, ly)
                    {
                        return Some(a);
                    }
                }
            }
            if missing.is_some() {
                return Some(Act::Place(Pos::new(*line, 0)));
            }
            Some(Act::Place(Pos::new(*line, 0)))
        }
        Widget::Footnotes { subs, .. } => {
            for (_, s) in subs {
                if ly >= s.y && ly <= s.y + s.lay.h {
                    if let Some(a @ (Act::Follow(_) | Act::Url(_) | Act::Tag(_))) =
                        sub_press(c, s, lx, ly)
                    {
                        return Some(a);
                    }
                }
            }
            None
        }
    }
}

// ---- the find bar -------------------------------------------------------------------------------

fn input_box(
    c: u64,
    input: &crate::app::Input,
    rc: Rect,
    placeholder: &str,
    focused: bool,
    caret: bool,
    t: &Theme,
) {
    round(c, rc, 5.0, t.field);
    look::outline(c, rc, 5.0, 1.0, if focused { t.accent } else { t.border });
    let base = rc.y + rc.h * 0.5 + 5.0;
    let x = rc.x + 10.0;
    if input.text.is_empty() {
        look::text(c, placeholder, x, base, 14.0, 400, t.faint);
    } else {
        if let Some((a, b)) = input.selection() {
            let xa = look::width(c, input.text.get(..a).unwrap_or(""), 14.0, 400);
            let xb = look::width(c, input.text.get(..b).unwrap_or(""), 14.0, 400);
            fill(c, r(x + xa, rc.y + 5.0, xb - xa, rc.h - 10.0), t.sel);
        }
        look::fit(c, &input.text, x, base, 14.0, 400, t.text, rc.w - 20.0);
    }
    if focused && caret {
        let cx = x + look::width(c, input.text.get(..input.caret).unwrap_or(""), 14.0, 400);
        fill(c, r(cx, rc.y + 6.0, 1.5, rc.h - 12.0), t.text);
    }
}

fn small_button(c: u64, rc: Rect, label: &str, t: &Theme) {
    round(c, rc, 5.0, t.hover);
    let w = look::width(c, label, 13.0, 400);
    look::text(
        c,
        label,
        rc.x + (rc.w - w) * 0.5,
        rc.y + rc.h * 0.5 + 4.5,
        13.0,
        400,
        t.text,
    );
}

/// The find (and replace) bar along the top of the note, as Obsidian
/// draws its in-note search.
pub fn draw_find_bar(
    c: u64,
    doc: &Doc,
    v: &View,
    t: &Theme,
    shown: &mut Shown,
    caret_on: bool,
    said: &mut krate::layout::Said,
) {
    let Some(f) = doc.find.as_ref() else {
        return;
    };
    let row_h = 40.0;
    let h = if f.replacing { row_h * 2.0 } else { row_h } + 8.0;
    let bar = r(v.area.x, v.area.y, v.area.w, h);
    fill(c, bar, t.side);
    fill(c, r(bar.x, bar.bottom() - 1.0, bar.w, 1.0), t.border);
    shown.find.push((FindHit::Bar, bar));
    let pad = 12.0;
    let right_w = 200.0;
    let field_w = (bar.w - pad * 2.0 - right_w).max(120.0);
    let q = r(bar.x + pad, bar.y + 6.0, field_w, 30.0);
    input_box(
        c,
        &f.query,
        q,
        "Find...",
        f.focused && !f.in_replace,
        caret_on,
        t,
    );
    said.text(
        q,
        if f.query.text.is_empty() {
            "Find"
        } else {
            &f.query.text
        },
    );
    shown.find.push((FindHit::Query, q));
    // Match case, whole word, regular expression: toggles in the field.
    let mut tx = q.right() - 4.0;
    for (hit, on, glyph) in [
        (FindHit::Regex, f.regex, ".*"),
        (FindHit::Word, f.word, "ab"),
        (FindHit::Case, f.case, "Aa"),
    ] {
        let b = r(tx - 26.0, q.y + 4.0, 24.0, q.h - 8.0);
        if on {
            round(c, b, 4.0, look::alpha(t.accent, 0.35));
        }
        let gw = look::width(c, glyph, 12.0, 600);
        let ink = if on { t.text } else { t.muted };
        look::text(
            c,
            glyph,
            b.x + (b.w - gw) * 0.5,
            b.y + b.h * 0.5 + 4.0,
            12.0,
            600,
            ink,
        );
        if hit == FindHit::Word {
            fill(c, r(b.x + 5.0, b.bottom() - 5.0, b.w - 10.0, 1.0), ink);
        }
        said.button(
            b,
            match hit {
                FindHit::Case => "Match case",
                FindHit::Word => "Match whole word",
                _ => "Use regular expression",
            },
        );
        shown.find.push((hit, b));
        tx -= 26.0;
    }
    let label = f.label();
    let lw = look::width(c, &label, 13.0, 400);
    look::text(c, &label, tx - lw - 8.0, q.y + 20.0, 13.0, 400, t.muted);
    let mut x = q.right() + 8.0;
    for hit in [
        FindHit::Prev,
        FindHit::Next,
        FindHit::ToggleReplace,
        FindHit::Close,
    ] {
        let b = r(x, q.y, 30.0, 30.0);
        if hit == FindHit::ToggleReplace && f.replacing {
            round(c, b, 5.0, t.hover);
        }
        let (cx, cy) = (b.x + 15.0, b.y + 15.0);
        let ink = t.muted;
        match hit {
            FindHit::Prev | FindHit::Next => {
                let d = if hit == FindHit::Prev { -1.0 } else { 1.0 };
                look::line(c, cx, cy - 6.0 * d, cx, cy + 6.0 * d, 1.5, ink);
                look::line(c, cx, cy + 6.0 * d, cx - 4.5, cy + 1.5 * d, 1.5, ink);
                look::line(c, cx, cy + 6.0 * d, cx + 4.5, cy + 1.5 * d, 1.5, ink);
            }
            FindHit::ToggleReplace => {
                // Two arrows, one each way: replace.
                look::line(c, cx - 6.0, cy - 3.0, cx + 6.0, cy - 3.0, 1.5, ink);
                look::line(c, cx + 6.0, cy - 3.0, cx + 3.0, cy - 6.0, 1.5, ink);
                look::line(c, cx + 6.0, cy + 3.0, cx - 6.0, cy + 3.0, 1.5, ink);
                look::line(c, cx - 6.0, cy + 3.0, cx - 3.0, cy + 6.0, 1.5, ink);
            }
            _ => {
                look::line(c, cx - 5.0, cy - 5.0, cx + 5.0, cy + 5.0, 1.5, ink);
                look::line(c, cx + 5.0, cy - 5.0, cx - 5.0, cy + 5.0, 1.5, ink);
            }
        }
        said.button(
            b,
            match hit {
                FindHit::Prev => "Previous match",
                FindHit::Next => "Next match",
                FindHit::ToggleReplace => "Replace",
                _ => "Close find",
            },
        );
        shown.find.push((hit, b));
        x += 34.0;
    }
    if f.replacing {
        let rr = r(bar.x + pad, q.bottom() + 10.0, field_w, 30.0);
        input_box(
            c,
            &f.replace,
            rr,
            "Replace...",
            f.focused && f.in_replace,
            caret_on,
            t,
        );
        shown.find.push((FindHit::Replace, rr));
        said.text(rr, "Replace with");
        let b1 = r(rr.right() + 8.0, rr.y, 80.0, 30.0);
        small_button(c, b1, "Replace", t);
        said.button(b1, "Replace");
        shown.find.push((FindHit::ReplaceOne, b1));
        let b2 = r(b1.right() + 6.0, rr.y, 96.0, 30.0);
        small_button(c, b2, "Replace all", t);
        said.button(b2, "Replace all");
        shown.find.push((FindHit::ReplaceAll, b2));
    }
}

// ---- page preview -------------------------------------------------------------------------------

/// The note a link names, shown over the page while the pointer rests on
/// the link with Cmd held (Obsidian's page preview).
pub struct Preview {
    pub target: String,
    pub x: f32,
    pub y: f32,
    pub rect: Option<Rect>,
}

/// The pointer moved (with Cmd held or not). True when the view should be
/// drawn again.
pub fn hover(
    cache: &mut LayCache,
    c: u64,
    doc: Option<&Doc>,
    shown: &Shown,
    px: f32,
    py: f32,
    cmd: bool,
) -> bool {
    let row_before = cache.pointer.and_then(|(_, y)| {
        shown
            .rows
            .iter()
            .position(|(_, ry, l)| y >= *ry && y < ry + l.h)
    });
    cache.pointer = Some((px, py));
    let row_now = shown
        .rows
        .iter()
        .position(|(_, ry, l)| py >= *ry && py < ry + l.h);
    let mut changed = row_before != row_now;
    let over_popup = cache
        .preview
        .as_ref()
        .and_then(|p| p.rect)
        .is_some_and(|rc| rc.contains(px, py));
    if over_popup {
        return changed;
    }
    // Editing view previews with Cmd held; reading view on plain hover
    // (Obsidian's defaults).
    let reading = doc.is_some_and(|d| d.mode == Mode::Reading);
    let cmd = cmd || reading;
    let link = match (doc, cmd) {
        (Some(d), true) => editor::link_at(c, d, shown, px, py),
        _ => None,
    };
    match link {
        Some((target, x, y)) => {
            if cache.preview.as_ref().is_none_or(|p| p.target != target) {
                cache.preview = Some(Preview {
                    target,
                    x,
                    y,
                    rect: None,
                });
                changed = true;
            }
        }
        None => {
            if cache.preview.is_some() && !cmd {
                cache.preview = None;
                changed = true;
            } else if cache.preview.is_some() {
                // Cmd still held but off the link and off the popover.
                cache.preview = None;
                changed = true;
            }
        }
    }
    changed
}

/// Draw the page preview, if one is up, inside `bounds`.
pub fn draw_preview(cache: &mut LayCache, c: u64, t: &Theme, sz: &Sizes, bounds: Rect) {
    let Some((target, x, y)) = cache.preview.as_ref().map(|p| (p.target.clone(), p.x, p.y)) else {
        return;
    };
    let (name, anchor) = match target.find('#') {
        Some(i) => (
            String::from(target.get(..i).unwrap_or("")),
            Some(String::from(target.get(i + 1..).unwrap_or(""))),
        ),
        None => (target.clone(), None),
    };
    let w = 460.0f32.min(bounds.w - 20.0);
    let h = 360.0f32.min(bounds.h * 0.6);
    let px = x.min(bounds.right() - w - 10.0).max(bounds.x + 10.0);
    let py = if y + h + 10.0 > bounds.bottom() {
        (y - h - 40.0).max(bounds.y + 10.0)
    } else {
        y + 6.0
    };
    let rc = r(px, py, w, h);
    if let Some(p) = cache.preview.as_mut() {
        p.rect = Some(rc);
    }
    let _ = canvas2d::drop_shadow_round_rect(
        c,
        look::g(r(rc.x, rc.y + 4.0, rc.w, rc.h)),
        look::radii(8.0),
        18.0,
        look::alpha(look::hex(0x000000), 0.45),
    );
    round(c, rc, 8.0, t.modal);
    look::outline(c, rc, 8.0, 1.0, t.border);
    let inner = rc.w - 48.0;
    let text = match cache.env.note(&name) {
        None => {
            look::text(c, "Loading", rc.x + 24.0, rc.y + 40.0, 15.0, 400, t.faint);
            return;
        }
        Some(NoteText::Missing) => {
            look::text(
                c,
                "This note does not exist yet.",
                rc.x + 24.0,
                rc.y + 40.0,
                15.0,
                400,
                t.faint,
            );
            return;
        }
        Some(NoteText::Text(t)) => t.clone(),
    };
    let _ = canvas2d::set_clip(c, rc.x + 1.0, rc.y + 1.0, rc.w - 2.0, rc.h - 2.0);
    let title = crate::vfs::file_name(&name);
    look::text(
        c,
        title,
        rc.x + 24.0,
        rc.y + 44.0,
        sz.body * 1.618,
        700,
        t.text,
    );
    let (subs, _) = note_lines(
        c,
        &mut cache.env,
        "",
        &text,
        anchor.as_deref(),
        inner,
        sz,
        24.0,
        64.0,
        40,
    );
    let env = &cache.env;
    for s in &subs {
        if s.y > rc.h {
            break;
        }
        draw_sub(c, s, rc.x, rc.y, t, &|_| true, env);
    }
    let _ = canvas2d::clear_clip(c);
}
