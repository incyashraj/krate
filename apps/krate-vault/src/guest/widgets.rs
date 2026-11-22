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
    Ready {
        handle: u64,
        w: u32,
        h: u32,
        /// An animation: each frame kept on the canvas, and its time (ms).
        frames: Vec<(u64, u32)>,
        /// An SVG's text, in the picture's own pixels.
        texts: Vec<crate::svg::Text>,
        /// The width of the pixels kept (the shown width is `w`).
        px_w: u32,
    },
    Failed(String),
}

/// A picture's frames kept on the canvas (or why it could not be).
fn keep(c: u64, pic: crate::image::Picture) -> Img {
    let Ok(handle) = krate::gfx::canvas_images::keep(c, pic.width, pic.height, &pic.rgba) else {
        return Img::Failed(String::from("Krate could not keep the picture"));
    };
    let mut frames = Vec::new();
    for (rgba, delay) in &pic.anim {
        match krate::gfx::canvas_images::keep(c, pic.width, pic.height, rgba) {
            Ok(h) => frames.push((h, (*delay).max(20))),
            Err(_) => break,
        }
    }
    Img::Ready {
        handle,
        w: pic.logical.0,
        h: pic.logical.1,
        frames,
        texts: pic.texts,
        px_w: pic.width,
    }
}

/// Embedded notes and pictures, read from the vault between frames.
#[derive(Default)]
pub struct Env {
    notes: BTreeMap<String, NoteText>,
    images: BTreeMap<String, Img>,
    want_notes: BTreeSet<String>,
    want_images: BTreeSet<String>,
    generation: u64,
    /// Pictures being fetched from the web: their key and request.
    fetching: Vec<(String, u64)>,
    /// The clock animated pictures play by (ms), set each frame.
    pub now: u64,
    /// When the next frame of a playing picture is due (ms on `now`'s
    /// clock), set while drawing.
    pub due: core::cell::Cell<Option<u64>>,
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

/// How long until a playing picture needs its next frame (ms), so the app
/// wakes for it; `None` when nothing plays. Cleared each time it is read.
pub fn anim_due(cache: &LayCache) -> Option<u64> {
    let due = cache.env.due.take()?;
    let now = krate::time::monotonic_nanos() / 1_000_000;
    Some(due.saturating_sub(now).max(16))
}

/// Read what the last frame asked for. True when something arrived (the
/// note should be drawn again).
pub fn fetch(cache: &mut LayCache, app: &mut crate::app::App, c: u64) -> bool {
    cache.env.now = krate::time::monotonic_nanos() / 1_000_000;
    // Spelling choices made in the menu: teach the checker, or ignore.
    let pending: Vec<crate::spell::SpellAct> = app
        .doc_mut()
        .map(|d| core::mem::take(&mut d.spell_pending))
        .unwrap_or_default();
    if !pending.is_empty() {
        for a in pending {
            match a {
                crate::spell::SpellAct::Learn(w) => krate::ui::spelling::learn(&w),
                crate::spell::SpellAct::Ignore(w) if !w.is_empty() => cache.spell_ignored.push(w),
                _ => {}
            }
        }
        cache.respell();
        return true;
    }
    let web = poll_web(&mut cache.env, c);
    if web {
        cache.clear();
    }
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
        if target.starts_with("http://") || target.starts_with("https://") {
            // A picture from the web: asked for now, kept when it comes.
            let req = krate::net::Request {
                method: krate::net::HttpMethod::Get,
                url: String::from(target),
                // Some picture hosts refuse a request that names no client.
                headers: alloc::vec![krate::net::Header {
                    name: String::from("User-Agent"),
                    value: String::from("Vault/0.1 (a Krate notes app)"),
                }],
                body: Vec::new(),
                timeout_millis: Some(20_000),
            };
            match krate::net::begin(req) {
                Ok(h) => env.fetching.push((key.clone(), h)),
                Err(_) => {
                    env.images.insert(
                        key.clone(),
                        Img::Failed(String::from("Krate did not let Vault fetch this picture")),
                    );
                    got = true;
                }
            }
            continue;
        }
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

/// Pictures from the web that arrived since the last frame.
fn poll_web(env: &mut Env, c: u64) -> bool {
    if env.fetching.is_empty() {
        return false;
    }
    let mut got = false;
    let mut still = Vec::new();
    for (key, h) in core::mem::take(&mut env.fetching) {
        match krate::net::poll(h) {
            krate::net::FetchStatus::Pending => still.push((key, h)),
            krate::net::FetchStatus::Ready(res) if (200..300).contains(&res.status) => {
                let img = match crate::image::decode(&res.body) {
                    Ok(pic) => keep(c, pic),
                    Err(why) => Img::Failed(why),
                };
                env.images.insert(key, img);
                got = true;
            }
            krate::net::FetchStatus::Ready(res) => {
                env.images.insert(
                    key,
                    Img::Failed(alloc::format!(
                        "The picture could not be fetched (HTTP {})",
                        res.status
                    )),
                );
                got = true;
            }
            _ => {
                env.images.insert(
                    key,
                    Img::Failed(String::from("The picture could not be fetched")),
                );
                got = true;
            }
        }
    }
    env.fetching = still;
    got
}

fn load_image(v: &mut crate::vault::Vault, target: &str, folder: &str, c: u64) -> Img {
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
            Ok(pic) => keep(c, pic),
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
    pub value: Value,
    /// A list's chips: where each starts and how wide it is (from the
    /// value column), the cross at its right end.
    pub chips: Vec<(f32, f32)>,
    /// Being edited: which field, and what it holds.
    pub edit: Option<(crate::propedit::Field, crate::app::Input)>,
}

/// The date picker's geometry: 7 columns of 32 px under a 40 px header.
pub const PICK_W: f32 = 7.0 * 32.0 + 16.0;
pub const PICK_H: f32 = 40.0 + 6.0 * 28.0 + 12.0;

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
        /// The date picker, open under a row: its line and month.
        picker: Option<(usize, i32, u32)>,
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
        d: crate::charts::Chart,
        x: f32,
        y: f32,
        line: usize,
    },
}

impl Widget {
    /// Tell the screen reader (and check-app) the parts a person presses
    /// one by one: each table cell, each property's name and value. True
    /// when it did (else the whole block is said as one).
    pub fn say_parts(&self, ox: f32, oy: f32, said: &mut krate::layout::Said) -> bool {
        match self {
            Widget::Table { cells, .. } => {
                for c in cells {
                    let t = md::plain(&c.text, 0);
                    if !t.is_empty() {
                        said.text(r(ox + c.x, oy + c.y, c.w.max(20.0), c.lay.h.max(20.0)), &t);
                    }
                }
                true
            }
            Widget::Props { rows, add_y, .. } => {
                said.heading(r(ox, oy + 8.0, 140.0, 24.0), "Properties");
                for p in rows {
                    said.text(r(ox + 22.0, oy + p.y, 120.0, 30.0), &p.key);
                    let v = p.value.shown();
                    said.text(
                        r(ox + 148.0, oy + p.y, 300.0, 30.0),
                        if v.is_empty() { "Empty" } else { &v },
                    );
                }
                said.button(r(ox, oy + add_y, 140.0, 30.0), "Add property");
                true
            }
            _ => false,
        }
    }

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
                for i in &d.items {
                    if let crate::charts::Item::Text { s, .. } = i {
                        add(s);
                    }
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
    if span.kind == SpanKind::Front {
        if let Some(e) = doc.prop_edit.as_ref() {
            let st = alloc::format!("{:?}", e);
            fnv(&mut h, st.as_bytes());
        }
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
    let measure = |s: &str, sz: f32| look::width(c, s, sz, 400);
    match crate::charts::chart(&src, &measure, size, width) {
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

fn properties(c: u64, doc: &Doc, span: Span, _width: f32, sz: &Sizes) -> Rc<Lay> {
    let k = sz.body / 16.0;
    let lines: Vec<&str> = (span.first + 1..span.last).map(|n| doc.line(n)).collect();
    let parsed = props::parse(&lines, span.first + 1);
    let mut rows = Vec::new();
    let mut y = 40.0 * k;
    let editing = doc.prop_edit.as_ref();
    for p in parsed {
        let mut chips = Vec::new();
        if let Value::List(items) = &p.value {
            let mut x = 0.0;
            for it in items {
                let label = if p.kind == Kind::Tags {
                    it.trim_start_matches('#')
                } else {
                    it.as_str()
                };
                let w = look::width(c, label, 15.0, 400);
                let cw = if p.kind == Kind::Tags {
                    w + 34.0
                } else {
                    w + 20.0
                };
                chips.push((x, cw));
                x += cw + 8.0;
            }
        }
        let edit = editing
            .filter(|e| e.line == p.line)
            .map(|e| (e.field, e.input.clone()));
        rows.push(PropRow {
            key: p.key,
            kind: p.kind,
            y,
            line: p.line,
            value: p.value,
            chips,
            edit,
        });
        y += 30.0 * k;
    }
    let add_y = y + 4.0 * k;
    let h = add_y + 30.0 * k + 22.0 * k;
    let picker = editing.and_then(|e| e.picker.map(|(yy, m)| (e.line, yy, m)));
    widget_lay(
        Widget::Props {
            rows,
            add_y,
            picker,
        },
        h,
        sz.body,
    )
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
        Widget::Props { rows, add_y, .. } => {
            look::text(c, "Properties", ox, oy + 26.0, 16.0, 600, t.text);
            for p in rows {
                let cy = oy + p.y + 15.0;
                prop_icon(c, p.kind, ox + 8.0, cy, t.muted);
                match &p.edit {
                    Some((crate::propedit::Field::Key, input)) => {
                        edit_box(c, input, r(ox + 22.0, cy - 13.0, 120.0, 26.0), t);
                    }
                    _ => {
                        look::fit(c, &p.key, ox + 26.0, cy + 5.0, 15.0, 400, t.muted, 112.0);
                    }
                }
                match &p.edit {
                    Some((crate::propedit::Field::Value, input)) => {
                        edit_box(
                            c,
                            input,
                            r(ox + 144.0, cy - 13.0, (width - 150.0).min(360.0), 26.0),
                            t,
                        );
                    }
                    Some((crate::propedit::Field::NewItem, input)) => {
                        draw_value(c, p, ox + 148.0, cy, width - 148.0, t);
                        let after = p.chips.last().map_or(0.0, |(x, w)| x + w + 8.0);
                        edit_box(c, input, r(ox + 148.0 + after, cy - 13.0, 140.0, 26.0), t);
                    }
                    _ => draw_value(c, p, ox + 148.0, cy, width - 148.0, t),
                }
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
            (
                None,
                Some(Img::Ready {
                    handle,
                    frames,
                    texts,
                    px_w,
                    ..
                }),
            ) => {
                // An animation plays by the clock, looping.
                let mut shown = *handle;
                if !frames.is_empty() {
                    let total: u64 = frames.iter().map(|f| u64::from(f.1)).sum::<u64>().max(1);
                    let mut t = env.now % total;
                    for (h, d) in frames {
                        if t < u64::from(*d) {
                            shown = *h;
                            let next = env.now + (u64::from(*d) - t);
                            let due = env.due.get().map_or(next, |x| x.min(next));
                            env.due.set(Some(due));
                            break;
                        }
                        t -= u64::from(*d);
                    }
                }
                let _ = krate::gfx::canvas_images::draw_round(
                    c,
                    shown,
                    look::g(r(ox + x, oy + y, *w, *h)),
                    look::radii(4.0),
                );
                // An SVG's text, drawn at the picture's scale.
                let k = *w / (*px_w as f32).max(1.0);
                for tx in texts {
                    let size = tx.size * k;
                    let st = krate::gfx::types::TextStyle {
                        weight: if tx.bold { 700 } else { 400 },
                        italic: false,
                        letter_spacing: 0.0,
                        family: if tx.serif {
                            krate::gfx::types::FontFamily::Serif
                        } else {
                            krate::gfx::types::FontFamily::Sans
                        },
                    };
                    let tw = look::width_styled(c, &tx.text, size, st);
                    let dx = match tx.anchor {
                        1 => tw * 0.5,
                        2 => tw,
                        _ => 0.0,
                    };
                    let col = krate::gfx::types::Color {
                        r: tx.color[0],
                        g: tx.color[1],
                        b: tx.color[2],
                        a: tx.color[3],
                    };
                    look::text_styled(
                        c,
                        &tx.text,
                        ox + x + tx.x * k - dx,
                        oy + y + tx.y * k,
                        size,
                        st,
                        col,
                    );
                }
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
        Widget::Diagram { d, x, y, .. } => draw_chart(c, d, ox + x, oy + y, t),
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

/// A property's field while it is edited: the text, its selection and
/// the caret, in a bordered box.
fn edit_box(c: u64, input: &crate::app::Input, rc: Rect, t: &Theme) {
    round(c, rc, 4.0, t.bg);
    look::outline(c, rc, 4.0, 1.0, t.border_focus);
    let base = rc.y + rc.h * 0.5 + 5.0;
    let x = rc.x + 6.0;
    if let Some((a, b)) = input.selection() {
        let xa = look::width(c, input.text.get(..a).unwrap_or(""), 15.0, 400);
        let xb = look::width(c, input.text.get(..b).unwrap_or(""), 15.0, 400);
        fill(c, r(x + xa, rc.y + 4.0, xb - xa, rc.h - 8.0), t.sel);
    }
    look::fit(c, &input.text, x, base, 15.0, 400, t.text, rc.w - 12.0);
    let cx = x + look::width(c, input.text.get(..input.caret).unwrap_or(""), 15.0, 400);
    fill(c, r(cx, rc.y + 5.0, 1.5, rc.h - 10.0), t.text);
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Where the date picker of the Properties block sits, relative to the
/// block's top-left: under the row of `line`.
fn picker_rect(rows: &[PropRow], line: usize) -> Option<Rect> {
    let row = rows.iter().find(|p| p.line == line)?;
    Some(r(144.0, row.y + 30.0, PICK_W, PICK_H))
}

/// Draw the open date picker over whatever is below the block.
pub fn draw_picker(c: u64, w: &Widget, ox: f32, oy: f32, t: &Theme) {
    let Widget::Props {
        rows,
        picker: Some((line, y, m)),
        ..
    } = w
    else {
        return;
    };
    let Some(pr) = picker_rect(rows, *line) else {
        return;
    };
    let rc = r(ox + pr.x, oy + pr.y, pr.w, pr.h);
    let _ = canvas2d::drop_shadow_round_rect(
        c,
        look::g(r(rc.x, rc.y + 3.0, rc.w, rc.h)),
        look::radii(8.0),
        14.0,
        look::alpha(look::hex(0x000000), 0.4),
    );
    round(c, rc, 8.0, t.modal);
    look::outline(c, rc, 8.0, 1.0, t.border);
    let title = alloc::format!(
        "{} {}",
        MONTHS
            .get((*m as usize).saturating_sub(1))
            .copied()
            .unwrap_or(""),
        y
    );
    look::text(c, &title, rc.x + 14.0, rc.y + 26.0, 15.0, 600, t.text);
    // The month arrows: back, then on.
    let (bx, by) = (rc.right() - 44.0, rc.y + 21.0);
    look::line(c, bx + 1.8, by - 3.5, bx - 1.8, by, 1.4, t.muted);
    look::line(c, bx - 1.8, by, bx + 1.8, by + 3.5, 1.4, t.muted);
    look::chevron(c, rc.right() - 18.0, by, false, 1.0, t.muted);
    let first = crate::props::weekday(*y, *m, 1);
    let days = crate::props::days_in_month(*y, *m);
    let current = rows
        .iter()
        .find(|p| p.line == *line)
        .map(|p| p.value.shown_raw())
        .unwrap_or_default();
    for (i, d) in ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"]
        .iter()
        .enumerate()
    {
        look::text(
            c,
            d,
            rc.x + 8.0 + i as f32 * 32.0 + 8.0,
            rc.y + 52.0,
            11.0,
            400,
            t.faint,
        );
    }
    for day in 1..=days {
        let slot = first + day - 1;
        let (col, row) = (slot % 7, slot / 7);
        let cell = r(
            rc.x + 8.0 + col as f32 * 32.0,
            rc.y + 58.0 + row as f32 * 26.0,
            30.0,
            24.0,
        );
        let this = alloc::format!("{y:04}-{m:02}-{day:02}");
        let on = current.starts_with(&this);
        if on {
            round(c, cell, 5.0, t.accent);
        }
        let s = alloc::format!("{day}");
        let w = look::width(c, &s, 13.0, 400);
        look::text(
            c,
            &s,
            cell.x + (cell.w - w) * 0.5,
            cell.y + 16.5,
            13.0,
            400,
            if on { t.on_accent } else { t.text },
        );
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
            for (it, (cx, cw)) in items.iter().zip(p.chips.iter()) {
                if cx + cw > room {
                    break;
                }
                let px = x + cx;
                let label = if p.kind == Kind::Tags {
                    it.trim_start_matches('#')
                } else {
                    it.as_str()
                };
                if p.kind == Kind::Tags {
                    round(c, r(px, cy - 12.0, *cw, 24.0), 12.0, t.tag_bg);
                    look::text(c, label, px + 10.0, base, 15.0, 400, t.link);
                    lucide(c, crate::lucide::X, px + cw - 13.0, cy, 11.0, t.link);
                } else {
                    // Aliases and lists are plain words with a cross, no pill.
                    look::text(c, label, px, base, 15.0, 400, t.text);
                    lucide(c, crate::lucide::X, px + cw - 9.0, cy, 11.0, t.muted);
                }
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

/// The categorical colours of charts (pie wedges, Gantt sections), from
/// Obsidian's palette.
const SERIES: [u32; 12] = [
    0x027AFF, 0xE9973F, 0x44CF6E, 0xFB464C, 0xA882FF, 0x53DFDD, 0xE0DE71, 0xFA99CD, 0x7A8BA3,
    0xB5784C, 0x3DA35D, 0x9E6FD6,
];

fn role(rl: crate::charts::Role, t: &Theme, fill: bool) -> Option<Rgba> {
    use crate::charts::Role;
    Some(match rl {
        Role::None => return None,
        Role::Node => {
            if fill {
                t.code_bg
            } else {
                t.muted
            }
        }
        Role::Paper => t.bg,
        Role::Cluster => look::alpha(t.muted, 0.12),
        Role::Note => look::alpha(t.muted, 0.18),
        Role::Line => t.muted,
        Role::Text => t.text,
        Role::Muted => t.faint,
        Role::Series(i) => look::alpha(look::hex(SERIES[usize::from(i) % SERIES.len()]), 0.85),
        Role::Done => look::alpha(t.muted, 0.25),
        Role::Active => look::alpha(look::hex(0xA882FF), 0.45),
        Role::Critical => look::alpha(look::hex(0xFB464C), 0.85),
    })
}

fn head(c: u64, h: crate::charts::Head, tip: (f32, f32), from: (f32, f32), col: Rgba, bg: Rgba) {
    use crate::charts::Head;
    let (dx, dy) = (tip.0 - from.0, tip.1 - from.1);
    let len = (dx * dx + dy * dy).sqrt().max(1.0);
    let (ux, uy) = (dx / len, dy / len);
    let (px, py) = (-uy, ux);
    let at = |back: f32, side: f32| (tip.0 - ux * back + px * side, tip.1 - uy * back + py * side);
    match h {
        Head::None => {}
        Head::Arrow | Head::Open => {
            let (a, b) = (at(9.0, 4.5), at(9.0, -4.5));
            look::line(c, tip.0, tip.1, a.0, a.1, 1.6, col);
            look::line(c, tip.0, tip.1, b.0, b.1, 1.6, col);
            if h == Head::Arrow {
                for k in 1..8 {
                    let f = k as f32 / 8.0;
                    let (p, q) = (at(9.0 * f, 4.5 * f), at(9.0 * f, -4.5 * f));
                    look::line(c, p.0, p.1, q.0, q.1, 1.4, col);
                }
            }
        }
        Head::Hollow => {
            let (a, b) = (at(12.0, 6.0), at(12.0, -6.0));
            for k in 1..10 {
                let f = k as f32 / 10.0;
                let (p, q) = (at(12.0 * f, 6.0 * f), at(12.0 * f, -6.0 * f));
                look::line(c, p.0, p.1, q.0, q.1, 1.3, bg);
            }
            look::line(c, tip.0, tip.1, a.0, a.1, 1.4, col);
            look::line(c, tip.0, tip.1, b.0, b.1, 1.4, col);
            look::line(c, a.0, a.1, b.0, b.1, 1.4, col);
        }
        Head::Diamond | Head::FilledDiamond => {
            let (l, m, r) = (at(7.0, 5.0), at(14.0, 0.0), at(7.0, -5.0));
            let ink = if h == Head::FilledDiamond { col } else { bg };
            let _ = canvas2d::fill_circle(
                c,
                krate::gfx::types::Point {
                    x: (tip.0 + m.0) * 0.5,
                    y: (tip.1 + m.1) * 0.5,
                },
                4.0,
                ink,
            );
            look::line(c, tip.0, tip.1, l.0, l.1, 1.4, col);
            look::line(c, l.0, l.1, m.0, m.1, 1.4, col);
            look::line(c, m.0, m.1, r.0, r.1, 1.4, col);
            look::line(c, r.0, r.1, tip.0, tip.1, 1.4, col);
        }
        Head::Cross => {
            let (a, b, c2, d) = (at(10.0, 5.0), at(2.0, -5.0), at(10.0, -5.0), at(2.0, 5.0));
            look::line(c, a.0, a.1, b.0, b.1, 1.6, col);
            look::line(c, c2.0, c2.1, d.0, d.1, 1.6, col);
        }
    }
}

/// Paint a laid-out Mermaid chart with its top-left at (ox, oy).
pub fn draw_chart(c: u64, d: &crate::charts::Chart, ox: f32, oy: f32, t: &Theme) {
    use crate::charts::Item;
    for it in &d.items {
        match it {
            Item::Box {
                x,
                y,
                w,
                h,
                r: rad,
                fill: f,
                stroke,
                dashed,
            } => {
                let rc = r(ox + x, oy + y, *w, *h);
                if let Some(col) = role(*f, t, true) {
                    round(c, rc, *rad, col);
                }
                if let Some(col) = role(*stroke, t, false) {
                    if *dashed {
                        let mut xx = rc.x;
                        while xx < rc.right() {
                            fill(c, r(xx, rc.y, 4.0f32.min(rc.right() - xx), 1.0), col);
                            fill(
                                c,
                                r(xx, rc.bottom() - 1.0, 4.0f32.min(rc.right() - xx), 1.0),
                                col,
                            );
                            xx += 8.0;
                        }
                    } else {
                        look::outline(c, rc, *rad, 1.0, col);
                    }
                }
            }
            Item::Diamond {
                x,
                y,
                w,
                h,
                fill: f,
                stroke,
            } => {
                let (cx, cy) = (ox + x + w * 0.5, oy + y + h * 0.5);
                if let Some(col) = role(*f, t, true) {
                    let steps = (h * 0.5) as usize;
                    for i in 0..steps {
                        let k = i as f32 / steps.max(1) as f32;
                        let half = w * 0.5 * (1.0 - k);
                        fill(
                            c,
                            r(cx - half, cy - h * 0.5 * k - 1.0, half * 2.0, 1.0),
                            col,
                        );
                        fill(c, r(cx - half, cy + h * 0.5 * k, half * 2.0, 1.0), col);
                    }
                }
                if let Some(col) = role(*stroke, t, false) {
                    look::line(c, cx, cy - h * 0.5, cx + w * 0.5, cy, 1.2, col);
                    look::line(c, cx + w * 0.5, cy, cx, cy + h * 0.5, 1.2, col);
                    look::line(c, cx, cy + h * 0.5, cx - w * 0.5, cy, 1.2, col);
                    look::line(c, cx - w * 0.5, cy, cx, cy - h * 0.5, 1.2, col);
                }
            }
            Item::Circle {
                cx,
                cy,
                r: rad,
                fill: f,
                stroke,
            } => {
                let p = krate::gfx::types::Point {
                    x: ox + cx,
                    y: oy + cy,
                };
                if let Some(col) = role(*f, t, true) {
                    let _ = canvas2d::fill_circle(c, p, *rad, col);
                }
                if let Some(col) = role(*stroke, t, false) {
                    let _ = canvas2d::stroke_circle(c, p, *rad, 1.2, col);
                }
            }
            Item::Wedge {
                cx,
                cy,
                r: rad,
                start,
                sweep,
                fill: f,
            } => {
                if let Some(col) = role(*f, t, false) {
                    let p = krate::gfx::types::Point {
                        x: ox + cx,
                        y: oy + cy,
                    };
                    // An arc as wide as the radius, half way out, fills the
                    // whole wedge.
                    let _ = canvas2d::stroke_arc(c, p, rad * 0.5, *start, *sweep, *rad, col);
                }
            }
            Item::Line {
                pts,
                role: rl,
                width,
                dashed,
                head: h,
                tail,
            } => {
                let Some(col) = role(*rl, t, false) else {
                    continue;
                };
                for seg in pts.windows(2) {
                    let (a, b) = (
                        (ox + seg[0].0, oy + seg[0].1),
                        (ox + seg[1].0, oy + seg[1].1),
                    );
                    if *dashed {
                        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
                        let len = (dx * dx + dy * dy).sqrt().max(1.0);
                        let n = (len / 6.0) as usize;
                        for i in (0..n).step_by(2) {
                            let (f0, f1) =
                                (i as f32 / n as f32, ((i + 1) as f32 / n as f32).min(1.0));
                            look::line(
                                c,
                                a.0 + dx * f0,
                                a.1 + dy * f0,
                                a.0 + dx * f1,
                                a.1 + dy * f1,
                                *width,
                                col,
                            );
                        }
                    } else {
                        look::line(c, a.0, a.1, b.0, b.1, *width, col);
                    }
                }
                let n = pts.len();
                if n >= 2 {
                    let tip = (ox + pts[n - 1].0, oy + pts[n - 1].1);
                    let from = (ox + pts[n - 2].0, oy + pts[n - 2].1);
                    head(c, *h, tip, from, col, t.bg);
                    let tip0 = (ox + pts[0].0, oy + pts[0].1);
                    let from0 = (ox + pts[1].0, oy + pts[1].1);
                    head(c, *tail, tip0, from0, col, t.bg);
                }
            }
            Item::Text {
                x,
                y,
                s,
                size,
                anchor,
                bold,
                role: rl,
            } => {
                let col = role(*rl, t, false).unwrap_or(t.text);
                let wgt = if *bold { 600 } else { 400 };
                let w = look::width(c, s, *size, wgt);
                let dx = match anchor {
                    1 => w * 0.5,
                    2 => w,
                    _ => 0.0,
                };
                look::text(c, s, ox + x - dx, oy + y, *size, wgt, col);
            }
        }
    }
}

/// Where the caret is drawn when it is in a table cell: x, top and height
/// relative to the table's row.
pub fn table_caret(c: u64, w: &Widget, cur: Pos) -> Option<(f32, f32, f32)> {
    let Widget::Table { cells, .. } = w else {
        return None;
    };
    let cell = cells.iter().find(|s| {
        s.line == Some(cur.line) && cur.col >= s.col0 && cur.col <= s.col0 + s.text.len()
    })?;
    // A header cell's text is wrapped in `**`; the caret stays inside it.
    let bold = cell.text.starts_with("**");
    let mut local = cur.col - cell.col0;
    if bold {
        local = local.clamp(2, cell.text.len().saturating_sub(2));
    }
    let (row, x) = cell.lay.x_of(c, &cell.text, local);
    let rl = cell.lay.rows.get(row)?;
    let size = cell.lay.size;
    Some((cell.x + x, cell.y + rl.base - size * 0.95, size * 1.25))
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
        Widget::Props {
            rows,
            add_y,
            picker,
        } => {
            // The open date picker first: it lies over everything.
            if let Some((line, y, m)) = picker {
                if let Some(pr) = picker_rect(rows, *line) {
                    if pr.contains(lx, ly) {
                        let (px, py) = (lx - pr.x, ly - pr.y);
                        if py < 40.0 {
                            if px > pr.w - 56.0 {
                                return Some(Act::DateMonth(px > pr.w - 30.0));
                            }
                            return Some(Act::Nothing);
                        }
                        let col = ((px - 8.0) / 32.0).floor();
                        let row = ((py - 58.0) / 26.0).floor();
                        if (0.0..7.0).contains(&col) && row >= 0.0 {
                            let slot = row as u32 * 7 + col as u32;
                            let first = crate::props::weekday(*y, *m, 1);
                            if slot >= first {
                                let d = slot - first + 1;
                                if d <= crate::props::days_in_month(*y, *m) {
                                    return Some(Act::DatePick(*line, *y, *m, d));
                                }
                            }
                        }
                        return Some(Act::Nothing);
                    }
                }
            }
            if ly >= *add_y && ly < add_y + 30.0 && lx < 140.0 {
                return Some(Act::AddProperty);
            }
            for p in rows {
                if ly >= p.y && ly < p.y + 30.0 {
                    if lx < 22.0 {
                        return Some(Act::PropMenu(p.line, 0.0, 0.0));
                    }
                    if lx < 144.0 {
                        return Some(Act::PropEdit(p.line, crate::propedit::Field::Key));
                    }
                    let vx = lx - 148.0;
                    if matches!(p.value, Value::Bool(_)) {
                        return (vx < 28.0).then_some(Act::Property(p.line));
                    }
                    for (i, (cx, cw)) in p.chips.iter().enumerate() {
                        if vx >= cx + cw - 22.0 && vx <= cx + cw {
                            return Some(Act::PropItemRemove(p.line, i));
                        }
                    }
                    return Some(Act::PropEdit(p.line, crate::propedit::Field::Value));
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
