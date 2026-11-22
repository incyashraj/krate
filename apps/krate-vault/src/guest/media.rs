//! A PDF, a sound or a video in a tab, as the notes app shows them.
//!
//! A PDF's pages stack down the tab under a toolbar (thumbnails, zoom, the
//! page number and the count). Krate draws each page to pixels
//! (`krate:gfx/pdf`); only the pages in view are drawn, at the size they
//! are shown, and the last few are kept so scrolling back is instant.
//! Sound and video play through `krate:gfx/video` from the vault folder the
//! person picked, under the player controls the notes app's own player has.

use super::look::{self, fill, icon, r, round, text, width, Icon, Rgba};
use super::{Drag, Ui, Zone};
use crate::app::{Menu, MenuAction, Overlay, View, TICKED};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use krate::gfx::{canvas2d, canvas_images, pdf, types as gfx, video};
use krate::layout::Rect;
#[allow(unused_imports)]
use krate::prelude::*;

/// Pages kept drawn, over every open PDF.
const KEEP_PAGES: usize = 8;
/// Thumbnails kept drawn.
const KEEP_THUMBS: usize = 48;
/// PDFs kept open.
const KEEP_DOCS: usize = 3;
/// The pdf viewer's widest automatic zoom.
const MAX_AUTO: f32 = 1.25;
/// Points to the viewer's CSS pixels.
const PT: f32 = 96.0 / 72.0;
/// The steps the zoom buttons go through.
const STEPS: [f32; 16] = [
    0.25, 0.33, 0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0,
];

/// A control in a PDF's toolbar or a player.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum FileCtl {
    Thumbs,
    ThumbsMenu,
    ZoomOut,
    ZoomIn,
    ZoomMenu,
    PageBox,
    /// A thumbnail, by page from 0.
    Thumb(u32),
    Play,
    Mute,
    More,
    Scrub,
    /// The picture itself: a click plays or pauses.
    Picture,
}

struct Doc {
    path: String,
    /// The vault generation it was read at.
    generation: u64,
    doc: Result<u64, String>,
    /// Each page's size in points.
    sizes: Vec<(f32, f32)>,
    /// The scale (pixels per point) it was last shown at, to keep the place
    /// when the zoom changes.
    shown_px: f32,
    used: u64,
    thumb_scroll: f32,
    /// How far across a page wider than the view is scrolled.
    across: f32,
}

struct Page {
    path: String,
    page: u32,
    /// Pixels per point it was drawn at.
    scale: f32,
    image: u64,
    used: u64,
    thumb: bool,
}

struct Player {
    path: String,
    id: Result<u64, String>,
    state: Option<video::VideoState>,
    muted: bool,
    looping: bool,
    seen: u64,
}

/// What the PDF and player tabs keep between frames.
#[derive(Default)]
pub struct Files {
    docs: Vec<Doc>,
    pages: Vec<Page>,
    players: Vec<Player>,
    frame: u64,
    /// A page in view is not drawn at its size yet.
    pending: bool,
    /// The page number being typed: its group and the text.
    pub page_edit: Option<(usize, String)>,
    /// When the pointer last moved over a player.
    pub stirred: u64,
    /// The scrubber as drawn: its group and track.
    track: Option<(usize, Rect)>,
}

pub use crate::popout::is_sound;

fn clock(ms: u64) -> String {
    let s = ms / 1000;
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// A filled triangle, pointing right, as rows of pixels.
fn triangle(c: u64, x: f32, y: f32, w: f32, h: f32, col: Rgba) {
    let rows = (h * 2.0).ceil().max(1.0) as usize;
    let step = h / rows as f32;
    for i in 0..rows {
        let mid = (i as f32 + 0.5) * step;
        let reach = (1.0 - (mid - h * 0.5).abs() / (h * 0.5)) * w;
        fill(c, r(x, y + i as f32 * step, reach, step), col);
    }
}

/// The player's own glyphs, filled as its controls are.
fn play_glyph(c: u64, playing: bool, cx: f32, cy: f32, k: f32, col: Rgba) {
    if playing {
        fill(c, r(cx - 6.0 * k, cy - 7.0 * k, 4.0 * k, 14.0 * k), col);
        fill(c, r(cx + 2.0 * k, cy - 7.0 * k, 4.0 * k, 14.0 * k), col);
    } else {
        triangle(c, cx - 5.0 * k, cy - 8.0 * k, 13.0 * k, 16.0 * k, col);
    }
}

fn speaker_glyph(c: u64, muted: bool, cx: f32, cy: f32, k: f32, col: Rgba) {
    // A box and a cone opening to the right, row by row.
    let x = cx - 9.0 * k;
    let rows = (16.0 * k).ceil() as usize;
    for i in 0..rows {
        let y = cy - 8.0 * k + i as f32;
        let d = (y + 0.5 - cy).abs();
        let start = if d <= 3.5 * k {
            x
        } else {
            x + 4.0 * k + 5.0 * k * ((d - 3.5 * k) / (4.5 * k))
        };
        let end = x + 9.0 * k;
        if end > start {
            fill(c, r(start, y, end - start, 1.0), col);
        }
    }
    let p = |x: f32, y: f32| gfx::Point { x, y };
    if muted {
        look::stroke(
            c,
            cx + 2.0 * k,
            cy - 4.0 * k,
            cx + 9.0 * k,
            cy + 3.0 * k,
            2.0 * k,
            col,
        );
        look::stroke(
            c,
            cx + 9.0 * k,
            cy - 4.0 * k,
            cx + 2.0 * k,
            cy + 3.0 * k,
            2.0 * k,
            col,
        );
    } else {
        let _ = canvas2d::stroke_arc(c, p(cx, cy), 4.0 * k, -50.0, 100.0, 2.0 * k, col);
        let _ = canvas2d::stroke_arc(c, p(cx, cy), 8.0 * k, -55.0, 110.0, 2.0 * k, col);
    }
}

fn dots_glyph(c: u64, cx: f32, cy: f32, k: f32, col: Rgba) {
    for d in [-6.0, 0.0, 6.0] {
        let _ = canvas2d::fill_circle(
            c,
            gfx::Point {
                x: cx,
                y: cy + d * k,
            },
            2.0 * k,
            col,
        );
    }
}

impl Files {
    /// The open PDF for a path, read and opened on first use.
    fn doc(
        &mut self,
        path: &str,
        generation: u64,
        read: impl FnOnce() -> Result<Vec<u8>, String>,
    ) -> usize {
        let now = self.frame;
        if let Some(i) = self.docs.iter().position(|d| d.path == path) {
            if self.docs[i].generation == generation || self.docs[i].doc.is_err() {
                self.docs[i].used = now;
                return i;
            }
            // The file changed on disk: read it again.
            let gone = self.docs.remove(i);
            self.forget_doc(gone);
        }
        while self.docs.len() >= KEEP_DOCS {
            let old = (0..self.docs.len())
                .min_by_key(|&i| self.docs[i].used)
                .unwrap_or(0);
            let gone = self.docs.remove(old);
            self.forget_doc(gone);
        }
        let mut sizes = Vec::new();
        let doc = read().and_then(|bytes| {
            let d = pdf::open(&bytes).map_err(|e| why(&e))?;
            let n = pdf::page_count(d).map_err(|e| why(&e))?;
            for i in 0..n {
                sizes.push(pdf::page_size(d, i).unwrap_or((612.0, 792.0)));
            }
            Ok(d)
        });
        self.docs.push(Doc {
            path: String::from(path),
            generation,
            doc,
            sizes,
            shown_px: 0.0,
            used: now,
            thumb_scroll: 0.0,
            across: 0.0,
        });
        self.docs.len() - 1
    }

    fn forget_doc(&mut self, d: Doc) {
        if let Ok(h) = d.doc {
            pdf::close(h);
        }
        self.pages.retain(|p| p.path != d.path);
    }
}

fn why(e: &pdf::GfxError) -> String {
    match e {
        pdf::GfxError::Unsupported(s) | pdf::GfxError::Platform(s) => s.clone(),
        pdf::GfxError::PermissionDenied => String::from("Krate did not allow it"),
        pdf::GfxError::InvalidTarget => String::from("it is gone"),
    }
}

/// Pixels per point for a PDF's zoom setting in a view `w` by `h`.
fn px_per_pt(zoom: f32, sizes: &[(f32, f32)], w: f32, h: f32) -> f32 {
    let (pw, ph) = sizes
        .iter()
        .fold((1.0f32, 1.0f32), |(a, b), (x, y)| (a.max(*x), b.max(*y)));
    let width = (w - 40.0).max(40.0) / (pw * PT);
    let page = ((h - 10.0).max(40.0) / (ph * PT)).min(width);
    let scale = if zoom == 0.0 {
        if pw <= ph {
            width.min(MAX_AUTO)
        } else {
            page.min(width)
        }
    } else if zoom == -1.0 {
        page
    } else if zoom == -2.0 {
        width
    } else {
        zoom
    };
    scale.clamp(0.1, 10.0) * PT
}

/// The zoom as a pdf-viewer scale (1 is actual size), for the buttons.
fn scale_of(px: f32) -> f32 {
    px / PT
}

impl Ui {
    pub(super) fn files_pending(&self) -> bool {
        self.files.pending
    }

    /// A sound or video in view is playing or still loading.
    pub(super) fn files_busy(&self) -> bool {
        self.files.players.iter().any(|p| {
            p.seen + 2 >= self.files.frame
                && p.state.as_ref().is_some_and(|s| s.playing || !s.ready)
        })
    }

    /// Before a frame: close players whose tab is gone.
    pub(super) fn files_sweep(&mut self) {
        self.files.frame += 1;
        self.files.pending = false;
        let open: Vec<String> = self
            .app
            .all_tabs()
            .filter_map(|t| match t.view.as_ref() {
                Some(View::File(f)) => Some(f.path.clone()),
                _ => None,
            })
            .collect();
        let mut gone = Vec::new();
        self.files.players.retain(|p| {
            let keep = open.contains(&p.path);
            if !keep {
                if let Ok(id) = p.id {
                    gone.push(id);
                }
            }
            keep
        });
        for id in gone {
            video::close(id);
        }
        let mut closed = Vec::new();
        let mut i = 0;
        while i < self.files.docs.len() {
            if open.contains(&self.files.docs[i].path) {
                i += 1;
            } else {
                closed.push(self.files.docs.remove(i));
            }
        }
        for d in closed {
            for p in self.files.pages.iter().filter(|p| p.path == d.path) {
                canvas_images::forget(self.c, p.image);
            }
            self.files.forget_doc(d);
        }
    }

    // ---- a PDF ----------------------------------------------------------------------

    pub(super) fn draw_pdf(&mut self, pane: usize, a: Rect, focused: bool, path: &str) {
        let t = self.t();
        let sz = self.sz;
        let k = sz.scale;
        let (zoom, thumbs, goto, scroll0) =
            match self.app.pane_tabs(pane).0.get(self.app.pane_tabs(pane).1) {
                Some(tb) => match tb.view.as_ref() {
                    Some(View::File(f)) => (f.zoom, f.thumbs, f.goto, f.scroll),
                    _ => return,
                },
                None => return,
            };
        let generation = self.app.vault.as_ref().map_or(0, |v| v.generation);
        let vault = &mut self.app.vault;
        let di = self.files.doc(path, generation, || {
            vault
                .as_mut()
                .ok_or_else(|| String::from("no vault"))
                .and_then(|v| v.fs.read_bytes(path))
        });
        if let Err(e) = &self.files.docs[di].doc {
            let e = e.clone();
            self.say_lines(
                a,
                &[crate::vfs::file_name(path), "This PDF cannot be shown", &e],
            );
            return;
        }
        let sizes = self.files.docs[di].sizes.clone();
        let count = sizes.len() as u32;

        // The toolbar.
        let bar_h = 36.0 * k;
        let bar = r(a.x, a.y, a.w, bar_h);
        fill(self.c, bar, t.bg);
        fill(self.c, r(a.x, bar.bottom(), a.w, 1.0), t.border);
        let cy = bar.y + bar_h * 0.5;
        let button =
            |me: &mut Ui, cx: f32, ic: Option<Icon>, ctl: FileCtl, label: &str, on: bool| {
                let b = r(cx - 13.0 * k, cy - 12.0 * k, 26.0 * k, 24.0 * k);
                let z = Zone::File(pane, ctl);
                let hot = me.hover.as_ref() == Some(&z);
                if hot || on {
                    round(me.c, b, 4.0, t.hover);
                }
                let col = if hot || on { t.text } else { t.muted };
                match ic {
                    Some(ic) => icon(me.c, ic, cx, cy, k, col),
                    None => look::chevron(me.c, cx, cy, true, k, col),
                }
                me.hit(b, z);
                me.said.button(b, label);
            };
        // Where the viewer puts them: the sidebar's two, the zoom's three
        // around a divider, then the page number.
        let at = |d: f32| a.x + d * k;
        button(
            self,
            at(26.0),
            Some(Icon::LayoutList),
            FileCtl::Thumbs,
            "Toggle sidebar",
            thumbs,
        );
        button(
            self,
            at(58.0),
            None,
            FileCtl::ThumbsMenu,
            "Sidebar options",
            false,
        );
        button(
            self,
            at(106.0),
            Some(Icon::ZoomOut),
            FileCtl::ZoomOut,
            "Zoom out",
            false,
        );
        fill(self.c, r(at(124.0), cy - 8.0 * k, 1.0, 16.0 * k), t.border);
        button(
            self,
            at(143.0),
            Some(Icon::ZoomIn),
            FileCtl::ZoomIn,
            "Zoom in",
            false,
        );
        button(
            self,
            at(175.0),
            None,
            FileCtl::ZoomMenu,
            "Zoom options",
            false,
        );
        let x = at(209.0);

        // The pages' area, and the thumbnails beside it.
        let side_w = if thumbs { 140.0 * k } else { 0.0 };
        let body = r(
            a.x + side_w,
            bar.bottom() + 1.0,
            a.w - side_w,
            a.bottom() - bar.bottom() - 1.0,
        );
        self.view_areas.push((pane, r(a.x, body.y, a.w, body.h)));
        let px = px_per_pt(zoom, &sizes, body.w, body.h);
        let border = 4.0 * scale_of(px);
        let mut tops = Vec::with_capacity(sizes.len());
        let mut y = 0.0;
        for (_, h) in &sizes {
            tops.push(y + border);
            y += h * px + border * 2.0;
        }
        let total = y;
        // Keep the place when the zoom changes.
        let old = self.files.docs[di].shown_px;
        let mut scroll = if old > 0.0 && (old - px).abs() > 0.001 {
            scroll0 * px / old
        } else {
            scroll0
        };
        if let Some(n) = goto {
            let n = (n.max(1).min(count.max(1)) - 1) as usize;
            scroll = tops.get(n).map_or(0.0, |t| t - border);
        }
        scroll = scroll.clamp(0.0, (total - body.h).max(0.0));
        self.files.docs[di].shown_px = px;
        if let Some(View::File(f)) = self.app.pane_view_mut(pane) {
            f.scroll = scroll;
            f.goto = None;
        }

        // The page most in view.
        let mut current = 0u32;
        let mut best = -1.0f32;
        for (i, (_, h)) in sizes.iter().enumerate() {
            let top = tops[i] - scroll;
            let seen = (top + h * px).min(body.h) - top.max(0.0);
            if seen > best + 0.5 {
                best = seen;
                current = i as u32;
            }
        }

        // The page number box and the count.
        let editing = self
            .files
            .page_edit
            .as_ref()
            .filter(|(p, _)| *p == pane)
            .map(|(_, s)| s.clone());
        let bw = 57.0 * k;
        let bx = r(x, cy - 14.0 * k, bw, 28.0 * k);
        round(self.c, bx, 4.0, t.field);
        look::outline(
            self.c,
            bx,
            4.0,
            1.0,
            if editing.is_some() {
                t.border_focus
            } else {
                t.border
            },
        );
        let shown = editing
            .clone()
            .unwrap_or_else(|| format!("{}", current + 1));
        let tw = width(self.c, &shown, sz.ui, 400);
        let base = cy + sz.ui * 0.36;
        text(
            self.c,
            &shown,
            bx.right() - 12.0 * k - tw,
            base,
            sz.ui,
            400,
            t.text,
        );
        if editing.is_some() && self.caret_visible() {
            fill(
                self.c,
                r(bx.right() - 11.0 * k, cy - 8.0 * k, 1.0, 16.0 * k),
                t.text,
            );
        }
        self.hit(bx, Zone::File(pane, FileCtl::PageBox));
        self.said.field(bx, "Page number", &shown);
        let of = format!("of {count}");
        text(self.c, &of, bx.right() + 4.0 * k, base, sz.ui, 400, t.text);
        self.said.text(r(bx.right(), bar.y, 60.0 * k, bar_h), &of);

        // The pages: only those in view are drawn, at the size shown.
        fill(self.c, body, t.bg);
        let _ = canvas2d::set_clip(self.c, body.x, body.y, body.w, body.h);
        let widest = sizes.iter().fold(0.0f32, |m, (w, _)| m.max(*w)) * px + border * 2.0;
        let across = self.files.docs[di]
            .across
            .clamp(0.0, (widest - body.w).max(0.0));
        self.files.docs[di].across = across;
        let want = (px * 2.0).min(4.0);
        let mut drew_one = false;
        for (i, (w, h)) in sizes.iter().enumerate() {
            let top = body.y + tops[i] - scroll;
            let (pw, ph) = (w * px, h * px);
            if top > body.bottom() || top + ph < body.y {
                continue;
            }
            let rc = r(
                body.x + ((body.w - pw) * 0.5).max(border) - across,
                top,
                pw,
                ph,
            );
            fill(
                self.c,
                r(rc.x - 1.0, rc.y - 1.0, rc.w + 2.0, rc.h + 2.0),
                t.border,
            );
            fill(self.c, rc, look::hex(0xFFFFFF));
            let page = i as u32;
            let scale = want.min(8192.0 / w.max(*h));
            if let Some(img) = self.page_image(path, page, scale, false, &mut drew_one, di) {
                let _ = canvas_images::draw(self.c, img, look::g(rc));
            }
            self.said.text(rc, &format!("Page {} of {}", i + 1, count));
        }
        let _ = canvas2d::clear_clip(self.c);
        if focused {
            self.hit(body, Zone::Pane(pane));
        }

        // The thumbnails.
        if thumbs {
            let side = r(a.x, body.y, side_w, body.h);
            fill(self.c, side, t.bg);
            fill(self.c, r(side.right() - 1.0, side.y, 1.0, side.h), t.border);
            let tw = side_w - 24.0 * k - 16.0 * k;
            let mut ty = side.y + 12.0 * k - self.files.docs[di].thumb_scroll;
            let mut boxes = Vec::new();
            for (i, (w, h)) in sizes.iter().enumerate() {
                let th = tw * h / w.max(1.0);
                boxes.push((
                    i as u32,
                    r(
                        side.x + (side_w - tw) * 0.5 - 8.0 * k,
                        ty,
                        tw + 16.0 * k,
                        th + 16.0 * k,
                    ),
                ));
                ty += th + 16.0 * k + 4.0 * k;
            }
            let full = ty + self.files.docs[di].thumb_scroll - side.y + 8.0 * k;
            let max = (full - side.h).max(0.0);
            if self.files.docs[di].thumb_scroll > max {
                self.files.docs[di].thumb_scroll = max;
            }
            let _ = canvas2d::set_clip(self.c, side.x, side.y, side.w - 1.0, side.h);
            for (page, b) in boxes {
                if b.bottom() < side.y || b.y > side.bottom() {
                    continue;
                }
                let z = Zone::File(pane, FileCtl::Thumb(page));
                if page == current || self.hover.as_ref() == Some(&z) {
                    round(self.c, b, 4.0, t.hover);
                }
                let img_r = r(b.x + 8.0 * k, b.y + 8.0 * k, b.w - 16.0 * k, b.h - 16.0 * k);
                fill(
                    self.c,
                    r(img_r.x - 1.0, img_r.y - 1.0, img_r.w + 2.0, img_r.h + 2.0),
                    t.border,
                );
                fill(self.c, img_r, look::hex(0xFFFFFF));
                let (w, _) = sizes[page as usize];
                let scale = (img_r.w * 2.0 / w.max(1.0)).min(2.0);
                if let Some(img) = self.page_image(path, page, scale, true, &mut drew_one, di) {
                    let _ = canvas_images::draw(self.c, img, look::g(img_r));
                }
                let label = format!("{}", page + 1);
                let lw = width(self.c, &label, sz.small, 400);
                let pill = r(
                    img_r.right() - 4.0 * k - lw - 8.0 * k,
                    img_r.bottom() - 4.0 * k - sz.small - 4.0 * k,
                    lw + 8.0 * k,
                    sz.small + 4.0 * k,
                );
                round(self.c, pill, 3.0, t.side);
                text(
                    self.c,
                    &label,
                    pill.x + 4.0 * k,
                    pill.bottom() - 3.0 * k,
                    sz.small,
                    400,
                    t.muted,
                );
                self.hit(b, z);
                self.said.button(b, &format!("Page {}", page + 1));
            }
            let _ = canvas2d::clear_clip(self.c);
        }
    }

    /// A page's picture at `scale`, drawn now if this frame has not drawn
    /// one yet; otherwise the nearest kept one (and another frame follows).
    fn page_image(
        &mut self,
        path: &str,
        page: u32,
        scale: f32,
        thumb: bool,
        drew_one: &mut bool,
        di: usize,
    ) -> Option<u64> {
        let now = self.files.frame;
        let close = |p: &Page| (p.scale - scale).abs() < scale * 0.02;
        if let Some(p) = self
            .files
            .pages
            .iter_mut()
            .find(|p| p.path == path && p.page == page && p.thumb == thumb && close(p))
        {
            p.used = now;
            return Some(p.image);
        }
        let Ok(doc) = self.files.docs[di].doc else {
            return None;
        };
        // One page drawn a frame keeps scrolling smooth; the first frame
        // draws every page in view, so the window never opens on white.
        if !*drew_one || !self.first_frame_done {
            *drew_one = true;
            if let Ok(px) = pdf::render(doc, page, scale) {
                if let Ok(image) = canvas_images::keep(self.c, px.width, px.height, &px.rgba) {
                    // Let go of the same page at another size, and the
                    // longest unused pages past the limit.
                    let c = self.c;
                    self.files.pages.retain(|p| {
                        let same = p.path == path && p.page == page && p.thumb == thumb;
                        if same {
                            canvas_images::forget(c, p.image);
                        }
                        !same
                    });
                    self.files.pages.push(Page {
                        path: String::from(path),
                        page,
                        scale,
                        image,
                        used: now,
                        thumb,
                    });
                    let limit = if thumb { KEEP_THUMBS } else { KEEP_PAGES };
                    loop {
                        let mine: Vec<usize> = (0..self.files.pages.len())
                            .filter(|&i| self.files.pages[i].thumb == thumb)
                            .collect();
                        if mine.len() <= limit {
                            break;
                        }
                        let Some(&old) = mine.iter().min_by_key(|&&i| self.files.pages[i].used)
                        else {
                            break;
                        };
                        if self.files.pages[old].used == now {
                            break;
                        }
                        let p = self.files.pages.remove(old);
                        canvas_images::forget(self.c, p.image);
                    }
                    return Some(image);
                }
            }
            return None;
        }
        self.files.pending = true;
        // Meanwhile, the page at another size, stretched.
        self.files
            .pages
            .iter_mut()
            .find(|p| p.path == path && p.page == page && p.thumb == thumb)
            .map(|p| {
                p.used = now;
                p.image
            })
    }

    fn say_lines(&mut self, a: Rect, lines: &[&str]) {
        let t = self.t();
        let sz = self.sz;
        let mut y = a.y + a.h * 0.4;
        for (i, l) in lines.iter().enumerate() {
            let size = if i == 0 { sz.medium } else { sz.ui };
            let weight = if i == 0 { 600 } else { 400 };
            let w = width(self.c, l, size, weight);
            text(
                self.c,
                l,
                a.x + (a.w - w) * 0.5,
                y,
                size,
                weight,
                if i == 0 { t.text } else { t.muted },
            );
            self.said.text(r(a.x, y - size, a.w, size * 1.4), l);
            y += size * 1.8;
        }
    }

    // ---- sound and video ------------------------------------------------------------

    fn player(&mut self, path: &str) -> usize {
        if let Some(i) = self.files.players.iter().position(|p| p.path == path) {
            return i;
        }
        let host = self.app.vault.as_ref().and_then(|v| v.fs.host_path(path));
        let id = match host {
            Some(h) => video::open(&h).map_err(|e| why(&e)),
            None => Err(String::from("This vault is not a folder on the computer")),
        };
        self.files.players.push(Player {
            path: String::from(path),
            id,
            state: None,
            muted: false,
            looping: false,
            seen: 0,
        });
        self.files.players.len() - 1
    }

    pub(super) fn draw_media(&mut self, pane: usize, a: Rect, focused: bool, path: &str) {
        let t = self.t();
        let k = self.sz.scale;
        let looping = match self.app.pane_tabs(pane).0.get(self.app.pane_tabs(pane).1) {
            Some(tb) => match tb.view.as_ref() {
                Some(View::File(f)) => f.looping,
                _ => return,
            },
            None => return,
        };
        let i = self.player(path);
        let frame = self.files.frame;
        let p = &mut self.files.players[i];
        p.seen = frame;
        let id = match &p.id {
            Ok(id) => *id,
            Err(e) => {
                let e = e.clone();
                self.say_lines(
                    a,
                    &[
                        crate::vfs::file_name(path),
                        "This file cannot be played",
                        &e,
                    ],
                );
                return;
            }
        };
        if p.looping != looping {
            p.looping = looping;
            let _ = video::set_looping(id, looping);
        }
        let got = video::state(id);
        if let Err(e) = &got {
            let e = why(e);
            self.say_lines(
                a,
                &[
                    crate::vfs::file_name(path),
                    "This file cannot be played",
                    &e,
                ],
            );
            return;
        }
        let p = &mut self.files.players[i];
        p.state = got.ok();
        let st = p.state.clone().unwrap_or(video::VideoState {
            position_millis: 0,
            duration_millis: 0,
            width: 0,
            height: 0,
            ready: false,
            playing: false,
            ended: false,
            problem: String::new(),
        });
        let muted = p.muted;
        if !st.problem.is_empty() {
            self.say_lines(
                a,
                &[
                    crate::vfs::file_name(path),
                    "This file cannot be played",
                    &st.problem,
                ],
            );
            return;
        }
        if focused {
            self.hit(a, Zone::Pane(pane));
        }
        let sound = is_sound(path) || (st.ready && st.width == 0);
        if sound {
            self.draw_audio(pane, a, &st, muted, k, t);
            return;
        }
        // The picture at its own size, smaller when the tab is.
        let (vw, vh) = if st.width > 0 {
            (st.width as f32, st.height as f32)
        } else {
            (640.0, 360.0)
        };
        let room_w = a.w - 2.0 * 12.0 * k;
        let room_h = a.h - 2.0 * 10.0 * k;
        let f = (room_w / vw).min(room_h / vh).min(1.0);
        let (dw, dh) = (vw * f, vh * f);
        let v = r(a.x + (a.w - dw) * 0.5, a.y + 10.0 * k, dw, dh);
        fill(self.c, v, look::hex(0x000000));
        let _ = video::draw(self.c, id, look::g(v));
        self.hit(v, Zone::File(pane, FileCtl::Picture));
        self.said.text(v, crate::vfs::file_name(path));
        let near = self.hover_point.is_some_and(|(x, y)| v.contains(x, y))
            && self.app.now.saturating_sub(self.files.stirred) < 3000;
        if st.playing && !near {
            return;
        }
        // The controls over the bottom of the picture.
        let shade = r(v.x, v.bottom() - 72.0 * k, v.w, 72.0 * k);
        let _ = canvas2d::linear_gradient(
            self.c,
            look::g(shade),
            look::alpha(look::hex(0), 0.0),
            look::alpha(look::hex(0), 0.6),
        );
        let white = look::hex(0xFFFFFF);
        let row = v.bottom() - 48.0 * k;
        let play = r(v.x + 8.0 * k, row - 16.0 * k, 32.0 * k, 32.0 * k);
        play_glyph(self.c, st.playing, play.x + 16.0 * k, row, k, white);
        self.hit(play, Zone::File(pane, FileCtl::Play));
        self.said
            .button(play, if st.playing { "Pause" } else { "Play" });
        let time = format!(
            "{} / {}",
            clock(st.position_millis),
            clock(st.duration_millis)
        );
        text(
            self.c,
            &time,
            v.x + 48.0 * k,
            row + 5.0 * k,
            14.0 * k,
            400,
            white,
        );
        self.said.text(
            r(v.x + 48.0 * k, row - 10.0 * k, 100.0 * k, 20.0 * k),
            &time,
        );
        let more = r(v.right() - 40.0 * k, row - 16.0 * k, 32.0 * k, 32.0 * k);
        dots_glyph(self.c, more.x + 16.0 * k, row, k, white);
        self.hit(more, Zone::File(pane, FileCtl::More));
        self.said.button(more, "Show more media controls");
        let mute = r(v.right() - 88.0 * k, row - 16.0 * k, 32.0 * k, 32.0 * k);
        speaker_glyph(self.c, muted, mute.x + 16.0 * k, row, k, white);
        self.hit(mute, Zone::File(pane, FileCtl::Mute));
        self.said
            .button(mute, if muted { "Unmute" } else { "Mute" });
        let track = r(
            v.x + 16.0 * k,
            v.bottom() - 24.0 * k,
            v.w - 32.0 * k,
            4.0 * k,
        );
        self.scrubber(pane, track, &st, white, look::alpha(white, 0.4));
    }

    fn draw_audio(
        &mut self,
        pane: usize,
        a: Rect,
        st: &video::VideoState,
        muted: bool,
        k: f32,
        t: &super::look::Theme,
    ) {
        let bx = r(a.x + 12.0 * k, a.y + 10.0 * k, a.w - 24.0 * k, 42.0 * k);
        round(self.c, bx, 8.0 * k, t.code_bg);
        look::outline(self.c, bx, 8.0 * k, 1.0, t.border);
        let cy = bx.y + bx.h * 0.5;
        let play = r(bx.x + 11.0 * k, cy - 16.0 * k, 32.0 * k, 32.0 * k);
        play_glyph(self.c, st.playing, play.x + 16.0 * k, cy, k, t.text);
        self.hit(play, Zone::File(pane, FileCtl::Play));
        self.said
            .button(play, if st.playing { "Pause" } else { "Play" });
        let time = format!(
            "{} / {}",
            clock(st.position_millis),
            clock(st.duration_millis)
        );
        let tw = width(self.c, &time, 14.0 * k, 400);
        text(
            self.c,
            &time,
            bx.x + 48.0 * k,
            cy + 5.0 * k,
            14.0 * k,
            400,
            t.text,
        );
        self.said.text(r(bx.x + 48.0 * k, bx.y, tw, bx.h), &time);
        let more = r(bx.right() - 43.0 * k, cy - 16.0 * k, 32.0 * k, 32.0 * k);
        dots_glyph(self.c, more.x + 16.0 * k, cy, k, t.text);
        self.hit(more, Zone::File(pane, FileCtl::More));
        self.said.button(more, "Show more media controls");
        let mute = r(bx.right() - 75.0 * k, cy - 16.0 * k, 32.0 * k, 32.0 * k);
        speaker_glyph(self.c, muted, mute.x + 16.0 * k, cy, k, t.text);
        self.hit(mute, Zone::File(pane, FileCtl::Mute));
        self.said
            .button(mute, if muted { "Unmute" } else { "Mute" });
        let x0 = bx.x + 48.0 * k + tw + 16.0 * k;
        let track = r(
            x0,
            cy - 2.0 * k,
            (mute.x - 16.0 * k - x0).max(20.0),
            4.0 * k,
        );
        let rest = look::alpha(t.text, 0.75);
        self.scrubber(pane, track, st, t.text, rest);
    }

    fn scrubber(
        &mut self,
        pane: usize,
        track: Rect,
        st: &video::VideoState,
        played: Rgba,
        rest: Rgba,
    ) {
        let k = self.sz.scale;
        round(self.c, track, track.h * 0.5, rest);
        let f = if st.duration_millis > 0 {
            (st.position_millis as f32 / st.duration_millis as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };
        if f > 0.0 {
            round(
                self.c,
                r(track.x, track.y, track.w * f, track.h),
                track.h * 0.5,
                played,
            );
            let _ = canvas2d::fill_circle(
                self.c,
                gfx::Point {
                    x: track.x + track.w * f,
                    y: track.y + track.h * 0.5,
                },
                6.0 * k,
                played,
            );
        }
        let grab = r(
            track.x - 6.0 * k,
            track.y - 8.0 * k,
            track.w + 12.0 * k,
            track.h + 16.0 * k,
        );
        self.hit(grab, Zone::File(pane, FileCtl::Scrub));
        self.said
            .adjustable(grab, "Seek", &clock(st.position_millis));
        self.files.track = Some((pane, track));
    }

    // ---- presses, the wheel and keys ------------------------------------------------

    fn file_tab(&self, pane: usize) -> Option<&crate::app::FileTab> {
        let (tabs, act) = self.app.pane_tabs(pane);
        match tabs.get(act)?.view.as_ref()? {
            View::File(f) => Some(f),
            _ => None,
        }
    }

    pub(super) fn file_press(&mut self, pane: usize, ctl: FileCtl, px: f32, py: f32) {
        if pane != self.app.pane {
            self.app.focus_pane(pane);
        }
        let Some(f) = self.file_tab(pane) else {
            return;
        };
        let (path, zoom, thumbs, looping) = (f.path.clone(), f.zoom, f.thumbs, f.looping);
        if ctl != FileCtl::PageBox {
            self.files.page_edit = None;
        }
        match ctl {
            FileCtl::Thumbs => self.app.menu_action(MenuAction::PdfThumbs),
            FileCtl::ThumbsMenu => {
                let mut l = String::from("Thumbnails");
                if thumbs {
                    l.push_str(TICKED);
                }
                self.app.overlay = Some(Overlay::Menu(Menu {
                    x: px - 20.0,
                    y: py + 14.0,
                    items: alloc::vec![(l, MenuAction::PdfThumbs)],
                }));
            }
            FileCtl::ZoomOut | FileCtl::ZoomIn => {
                let now = self.shown_scale(pane, &path, zoom);
                let next = if ctl == FileCtl::ZoomIn {
                    STEPS
                        .iter()
                        .copied()
                        .find(|s| *s > now + 0.001)
                        .unwrap_or(10.0f32.min(now * 1.1))
                } else {
                    STEPS
                        .iter()
                        .rev()
                        .copied()
                        .find(|s| *s < now - 0.001)
                        .unwrap_or(0.1f32.max(now / 1.1))
                };
                if let Some(View::File(f)) = self.app.pane_view_mut(pane) {
                    f.zoom = next;
                }
            }
            FileCtl::ZoomMenu => {
                let pick = |l: &str, on: bool, v: i32| {
                    let mut s = String::from(l);
                    if on {
                        s.push_str(TICKED);
                    }
                    (s, MenuAction::PdfZoom(v))
                };
                let mut items = alloc::vec![
                    pick("Automatic zoom", zoom == 0.0, 0),
                    pick("Actual size", zoom == 1.0, 100),
                    pick("Page fit", zoom == -1.0, -1),
                    pick("Page width", zoom == -2.0, -2),
                ];
                for pct in [50, 75, 100, 125, 150, 200, 300, 400] {
                    if pct == 100 {
                        continue;
                    }
                    items.push(pick(
                        &format!("{pct}%"),
                        (zoom * 100.0).round() as i32 == pct,
                        pct,
                    ));
                }
                self.app.overlay = Some(Overlay::Menu(Menu {
                    x: px - 20.0,
                    y: py + 14.0,
                    items,
                }));
            }
            FileCtl::PageBox => {
                self.files.page_edit = Some((pane, String::new()));
            }
            FileCtl::Thumb(page) => {
                if let Some(View::File(f)) = self.app.pane_view_mut(pane) {
                    f.goto = Some(page + 1);
                }
            }
            FileCtl::Play | FileCtl::Picture => self.toggle_play(&path),
            FileCtl::Mute => {
                if let Some(p) = self.files.players.iter_mut().find(|p| p.path == path) {
                    p.muted = !p.muted;
                    if let Ok(id) = p.id {
                        let _ = video::set_muted(id, p.muted);
                    }
                }
            }
            FileCtl::More => {
                let mut l = String::from("Loop");
                if looping {
                    l.push_str(TICKED);
                }
                self.app.overlay = Some(Overlay::Menu(Menu {
                    x: px - 190.0,
                    y: py + 14.0,
                    items: alloc::vec![(l, MenuAction::MediaLoop)],
                }));
            }
            FileCtl::Scrub => {
                self.seek_to(&path, px);
                self.drag = Drag::Scrub(pane);
            }
        }
    }

    /// The zoom as shown, as a scale where 1 is actual size.
    fn shown_scale(&self, _pane: usize, path: &str, zoom: f32) -> f32 {
        if zoom > 0.0 {
            return zoom;
        }
        self.files
            .docs
            .iter()
            .find(|d| d.path == path)
            .map_or(1.0, |d| scale_of(d.shown_px))
    }

    fn toggle_play(&mut self, path: &str) {
        let Some(p) = self.files.players.iter_mut().find(|p| p.path == path) else {
            return;
        };
        let Ok(id) = p.id else {
            return;
        };
        let st = video::state(id).ok();
        if st.as_ref().is_some_and(|s| s.playing) {
            let _ = video::pause(id);
        } else {
            if st.as_ref().is_some_and(|s| s.ended) {
                let _ = video::seek(id, 0);
            }
            let _ = video::play(id);
        }
        p.state = video::state(id).ok();
    }

    fn seek_to(&mut self, path: &str, px: f32) {
        let Some((_, track)) = self.files.track else {
            return;
        };
        let Some(p) = self.files.players.iter_mut().find(|p| p.path == path) else {
            return;
        };
        let (Ok(id), Some(st)) = (p.id.as_ref(), p.state.as_ref()) else {
            return;
        };
        let f = ((px - track.x) / track.w.max(1.0)).clamp(0.0, 1.0);
        let _ = video::seek(*id, (st.duration_millis as f32 * f) as u64);
    }

    pub(super) fn file_drag(&mut self, pane: usize, px: f32) {
        if let Some(path) = self.file_tab(pane).map(|f| f.path.clone()) {
            self.seek_to(&path, px);
        }
    }

    /// The wheel over a PDF: its pages scroll, or with Cmd it zooms; over
    /// the thumbnails, they scroll.
    pub(super) fn file_wheel(
        &mut self,
        pane: usize,
        a: Rect,
        x: f32,
        (dx, dy): (f32, f32),
        zoom_key: bool,
    ) {
        let Some(f) = self.file_tab(pane) else {
            return;
        };
        let (path, zoom, thumbs) = (f.path.clone(), f.zoom, f.thumbs);
        if thumbs && x < a.x + 140.0 * self.sz.scale {
            if let Some(d) = self.files.docs.iter_mut().find(|d| d.path == path) {
                d.thumb_scroll = (d.thumb_scroll + dy).max(0.0);
            }
            return;
        }
        if zoom_key {
            let now = self.shown_scale(pane, &path, zoom);
            let next = (now * (-dy * 0.004).exp()).clamp(0.1, 10.0);
            if let Some(View::File(f)) = self.app.pane_view_mut(pane) {
                f.zoom = next;
            }
            return;
        }
        if let Some(d) = self.files.docs.iter_mut().find(|d| d.path == path) {
            d.across = (d.across + dx).max(0.0);
        }
        if let Some(View::File(f)) = self.app.pane_view_mut(pane) {
            f.scroll = (f.scroll + dy).max(0.0);
        }
    }

    /// A key while a PDF or player tab is in front. True when it was used.
    pub(super) fn file_key(&mut self, key: &str, m: crate::app::Mods) -> bool {
        if self.app.overlay.is_some() || m.primary || m.ctrl || m.alt {
            return false;
        }
        let pane = self.app.pane;
        if let Some((p, s)) = self.files.page_edit.as_mut() {
            let p = *p;
            match key {
                "Backspace" => {
                    s.pop();
                }
                "Enter" => {
                    let n = s.parse::<u32>().ok();
                    self.files.page_edit = None;
                    if let (Some(n), Some(View::File(f))) = (n, self.app.pane_view_mut(p)) {
                        f.goto = Some(n.max(1));
                    }
                }
                "Escape" => self.files.page_edit = None,
                _ => return true,
            }
            return true;
        }
        let Some(f) = self.file_tab(pane) else {
            return false;
        };
        let (path, kind) = (f.path.clone(), f.kind);
        match kind {
            crate::vault::FileKind::Pdf => {
                let page = self.h * 0.9;
                let step = match key {
                    "ArrowDown" => 40.0,
                    "ArrowUp" => -40.0,
                    "PageDown" | "Space" => page,
                    "PageUp" => -page,
                    "Home" => -1.0e9,
                    "End" => 1.0e9,
                    _ => return false,
                };
                if let Some(View::File(f)) = self.app.pane_view_mut(pane) {
                    f.scroll = (f.scroll + step).max(0.0);
                }
                true
            }
            crate::vault::FileKind::Media => {
                let Some(p) = self.files.players.iter().find(|p| p.path == path) else {
                    return false;
                };
                let (Ok(id), pos) = (
                    p.id.clone(),
                    p.state.as_ref().map_or(0, |s| s.position_millis),
                ) else {
                    return false;
                };
                match key {
                    "Space" | "k" => self.toggle_play(&path),
                    "ArrowRight" => {
                        let _ = video::seek(id, pos + 5000);
                    }
                    "ArrowLeft" => {
                        let _ = video::seek(id, pos.saturating_sub(5000));
                    }
                    _ => return false,
                }
                true
            }
            _ => false,
        }
    }

    /// Typed text while the page number box is open.
    pub(super) fn file_text(&mut self, s: &str) -> bool {
        let Some((_, edit)) = self.files.page_edit.as_mut() else {
            return false;
        };
        for ch in s.chars().filter(char::is_ascii_digit) {
            if edit.len() < 6 {
                edit.push(ch);
            }
        }
        true
    }
}
