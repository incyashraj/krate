//! Drawing every screen: the header and the floating bar, Library,
//! Discover, You, Settings and its pages, an app's page, and the sheet that
//! asks before something is removed.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use krate::gfx::{canvas2d, canvas_images, types as gfx};
use krate::layout::{self, Said};
use krate::math::F32Ext as _;

use crate::kit::*;
use crate::{
    progress, AppSync, Ask, Choice, Hit, Home, HubState, Metrics, SyncState, Tab, Target, CODE_LEN,
    MS, PAGE_NS, ROWS_NS, ROW_STEP_NS, SHEET_NS, TABS, TAB_NS,
};

type Drawn<T = ()> = Result<T, gfx::GfxError>;

/// What an empty Library says.
const EMPTY_BODY: &str =
    "Open a .krate from Messages, Mail or Files, or get one people made in Discover.";

/// The floating bar's height, and its gap above the home bar.
const NAV_H: f32 = 68.0;

/// An on/off switch's track.
const SWITCH_W: f32 = 51.0;
const SWITCH_H: f32 = 31.0;

/// What leads a row in a group.
#[derive(Clone)]
enum Lead {
    None,
    /// An app or a person: its face, the name an initial comes from, its
    /// side, and round (a person) or rounded-square (an app).
    Tile(Face, String, f32, bool),
    /// A small outline icon on a soft square.
    Icon(Icon),
    /// A word on a soft square ("Aa" for text size).
    Glyph(&'static str),
}

/// What ends a row in a group.
#[derive(Clone)]
enum Trail {
    None,
    /// Muted words on the right.
    Value(String),
    /// More behind this row.
    Nav,
    /// Muted words, and more behind.
    ValueNav(String),
    /// An on/off switch; the row's tap flips it.
    Switch(bool),
    /// A soft pill with a word on it; the row's tap does what it says.
    Button(String),
}

#[derive(Clone, Copy, PartialEq)]
enum Tone {
    Ink,
    Quiet,
    Accent,
    Danger,
}

#[derive(Clone)]
struct Row {
    label: String,
    sub: String,
    lead: Lead,
    trail: Trail,
    tone: Tone,
    hit: Option<Hit>,
}

impl Row {
    fn new(label: &str) -> Self {
        Row {
            label: label.to_string(),
            sub: String::new(),
            lead: Lead::None,
            trail: Trail::None,
            tone: Tone::Ink,
            hit: None,
        }
    }

    fn value(mut self, value: &str) -> Self {
        self.trail = Trail::Value(value.to_string());
        self
    }

    fn sub(mut self, sub: &str) -> Self {
        self.sub = sub.to_string();
        self
    }

    fn nav(mut self, hit: Hit) -> Self {
        self.trail = match self.trail {
            Trail::Value(v) | Trail::ValueNav(v) => Trail::ValueNav(v),
            _ => Trail::Nav,
        };
        self.hit = Some(hit);
        self
    }

    fn action(mut self, tone: Tone, hit: Hit) -> Self {
        self.tone = tone;
        self.hit = Some(hit);
        self
    }

    fn quiet(mut self) -> Self {
        self.tone = Tone::Quiet;
        self
    }

    fn switch(mut self, on: bool, hit: Hit) -> Self {
        self.trail = Trail::Switch(on);
        self.hit = Some(hit);
        self
    }

    fn button(mut self, word: &str, hit: Hit) -> Self {
        self.trail = Trail::Button(word.to_string());
        self.hit = Some(hit);
        self
    }

    fn tile(mut self, face: Face, name: &str, side: f32, round: bool) -> Self {
        self.lead = Lead::Tile(face, name.to_string(), side, round);
        self
    }

    fn icon(mut self, icon: Icon) -> Self {
        self.lead = Lead::Icon(icon);
        self
    }

    fn glyph(mut self, glyph: &'static str) -> Self {
        self.lead = Lead::Glyph(glyph);
        self
    }
}

/// A size as a person reads it ("18.4 MB"), as bytes, for sorting.
fn bytes_of(size: &str) -> f32 {
    let mut parts = size.split_whitespace();
    let n: f32 = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0.0);
    let unit = match parts.next().unwrap_or("").to_ascii_uppercase().as_str() {
        "KB" => 1024.0,
        "MB" => 1024.0 * 1024.0,
        "GB" => 1024.0 * 1024.0 * 1024.0,
        _ => 1.0,
    };
    n * unit
}

/// The app's own size from its line ("Received · 489 KB"): the part after
/// the last middle dot.
fn line_size(line: &str) -> f32 {
    line.rsplit('\u{00b7}')
        .next()
        .map(|s| bytes_of(s.trim()))
        .unwrap_or(0.0)
}

/// Bytes as a person reads them, the way the Player writes sizes.
fn human_bytes(bytes: f32) -> String {
    if bytes >= 1024.0 * 1024.0 {
        format!("{:.1} MB", bytes / (1024.0 * 1024.0))
    } else if bytes > 0.0 {
        format!("{} KB", ((bytes / 1024.0) as u32).max(1))
    } else {
        String::from("None")
    }
}

fn mix(a: gfx::Color, b: gfx::Color, t: f32) -> gfx::Color {
    gfx::Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

fn with_alpha(c: gfx::Color, a: f32) -> gfx::Color {
    gfx::Color { a: c.a * a, ..c }
}

/// `r` shrunk about its centre: a held button gives a little.
fn shrink(r: gfx::Rect, k: f32) -> gfx::Rect {
    let (w, h) = (r.width * k, r.height * k);
    rect(r.x + (r.width - w) / 2.0, r.y + (r.height - h) / 2.0, w, h)
}

/// The floating bar's colours: a light bar with a black capsule, or in
/// dark mode the same turned over.
struct NavLook {
    bar: gfx::Color,
    capsule: gfx::Color,
    pill: gfx::Color,
    on: gfx::Color,
    off: gfx::Color,
    button: gfx::Color,
    button_ink: gfx::Color,
}

fn nav_look(t: &Theme) -> NavLook {
    if t.dark {
        NavLook {
            bar: hex(0x18181b),
            capsule: hex(0xf4f4f5),
            pill: hex(0x0d0d0f),
            on: hex(0xf4f4f5),
            off: hex(0x52525b),
            button: hex(0xf4f4f5),
            button_ink: hex(0x0d0d0f),
        }
    } else {
        NavLook {
            bar: WHITE,
            capsule: hex(0x0d0d0d),
            pill: WHITE,
            on: hex(0x1c1c1e),
            off: rgba(1.0, 1.0, 1.0, 0.62),
            button: hex(0x0d0d0d),
            button_ink: WHITE,
        }
    }
}

/// Where everything in the floating bar sits with `selected` open.
struct NavPlaces {
    bar: gfx::Rect,
    capsule: gfx::Rect,
    button: gfx::Rect,
    slots: [gfx::Rect; 3],
}

impl Home {
    pub(crate) fn metrics(&self, w: f32, h: f32) -> Metrics {
        let screen = layout::Screen::of(self.win);
        let (st, sr, sb, sl) = screen.safe;
        let compact = w < 600.0;
        let scale = self.look.text_scale.clamp(0.85, 1.6);
        let side = if compact { 20.0 } else { 32.0 };
        let avail = (w - sl - sr - side * 2.0).max(200.0);
        let col = avail.min(640.0);
        let x = sl + side + (avail - col) / 2.0;
        // Below the status bar; a desktop window gets a little air instead.
        let head_y = if screen.touch || st > 0.0 {
            st.max(20.0) + 6.0
        } else {
            14.0
        };
        let title = 28.0 * scale.min(1.3);
        let head_h = (title + 26.0).max(52.0);
        let top = head_y + head_h;
        // The bar floats above the home bar, or above the window's foot.
        let foot = if sb > 0.0 { sb + 4.0 } else { 16.0 };
        let nav_top = h - foot - NAV_H;
        let base = if compact { 1.0 } else { 0.94 };
        Metrics {
            w,
            h,
            x,
            col,
            top,
            bottom: h,
            head_y,
            head_h,
            nav_top,
            safe_bottom: sb,
            title,
            name: 17.0 * scale * base,
            body: 14.5 * scale * base,
            small: 13.0 * scale * base,
            button: 15.0 * scale.min(1.4) * base,
            scale,
        }
    }

    /// Push a hit for something in the scrolling band, cut to the band so
    /// a row scrolled under the header cannot be tapped through it.
    fn content_hit(&mut self, m: &Metrics, r: gfx::Rect, hit: Hit) {
        let top = r.y.max(m.top);
        let bottom = (r.y + r.height).min(m.bottom);
        if bottom > top {
            self.hits.push((rect(r.x, top, r.width, bottom - top), hit));
        }
    }

    fn visible(m: &Metrics, y: f32, h: f32) -> bool {
        y + h > m.top && y < m.bottom
    }

    /// An outline icon in `ink`, kept once per shape, colour and size.
    fn icon(&mut self, icon: Icon, ink: gfx::Color, side: f32, stroke: f32) -> Option<u64> {
        let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0) as u64;
        let key = icon_key(icon)
            | c(ink.r) << 8
            | c(ink.g) << 16
            | c(ink.b) << 24
            | c(ink.a) << 32
            | ((side * 2.0) as u64 & 0xff) << 40
            | ((stroke * 10.0) as u64 & 0xff) << 48;
        if let Some((_, handle)) = self.icons.iter().find(|(k, _)| *k == key) {
            return Some(*handle);
        }
        let handle = keep_icon(self.canvas, icon, ink, side, stroke)?;
        if self.icons.len() > 40 {
            let (_, old) = self.icons.remove(0);
            canvas_images::forget(self.canvas, old);
        }
        self.icons.push((key, handle));
        Some(handle)
    }

    fn draw_icon(
        &mut self,
        icon: Icon,
        ink: gfx::Color,
        cx: f32,
        cy: f32,
        side: f32,
        stroke: f32,
    ) -> Drawn {
        if let Some(handle) = self.icon(icon, ink, side, stroke) {
            canvas_images::draw(
                self.canvas,
                handle,
                rect(cx - side / 2.0, cy - side / 2.0, side, side),
            )?;
        }
        Ok(())
    }

    /// A chevron in `ink`, kept once per colour.
    fn chevron(&mut self, ink: gfx::Color, back: bool) -> Option<u64> {
        let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0) as u64;
        let key = 0xfe
            | c(ink.r) << 8
            | c(ink.g) << 16
            | c(ink.b) << 24
            | c(ink.a) << 32
            | (back as u64) << 56;
        if let Some((_, handle)) = self.icons.iter().find(|(k, _)| *k == key) {
            return Some(*handle);
        }
        let handle = if back {
            keep_chevron(self.canvas, ink, true, 12.0, 21.0, 2.4)?
        } else {
            keep_chevron(self.canvas, ink, false, 8.0, 14.0, 1.9)?
        };
        self.icons.push((key, handle));
        Some(handle)
    }

    /// The page colour behind a screen: lists on the light background,
    /// settings-like pages on the grouped one.
    fn page_colour(&self, t: &Theme, layer: Option<usize>) -> gfx::Color {
        match layer {
            Some(_) => t.grouped,
            None if self.tab == Tab::You => t.grouped,
            None => t.bg,
        }
    }

    pub(crate) fn draw(&mut self, w: f32, h: f32) -> Drawn {
        let canvas = self.canvas;
        self.look = krate::layout::Look::now();
        let t = theme(&self.look);
        let m = self.metrics(w, h);
        self.hits.clear();
        self.said = Said::new();

        let top = self.stack.len().checked_sub(1);
        match self.page_anim {
            Some((pushing, since)) if top.is_some() => {
                // A page slides over the one under it, which drifts a little
                // the other way and dims, as a phone moves between pages.
                let p = progress(self.now, since, PAGE_NS);
                let out = if pushing { 1.0 - p } else { p };
                let under = top.and_then(|i| i.checked_sub(1));
                self.draw_layer(&m, &t, under, -0.28 * w * (1.0 - out), false)?;
                canvas2d::fill_rect(
                    canvas,
                    rect(0.0, 0.0, w, h),
                    rgba(0.0, 0.0, 0.0, 0.06 * (1.0 - out)),
                )?;
                let x = w * out;
                self.draw_layer(&m, &t, top, x, true)?;
                canvas2d::fill_rect(canvas, rect(x - 0.5, 0.0, 0.5, h), t.line)?;
            }
            _ => self.draw_layer(&m, &t, top, 0.0, true)?,
        }

        self.draw_nav(&m, &t)?;
        if let Some(ask) = self.ask {
            self.draw_ask(&m, &t, ask)?;
        }
        canvas2d::present(canvas)?;
        self.said.send(self.win);
        Ok(())
    }

    /// One screen -- a list, or a page from the stack -- at `dx` across.
    fn draw_layer(
        &mut self,
        m: &Metrics,
        t: &Theme,
        layer: Option<usize>,
        dx: f32,
        top: bool,
    ) -> Drawn {
        let canvas = self.canvas;
        let page = self.page_colour(t, layer);
        let mut m = *m;
        m.x += dx;
        canvas2d::fill_rect(canvas, rect(dx.max(0.0), 0.0, m.w - dx.max(0.0), m.h), page)?;
        let scroll = match layer {
            Some(i) => self.stack[i].scroll,
            None => self.scroll[self.tab.slot()],
        };
        let hits_before = self.hits.len();
        // The list stops at the floating bar's top edge, softened.
        m.bottom = m.nav_top + 2.0;
        canvas2d::set_clip(canvas, dx.max(0.0), m.top, m.w, (m.bottom - m.top).max(0.0))?;
        self.text.band = (m.top, m.bottom);
        let start = m.top + 8.0 - scroll;
        let end = match layer.map(|i| self.stack[i].target) {
            Some(target) => self.draw_page(&m, t, start, target)?,
            None => match self.tab {
                Tab::Library if self.library.is_empty() => self.draw_empty_library(&m, t, start)?,
                Tab::Library => self.draw_library(&m, t, start, page)?,
                Tab::Discover if self.hub.is_empty() => self.draw_no_hub(&m, t, start)?,
                Tab::Discover => self.draw_discover(&m, t, start, page)?,
                Tab::You => self.draw_you(&m, t, start)?,
            },
        };
        canvas2d::clear_clip(canvas)?;
        self.text.band = (f32::MIN, f32::MAX);
        // A soft edge where the list meets the bar: one small band.
        canvas2d::linear_gradient(
            canvas,
            rect(dx.max(0.0), m.nav_top - 26.0, m.w - dx.max(0.0), 28.0),
            with_alpha(page, 0.0),
            page,
        )?;
        if top {
            // Room to scroll the last row clear of the bar's soft edge.
            self.content = end + scroll - m.top + 34.0;
            self.span = m.nav_top - m.top;
            if scroll > self.max_scroll() + 0.5 {
                *self.scroll_mut() = self.max_scroll();
            }
        }
        self.draw_header(&m, t, layer, scroll, page, dx)?;
        if !top {
            // What is underneath is only a picture now.
            self.hits.truncate(hits_before);
            self.said = Said::new();
        }
        Ok(())
    }

    // -- The header -------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    fn draw_header(
        &mut self,
        m: &Metrics,
        t: &Theme,
        layer: Option<usize>,
        scroll: f32,
        page: gfx::Color,
        dx: f32,
    ) -> Drawn {
        let canvas = self.canvas;
        let (x, col) = (m.x, m.col);
        let y = m.head_y;
        let band = m.head_h;
        // The list passes under the header, which keeps its own colour.
        let left = dx.max(0.0);
        let across = (m.w - left).max(0.0);
        canvas2d::fill_rect(canvas, rect(left, 0.0, across, m.top), page)?;
        if scroll > 1.0 {
            canvas2d::fill_rect(canvas, rect(left, m.top - 0.5, across, 0.5), t.line)?;
        }
        if let Some(i) = layer {
            // Back to the screen underneath, named for it.
            let label = match i.checked_sub(1) {
                Some(under) => self.stack[under].target.name(self),
                None => self.tab.name().to_string(),
            };
            let size = m.name;
            let held = self.pressed == Some(Hit::Back);
            let lw = self.text.width(&label, size, 500);
            let target_r = rect(x - 12.0, y, lw + 44.0, band);
            if held {
                canvas2d::fill_round_rect(
                    canvas,
                    rect(x - 8.0, y + 8.0, lw + 34.0, band - 16.0),
                    radii(10.0),
                    t.press,
                )?;
            }
            if let Some(chev) = self.chevron(t.accent, true) {
                canvas_images::draw(
                    canvas,
                    chev,
                    rect(x - 2.0, y + (band - 21.0) / 2.0, 12.0, 21.0),
                )?;
            }
            self.text
                .draw(&label, x + 17.0, mid(y, band, size), size, t.accent, 500)?;
            self.hits.push((target_r, Hit::Back));
            self.said.button(lay(target_r), &format!("Back to {label}"));
            // Once the page's own title has scrolled away, it waits here.
            if scroll > 70.0 {
                let name = self.stack[i].target.name(self);
                let size = 16.5 * m.scale.min(1.3);
                let room = col - (lw + 44.0) * 2.0;
                let shown = self.text.fit(&name, size, 600, room.max(40.0));
                let nw = self.text.width(&shown, size, 600);
                self.text.draw(
                    &shown,
                    x + col / 2.0 - nw / 2.0,
                    mid(y, band, size),
                    size,
                    t.ink,
                    600,
                )?;
            }
            return Ok(());
        }
        let mark = 28.0 * m.scale.min(1.2);
        if let Some(handle) = self.mark {
            canvas_images::draw(canvas, handle, rect(x, y + (band - mark) / 2.0, mark, mark))?;
        }
        let title = self.tab.name();
        let tx = x + mark + 10.0;
        let tw = self.text.width(title, m.title, 700);
        self.text
            .draw(title, tx, mid(y, band, m.title), m.title, t.ink, 700)?;
        self.said.heading(layout::Rect::new(tx, y, tw, band), title);
        if self.tab == Tab::You {
            // Settings, from the corner.
            let d = 38.0;
            let (cx, cy) = (x + col - d / 2.0, y + band / 2.0);
            let held = self.pressed == Some(Hit::Show(Target::Settings));
            let r = if held { d / 2.0 * 0.94 } else { d / 2.0 };
            canvas2d::fill_circle(
                canvas,
                pt(cx, cy),
                r,
                if t.dark { t.group } else { t.surface },
            )?;
            canvas2d::stroke_circle(canvas, pt(cx, cy), r, 1.0, t.hair)?;
            self.draw_icon(Icon::Cog, t.ink2, cx, cy, 20.0, 1.7)?;
            let target = rect(x + col - d - 6.0, y, d + 12.0, band);
            self.hits.push((target, Hit::Show(Target::Settings)));
            self.said.button(lay(target), "Settings");
        }
        Ok(())
    }

    // -- The floating bar ------------------------------------------------

    fn nav_places(&mut self, m: &Metrics, selected: Tab) -> NavPlaces {
        let bw = (m.w - 32.0).min(420.0);
        let bar = rect((m.w - bw) / 2.0, m.nav_top, bw, NAV_H);
        let pad = 7.0;
        let inner = NAV_H - pad * 2.0;
        let button = rect(bar.x + bw - pad - inner, bar.y + pad, inner, inner);
        let capsule = rect(
            bar.x + pad,
            bar.y + pad,
            button.x - 8.0 - (bar.x + pad),
            inner,
        );
        let label = 14.0 * m.scale.min(1.25);
        let lw = self.text.width(selected.name(), label, 600);
        let inset = 5.0;
        let room = capsule.width - inset * 2.0;
        let pill_w = (lw + 22.0 + 8.0 + 36.0).min(room - 2.0 * 44.0);
        let other = (room - pill_w) / 2.0;
        let mut slots = [rect(0.0, 0.0, 0.0, 0.0); 3];
        let mut sx = capsule.x + inset;
        for (k, tab) in TABS.into_iter().enumerate() {
            let w = if tab == selected { pill_w } else { other };
            slots[k] = rect(sx, capsule.y + inset, w, capsule.height - inset * 2.0);
            sx += w;
        }
        NavPlaces {
            bar,
            capsule,
            button,
            slots,
        }
    }

    fn draw_nav(&mut self, m: &Metrics, t: &Theme) -> Drawn {
        let canvas = self.canvas;
        let look = nav_look(t);
        let from = self.nav_places(m, self.tab_from);
        let to = self.nav_places(m, self.tab);
        let p = progress(self.now, self.tab_since, TAB_NS);
        let lerp = |a: gfx::Rect, b: gfx::Rect| {
            rect(
                a.x + (b.x - a.x) * p,
                a.y,
                a.width + (b.width - a.width) * p,
                a.height,
            )
        };
        let bar = to.bar;
        // The one soft shadow, made once for this size.
        if !t.dark {
            let key = (bar.width as u32, bar.height as u32);
            let shadow = match self.nav_shadow {
                Some((k, handle)) if k == key => Some(handle),
                _ => {
                    if let Some((_, old)) = self.nav_shadow.take() {
                        canvas_images::forget(canvas, old);
                    }
                    let handle =
                        keep_shadow(canvas, bar.width, bar.height, NAV_H / 2.0, 22.0, 0.13);
                    self.nav_shadow = handle.map(|h| (key, h));
                    handle
                }
            };
            if let Some(shadow) = shadow {
                canvas_images::draw(
                    canvas,
                    shadow,
                    rect(
                        bar.x - 22.0,
                        bar.y - 22.0 + 6.0,
                        bar.width + 44.0,
                        bar.height + 44.0,
                    ),
                )?;
            }
        }
        canvas2d::fill_round_rect(canvas, bar, radii(NAV_H / 2.0), look.bar)?;
        canvas2d::stroke_round_rect(canvas, bar, radii(NAV_H / 2.0), 1.0, t.hair)?;
        let cap = to.capsule;
        canvas2d::fill_round_rect(canvas, cap, radii(cap.height / 2.0), look.capsule)?;

        let tab_index = self.tab.slot();
        let pill = lerp(from.slots[self.tab_from.slot()], to.slots[tab_index]);
        canvas2d::fill_round_rect(canvas, pill, radii(pill.height / 2.0), look.pill)?;
        let label_size = 14.0 * m.scale.min(1.25);
        for (k, tab) in TABS.into_iter().enumerate() {
            let slot = lerp(from.slots[k], to.slots[k]);
            let on = tab == self.tab;
            let icon = match tab {
                Tab::Library => Icon::Layers,
                Tab::Discover => Icon::Compass,
                Tab::You => Icon::Person,
            };
            let cy = slot.y + slot.height / 2.0;
            // Something waits for the person (a change to choose, a device
            // to say yes to): a dot on You.
            let badge = tab == Tab::You && (self.sync.waiting() > 0 || self.code.is_some());
            if on {
                // The icon and its name, the name fading in as the pill
                // arrives.
                let lw = self.text.width(tab.name(), label_size, 600);
                let group = 22.0 + 8.0 + lw;
                let gx = pill.x + (pill.width - group) / 2.0;
                self.draw_icon(icon, look.on, gx + 11.0, cy, 22.0, 1.8)?;
                if badge {
                    canvas2d::fill_circle(canvas, pt(gx + 20.0, cy - 8.5), 6.0, look.pill)?;
                    canvas2d::fill_circle(canvas, pt(gx + 20.0, cy - 8.5), 4.5, t.danger)?;
                }
                canvas2d::set_clip(canvas, pill.x, pill.y, pill.width, pill.height)?;
                self.text.draw(
                    tab.name(),
                    gx + 30.0,
                    mid(slot.y, slot.height, label_size),
                    label_size,
                    with_alpha(look.on, p),
                    600,
                )?;
                canvas2d::clear_clip(canvas)?;
            } else {
                let cx = slot.x + slot.width / 2.0;
                self.draw_icon(icon, look.off, cx, cy, 22.0, 1.6)?;
                if badge {
                    canvas2d::fill_circle(canvas, pt(cx + 9.0, cy - 8.5), 6.0, look.capsule)?;
                    canvas2d::fill_circle(canvas, pt(cx + 9.0, cy - 8.5), 4.5, t.danger)?;
                }
            }
            // A generous target: the whole height of the bar.
            let target = rect(to.slots[k].x, bar.y, to.slots[k].width, bar.height);
            self.hits.push((target, Hit::Tab(tab)));
            if badge {
                self.said.button(
                    lay(target),
                    &format!("{}, something waits for you", tab.name()),
                );
            } else {
                self.said.button(lay(target), tab.name());
            }
        }

        // Open a file: the round button at the end.
        let held = self.pressed == Some(Hit::Files);
        let b = if held {
            shrink(to.button, 0.94)
        } else {
            to.button
        };
        let (bx, by) = (b.x + b.width / 2.0, b.y + b.height / 2.0);
        canvas2d::fill_circle(canvas, pt(bx, by), b.width / 2.0, look.button)?;
        self.draw_icon(Icon::Crate, look.button_ink, bx, by, 22.0, 1.6)?;
        let target = rect(to.button.x - 4.0, bar.y, to.button.width + 8.0, bar.height);
        self.hits.push((target, Hit::Files));
        self.said.button(lay(target), "Open a file");
        Ok(())
    }

    // -- Pieces --------------------------------------------------------------

    /// An app's tile: its picture, or Krate's app icon on a quiet square,
    /// with a hairline edge. People and organisations may wear a colour and
    /// an initial.
    fn draw_tile(
        &mut self,
        area: gfx::Rect,
        face: &Face,
        name: &str,
        t: &Theme,
        round: bool,
    ) -> Drawn {
        let canvas = self.canvas;
        let r = if round {
            area.width / 2.0
        } else {
            area.width * 0.23
        };
        match face.tile {
            Some(handle) => canvas_images::draw_round(canvas, handle, area, radii(r))?,
            None => {
                let quiet = if t.dark { hex(0x1f1f23) } else { hex(0xf5f5f7) };
                canvas2d::fill_round_rect(canvas, area, radii(r), quiet)?;
                if let Some(mark) = self.mark {
                    let s = area.width * 0.5;
                    canvas_images::draw(
                        canvas,
                        mark,
                        rect(
                            area.x + (area.width - s) / 2.0,
                            area.y + (area.height - s) / 2.0,
                            s,
                            s,
                        ),
                    )?;
                }
            }
        }
        if face.initial && face.tile.is_some() {
            let size = area.width * 0.42;
            let letter = initial(name);
            let lw = self.text.width(&letter, size, 650);
            self.text.draw(
                &letter,
                area.x + (area.width - lw) / 2.0,
                mid(area.y, area.height, size),
                size,
                rgba(1.0, 1.0, 1.0, 0.96),
                650,
            )?;
        }
        canvas2d::stroke_round_rect(canvas, area, radii(r), 1.0, t.hair)
    }

    /// How far a download of hub app `i` has got, if it is under way.
    fn download(&self, i: usize) -> Option<u32> {
        let id = &self.hub[i].id;
        self.progress.iter().find(|(d, _)| d == id).map(|(_, p)| *p)
    }

    /// A pill with a word on it, right-aligned at `right`, centred on `cy`:
    /// soft in a list, solid where it is the page's one action. A download
    /// under way shows how far it has got instead.
    #[allow(clippy::too_many_arguments)]
    fn pill(
        &mut self,
        m: &Metrics,
        right: f32,
        cy: f32,
        word: &str,
        t: &Theme,
        held: bool,
        solid: bool,
        done: Option<u32>,
    ) -> Drawn<gfx::Rect> {
        let size = m.button;
        let ww = self.text.width(word, size, 650);
        let ph = (size + 15.0).max(30.0);
        let pw = (ww + 30.0).max(68.0);
        let area = rect(right - pw, cy - ph / 2.0, pw, ph);
        let shown = if held { shrink(area, 0.97) } else { area };
        let r = shown.height / 2.0;
        let (fill, ink) = if solid {
            (t.accent, if t.dark { t.btn_ink } else { WHITE })
        } else {
            (t.accent_soft, t.accent)
        };
        if let Some(done) = done {
            canvas2d::fill_round_rect(self.canvas, shown, radii(r), t.accent_soft)?;
            let filled = shown.width * done as f32 / 100.0;
            if filled > 1.0 {
                canvas2d::set_clip(self.canvas, shown.x, shown.y, filled, shown.height)?;
                canvas2d::fill_round_rect(
                    self.canvas,
                    shown,
                    radii(r),
                    with_alpha(t.accent, 0.28),
                )?;
                canvas2d::clear_clip(self.canvas)?;
            }
            let label = format!("{done}%");
            let lw = self.text.width(&label, size * 0.92, 650);
            self.text.draw(
                &label,
                shown.x + (shown.width - lw) / 2.0,
                mid(shown.y, shown.height, size * 0.92),
                size * 0.92,
                t.accent,
                650,
            )?;
            return Ok(area);
        }
        canvas2d::fill_round_rect(self.canvas, shown, radii(r), fill)?;
        if held {
            canvas2d::fill_round_rect(self.canvas, shown, radii(r), t.press)?;
        }
        let size = if held { size * 0.97 } else { size };
        let ww = if held { ww * 0.97 } else { ww };
        self.text.draw(
            word,
            shown.x + (shown.width - ww) / 2.0,
            mid(shown.y, shown.height, size),
            size,
            ink,
            650,
        )?;
        Ok(area)
    }

    /// A black (in dark mode, white) button across `w`, centred at `cx`.
    #[allow(clippy::too_many_arguments)]
    fn big_button(
        &mut self,
        m: &Metrics,
        t: &Theme,
        cx: f32,
        y: f32,
        w: f32,
        word: &str,
        hit: Hit,
    ) -> Drawn<f32> {
        let bh = (m.button + 25.0).max(48.0);
        let area = rect(cx - w / 2.0, y, w, bh);
        let held = self.pressed == Some(hit);
        let shown = if held { shrink(area, 0.97) } else { area };
        canvas2d::fill_round_rect(self.canvas, shown, radii(shown.height / 2.0), t.btn)?;
        if held {
            canvas2d::fill_round_rect(
                self.canvas,
                shown,
                radii(shown.height / 2.0),
                rgba(0.5, 0.5, 0.5, 0.2),
            )?;
        }
        let lw = self.text.width(word, m.button, 650);
        self.text.draw(
            word,
            cx - lw / 2.0,
            mid(y, bh, m.button),
            m.button,
            t.btn_ink,
            650,
        )?;
        self.content_hit(m, area, hit);
        self.said.button(lay(area), word);
        Ok(y + bh)
    }

    /// How hidden a list's `order`-th row still is as the list first shows.
    fn veil(&self, order: usize) -> f32 {
        let seen = self.seen[self.tab.slot()];
        if seen == 0 || !self.stack.is_empty() {
            return 0.0;
        }
        1.0 - progress(self.now, seen + order as u64 * ROW_STEP_NS, ROWS_NS)
    }

    /// One app in a list: tile, words, pill. A tap on the row opens its
    /// page; a tap on the pill does what the pill says. Words wrap rather
    /// than being cut. Returns the row's height.
    #[allow(clippy::too_many_arguments)]
    fn draw_row(
        &mut self,
        m: &Metrics,
        t: &Theme,
        y: f32,
        order: usize,
        page: Target,
        action: Hit,
        last: bool,
        face: Face,
        name: &str,
        lines: [&str; 2],
        word: &str,
        done: Option<u32>,
        veil_ink: gfx::Color,
    ) -> Drawn<f32> {
        let tall = !lines[1].is_empty();
        let tile: f32 = if tall { 60.0 } else { 56.0 };
        let (x, col) = (m.x, m.col);
        let ww = self.text.width(word, m.button, 650);
        let pw = (ww + 30.0).max(68.0);
        let tx = x + tile + 14.0;
        let text_w = (x + col - pw - 14.0 - tx).max(40.0);
        let names = self.text.wrap(name, m.name, 600, text_w, 2);
        let first = self.text.wrap(lines[0], m.body, 400, text_w, 3);
        let second = self.text.wrap(lines[1], m.small, 400, text_w, 2);
        let words_h = names.len() as f32 * m.name * 1.22
            + first.len() as f32 * m.body * 1.32
            + second.len() as f32 * m.small * 1.32
            + if second.is_empty() { 0.0 } else { 2.0 };
        let row_h = (tile + 20.0).max(words_h + 26.0);
        let row = rect(x, y, col, row_h);
        let pill_area = rect(x + col - pw, y + row_h / 2.0 - 15.0, pw, 30.0);
        if !Self::visible(m, y, row_h) {
            self.content_hit(m, row, Hit::Show(page));
            self.content_hit(m, pill_area, action);
            return Ok(row_h);
        }
        let canvas = self.canvas;
        if self.pressed == Some(Hit::Show(page)) {
            canvas2d::fill_round_rect(
                canvas,
                rect(x - 10.0, y + 2.0, col + 20.0, row_h - 4.0),
                radii(14.0),
                t.press,
            )?;
        }
        self.draw_tile(
            rect(x, y + (row_h - tile) / 2.0, tile, tile),
            &face,
            name,
            t,
            false,
        )?;
        let held = self.pressed == Some(action);
        let pill_area = self.pill(m, x + col, y + row_h / 2.0, word, t, held, false, done)?;
        let mut ty = y + (row_h - words_h) / 2.0;
        for line in &names {
            ty += m.name * 1.22;
            self.text
                .draw(line, tx, ty - m.name * 0.25, m.name, t.ink, 600)?;
        }
        for line in &first {
            ty += m.body * 1.32;
            self.text
                .draw(line, tx, ty - m.body * 0.28, m.body, t.ink2, 400)?;
        }
        if !second.is_empty() {
            ty += 2.0;
        }
        for line in &second {
            ty += m.small * 1.32;
            self.text
                .draw(line, tx, ty - m.small * 0.28, m.small, t.ink3, 400)?;
        }
        if !last {
            canvas2d::fill_rect(canvas, rect(tx, y + row_h - 0.5, x + col - tx, 0.5), t.line)?;
        }
        let veil = self.veil(order);
        if veil > 0.01 {
            canvas2d::fill_rect(
                canvas,
                rect(x - 12.0, y, col + 24.0, row_h),
                with_alpha(veil_ink, veil),
            )?;
        }
        self.content_hit(m, row, Hit::Show(page));
        self.content_hit(m, pill_area, action);
        let mut said = name.to_string();
        for line in lines.iter().filter(|l| !l.trim().is_empty()) {
            said.push_str(", ");
            said.push_str(line);
        }
        self.said.button(lay(rect(tx, y, text_w, row_h)), &said);
        self.said.button(lay(pill_area), &format!("{word} {name}"));
        Ok(row_h)
    }

    /// A caption over a group, with an aside on the right.
    fn caption(&mut self, m: &Metrics, t: &Theme, y: f32, title: &str, aside: &str) -> Drawn<f32> {
        if title.is_empty() {
            return Ok(y);
        }
        let size = m.small;
        let base = y + 12.0 + size;
        if Self::visible(m, y, 30.0) {
            self.text.draw(title, m.x + 4.0, base, size, t.ink3, 600)?;
            if !aside.is_empty() {
                let aw = self.text.width(aside, size, 500);
                self.text
                    .draw(aside, m.x + m.col - 4.0 - aw, base, size, t.ink3, 500)?;
            }
            self.said
                .heading(layout::Rect::new(m.x, y, m.col, size * 2.0), title);
        }
        Ok(base + 8.0)
    }

    /// A group of rows on its own card, iOS Settings style, with a caption
    /// above and a note below. A value too long to sit beside its label
    /// moves under it. Returns where the next group starts.
    #[allow(clippy::too_many_arguments)]
    fn group(
        &mut self,
        m: &Metrics,
        t: &Theme,
        y: f32,
        title: &str,
        aside: &str,
        rows: &[Row],
        note: &str,
    ) -> Drawn<f32> {
        let canvas = self.canvas;
        let (x, col) = (m.x, m.col);
        let mut y = self.caption(m, t, y, title, aside)?;
        let label_size = m.name * 0.96;
        // Lay every row out first: its lines, and where its value goes.
        struct Laid {
            tx: f32,
            labels: Vec<String>,
            subs: Vec<String>,
            value: Option<String>,
            h: f32,
        }
        let mut laid: Vec<Laid> = Vec::new();
        for row in rows {
            let lead = match row.lead {
                Lead::Tile(_, _, side, _) => side + 12.0,
                Lead::Icon(_) | Lead::Glyph(_) => 30.0 + 12.0,
                Lead::None => 0.0,
            };
            let tx = x + 16.0 + lead;
            let mut right = x + col - 16.0;
            match &row.trail {
                Trail::Nav | Trail::ValueNav(_) => right -= 18.0,
                Trail::Switch(_) => right -= SWITCH_W + 12.0,
                Trail::Button(word) => right -= self.button_w(m, word) + 12.0,
                _ => {}
            }
            let room = right - tx;
            let weight = if row.tone == Tone::Ink || row.tone == Tone::Quiet {
                400
            } else {
                500
            };
            let value = match &row.trail {
                Trail::Value(v) | Trail::ValueNav(v) if !v.is_empty() => Some(v.clone()),
                _ => None,
            };
            let label_w = self.text.width(&row.label, label_size, weight);
            let value_w = value
                .as_ref()
                .map(|v| self.text.width(v, m.body, 400))
                .unwrap_or(0.0);
            let beside = value.is_none() || label_w + 14.0 + value_w <= room;
            let label_room = if beside && value.is_some() {
                room - value_w - 14.0
            } else {
                room
            };
            let labels = self
                .text
                .wrap(&row.label, label_size, weight, label_room, 3);
            let mut sub_text = row.sub.clone();
            let value = if beside {
                value
            } else {
                // Too long to sit beside: it reads under the label.
                if let Some(v) = value {
                    if sub_text.is_empty() {
                        sub_text = v;
                    } else {
                        sub_text = format!("{sub_text} \u{00b7} {v}");
                    }
                }
                None
            };
            let subs = self.text.wrap(&sub_text, m.small, 400, room, 3);
            let words =
                labels.len() as f32 * label_size * 1.25 + subs.len() as f32 * m.small * 1.35;
            let lead_h = match row.lead {
                Lead::Tile(_, _, side, _) => side + 16.0,
                Lead::Icon(_) | Lead::Glyph(_) => 30.0 + 16.0,
                Lead::None => 0.0,
            };
            laid.push(Laid {
                tx,
                labels,
                subs,
                value,
                h: (words + 24.0).max(50.0).max(lead_h),
            });
        }
        let total: f32 = laid.iter().map(|l| l.h).sum();
        if Self::visible(m, y, total) {
            canvas2d::fill_round_rect(canvas, rect(x, y, col, total), radii(14.0), t.group)?;
        }
        let mut ry = y;
        let count = rows.len();
        for (i, (row, l)) in rows.iter().zip(laid.iter()).enumerate() {
            let rh = l.h;
            let area = rect(x, ry, col, rh);
            if !Self::visible(m, ry, rh) {
                if let Some(hit) = row.hit {
                    self.content_hit(m, area, hit);
                }
                ry += rh;
                continue;
            }
            if row.hit.is_some() && self.pressed == row.hit {
                let top = if i == 0 { 14.0 } else { 0.0 };
                let bottom = if i + 1 == count { 14.0 } else { 0.0 };
                canvas2d::fill_round_rect(canvas, area, corners(top, bottom), t.press)?;
            }
            let lead_x = x + 16.0;
            match &row.lead {
                Lead::Tile(face, name, side, round) => {
                    let tile = rect(lead_x, ry + (rh - side) / 2.0, *side, *side);
                    self.draw_tile(tile, face, name, t, *round)?;
                }
                Lead::Icon(icon) => {
                    let sq = rect(lead_x, ry + (rh - 30.0) / 2.0, 30.0, 30.0);
                    canvas2d::fill_round_rect(canvas, sq, radii(8.0), t.accent_soft)?;
                    self.draw_icon(*icon, t.accent, sq.x + 15.0, sq.y + 15.0, 19.0, 1.7)?;
                }
                Lead::Glyph(glyph) => {
                    let sq = rect(lead_x, ry + (rh - 30.0) / 2.0, 30.0, 30.0);
                    canvas2d::fill_round_rect(canvas, sq, radii(8.0), t.accent_soft)?;
                    let gw = self.text.width(glyph, 14.0, 650);
                    self.text.draw(
                        glyph,
                        sq.x + 15.0 - gw / 2.0,
                        mid(sq.y, 30.0, 14.0),
                        14.0,
                        t.accent,
                        650,
                    )?;
                }
                Lead::None => {}
            }
            let mut right = x + col - 16.0;
            if matches!(row.trail, Trail::Nav | Trail::ValueNav(_)) {
                if let Some(chev) = self.chevron(t.ink3, false) {
                    canvas_images::draw(
                        canvas,
                        chev,
                        rect(right - 8.0, ry + (rh - 14.0) / 2.0, 8.0, 14.0),
                    )?;
                }
                right -= 18.0;
            }
            match &row.trail {
                Trail::Switch(on) => {
                    self.draw_switch(t, right - SWITCH_W, ry + rh / 2.0, *on)?;
                }
                Trail::Button(word) => {
                    let held = row.hit.is_some() && self.pressed == row.hit;
                    self.pill(m, right, ry + rh / 2.0, word, t, held, false, None)?;
                }
                _ => {}
            }
            if let Some(value) = &l.value {
                let vw = self.text.width(value, m.body, 400);
                self.text
                    .draw(value, right - vw, mid(ry, rh, m.body), m.body, t.ink3, 400)?;
            }
            let ink = match row.tone {
                Tone::Ink => t.ink,
                Tone::Quiet => t.ink3,
                Tone::Accent => t.accent,
                Tone::Danger => t.danger,
            };
            let weight = if row.tone == Tone::Ink || row.tone == Tone::Quiet {
                400
            } else {
                500
            };
            let words_h =
                l.labels.len() as f32 * label_size * 1.25 + l.subs.len() as f32 * m.small * 1.35;
            let mut ty = ry + (rh - words_h) / 2.0;
            for line in &l.labels {
                ty += label_size * 1.25;
                self.text
                    .draw(line, l.tx, ty - label_size * 0.27, label_size, ink, weight)?;
            }
            for line in &l.subs {
                ty += m.small * 1.35;
                self.text
                    .draw(line, l.tx, ty - m.small * 0.3, m.small, t.ink3, 400)?;
            }
            if i + 1 < count {
                canvas2d::fill_rect(
                    canvas,
                    rect(l.tx, ry + rh - 0.5, x + col - l.tx, 0.5),
                    t.line,
                )?;
            }
            let mut said = row.label.clone();
            if !row.sub.is_empty() {
                said.push_str(", ");
                said.push_str(&row.sub);
            }
            match &row.trail {
                Trail::Value(v) | Trail::ValueNav(v) if !v.is_empty() => {
                    said.push_str(", ");
                    said.push_str(v);
                }
                Trail::Switch(on) => said.push_str(if *on { ", on" } else { ", off" }),
                Trail::Button(word) => {
                    said.push_str(", ");
                    said.push_str(word);
                }
                _ => {}
            }
            match row.hit {
                Some(hit) => {
                    self.content_hit(m, area, hit);
                    self.said.button(lay(area), &said);
                }
                None => {
                    self.said.text(lay(area), &said);
                }
            }
            ry += rh;
        }
        y = ry;
        if !note.is_empty() {
            y += 8.0;
            for line in self.text.wrap(note, m.small, 400, col - 8.0, 5) {
                if Self::visible(m, y, m.small * 1.4) {
                    self.text
                        .draw(&line, x + 4.0, y + m.small, m.small, t.ink3, 400)?;
                    self.said
                        .text(layout::Rect::new(x, y, col, m.small * 1.4), &line);
                }
                y += m.small * 1.4;
            }
        }
        Ok(y + 24.0)
    }

    /// A page's own big title. Returns the bottom.
    fn page_title(&mut self, m: &Metrics, t: &Theme, y: f32, title: &str) -> Drawn<f32> {
        let size = 30.0 * m.scale.min(1.3);
        let lines = self.text.wrap(title, size, 700, m.col, 3);
        let mut y = y + 2.0;
        for line in &lines {
            if Self::visible(m, y, size * 1.3) {
                self.text.draw(line, m.x, y + size, size, t.ink, 700)?;
            }
            y += size * 1.18;
        }
        self.said.heading(
            lay(rect(
                m.x,
                y - lines.len() as f32 * size * 1.18,
                m.col,
                lines.len() as f32 * size * 1.18,
            )),
            title,
        );
        Ok(y + 14.0)
    }

    /// A centred title and a few lines under it. Returns the bottom.
    fn centred_words(
        &mut self,
        m: &Metrics,
        t: &Theme,
        mut y: f32,
        title: &str,
        body: &str,
    ) -> Drawn<f32> {
        let cx = m.x + m.col / 2.0;
        let head = 21.0 * m.scale.min(1.4);
        for line in self.text.wrap(title, head, 700, m.col.min(340.0), 2) {
            let tw = self.text.width(&line, head, 700);
            self.text
                .draw(&line, cx - tw / 2.0, y + head, head, t.ink, 700)?;
            self.said
                .heading(layout::Rect::new(cx - tw / 2.0, y, tw, head * 1.3), &line);
            y += head * 1.3;
        }
        y += 8.0;
        for line in self.text.wrap(body, m.body, 400, m.col.min(320.0), 5) {
            let lw = self.text.width(&line, m.body, 400);
            self.text
                .draw(&line, cx - lw / 2.0, y + m.body, m.body, t.ink2, 400)?;
            self.said
                .text(layout::Rect::new(cx - lw / 2.0, y, lw, m.body * 1.4), &line);
            y += m.body * 1.45;
        }
        Ok(y)
    }

    /// An icon on a soft disc, centred: the top of an empty or a help page.
    fn badge(&mut self, m: &Metrics, t: &Theme, y: f32, icon: Icon, d: f32) -> Drawn<f32> {
        let cx = m.x + m.col / 2.0;
        canvas2d::fill_circle(self.canvas, pt(cx, y + d / 2.0), d / 2.0, t.accent_soft)?;
        self.draw_icon(icon, t.accent, cx, y + d / 2.0, d * 0.46, 1.8)?;
        Ok(y + d + 22.0)
    }

    // -- Library -------------------------------------------------------------

    fn draw_library(&mut self, m: &Metrics, t: &Theme, mut y: f32, page: gfx::Color) -> Drawn<f32> {
        let count = self.library.len();
        let caption = if count == 1 {
            "1 app".to_string()
        } else {
            format!("{count} apps")
        };
        self.text
            .draw(&caption, m.x, y + m.small, m.small, t.ink3, 500)?;
        self.said
            .text(layout::Rect::new(m.x, y, m.col, m.small * 1.3), &caption);
        y += m.small + 6.0;
        for n in 0..count {
            let (face, name, line) = {
                let app = &self.library[n];
                (app.face, app.name.clone(), app.line.clone())
            };
            y += self.draw_row(
                m,
                t,
                y,
                n,
                Target::Lib(n),
                Hit::Open(n),
                n + 1 == count,
                face,
                &name,
                [&line, ""],
                "Open",
                None,
                page,
            )?;
        }
        Ok(y)
    }

    /// No apps yet: say how one arrives, and offer both ways.
    fn draw_empty_library(&mut self, m: &Metrics, t: &Theme, y: f32) -> Drawn<f32> {
        let canvas = self.canvas;
        let cx = m.x + m.col / 2.0;
        // The words sit at the middle of the space, the picture above them
        // and the two ways in below.
        let middle = (m.top + m.nav_top) / 2.0 - 12.0;
        let mut y = (middle - 98.0).max(y + 16.0);
        // Three tiles from the hub, fanned, or Krate's icon alone.
        let picks: Vec<(Face, String)> = self
            .hub
            .iter()
            .filter(|a| a.face.tile.is_some())
            .take(3)
            .map(|a| (a.face, a.name.clone()))
            .collect();
        if picks.len() == 3 {
            for (i, dx) in [(1usize, -54.0f32), (2, 54.0), (0, 0.0)] {
                let (face, name) = &picks[i];
                let (s, lift) = if dx == 0.0 { (68.0, 0.0) } else { (56.0, 12.0) };
                let area = rect(cx + dx - s / 2.0, y + lift, s, s);
                let ring = 4.0;
                canvas2d::fill_round_rect(
                    canvas,
                    rect(area.x - ring, area.y - ring, s + ring * 2.0, s + ring * 2.0),
                    radii(s * 0.23 + ring),
                    t.bg,
                )?;
                self.draw_tile(area, face, name, t, false)?;
            }
            y += 68.0 + 30.0;
        } else {
            self.draw_tile(rect(cx - 36.0, y, 72.0, 72.0), &Face::KRATE, "", t, false)?;
            y += 72.0 + 28.0;
        }
        y = self.centred_words(m, t, y, "No apps yet", EMPTY_BODY)?;
        y += 22.0;
        let bw = (self.text.width("Open a file", m.button, 650) + 48.0)
            .max(220.0)
            .min(m.col);
        y = self.big_button(m, t, cx, y, bw, "Open a file", Hit::Files)?;
        y += 6.0;
        let bh = (m.button + 25.0).max(48.0);
        let label = "Browse Discover";
        let lw = self.text.width(label, m.button, 600);
        let link = rect(cx - bw / 2.0, y, bw, bh);
        self.text.draw(
            label,
            cx - lw / 2.0,
            mid(y, bh, m.button),
            m.button,
            t.accent,
            600,
        )?;
        self.content_hit(m, link, Hit::Tab(Tab::Discover));
        self.said.button(lay(link), label);
        Ok(y + bh)
    }

    // -- Discover ------------------------------------------------------------

    /// Discover with nothing to list: loading, offline, or an error.
    fn draw_no_hub(&mut self, m: &Metrics, t: &Theme, y: f32) -> Drawn<f32> {
        if self.hub_state == HubState::Loading {
            return self.draw_skeleton(m, t, y);
        }
        let space = m.nav_top - m.top;
        let mut y = y + ((space - 300.0) * 0.4).max(16.0);
        y = self.badge(m, t, y, Icon::Compass, 72.0)?;
        let (title, body) = match &self.hub_state {
            HubState::Error(why) if !why.is_empty() => ("Discover could not load", why.clone()),
            HubState::Error(_) => (
                "Discover could not load",
                "Something went wrong on the way to the hub.".to_string(),
            ),
            _ => (
                "The hub is out of reach",
                "Apps people share show up here when this phone is online.".to_string(),
            ),
        };
        y = self.centred_words(m, t, y, title, &body)?;
        y += 22.0;
        let cx = m.x + m.col / 2.0;
        self.big_button(m, t, cx, y, 200.0_f32.min(m.col), "Try again", Hit::Refresh)
    }

    /// Placeholders the shape of Discover while the hub answers, breathing
    /// slowly.
    fn draw_skeleton(&mut self, m: &Metrics, t: &Theme, mut y: f32) -> Drawn<f32> {
        let canvas = self.canvas;
        let phase = (self.now / MS % 1600) as f32 / 1600.0;
        let wave = 0.5 - 0.5 * (phase * core::f32::consts::TAU).cos();
        let (low, high) = if t.dark {
            (hex(0x17171a), hex(0x222226))
        } else {
            (hex(0xe7e7ec), hex(0xf1f1f4))
        };
        let ink = mix(low, high, wave);
        let (x, col) = (m.x, m.col);
        let card_h = (col * 0.74)
            .clamp(250.0, 340.0)
            .min(((m.nav_top - m.top) * 0.6).max(160.0));
        canvas2d::fill_round_rect(canvas, rect(x, y, col, card_h), radii(20.0), ink)?;
        y += card_h + 30.0;
        canvas2d::fill_round_rect(canvas, rect(x, y, 96.0, 18.0), radii(6.0), ink)?;
        y += 34.0;
        for k in 0..5 {
            let row_h = 84.0;
            if y > m.nav_top {
                break;
            }
            canvas2d::fill_round_rect(canvas, rect(x, y + 12.0, 60.0, 60.0), radii(14.0), ink)?;
            let tx = x + 74.0;
            let widths = [0.46, 0.72, 0.34];
            for (j, wk) in widths.iter().enumerate() {
                let bw = (col - 74.0 - 90.0) * wk * if k % 2 == 0 { 1.0 } else { 0.85 };
                canvas2d::fill_round_rect(
                    canvas,
                    rect(tx, y + 18.0 + j as f32 * 18.0, bw, 11.0),
                    radii(5.5),
                    ink,
                )?;
            }
            canvas2d::fill_round_rect(
                canvas,
                rect(x + col - 68.0, y + 27.0, 68.0, 30.0),
                radii(15.0),
                ink,
            )?;
            y += row_h;
        }
        self.said.text(
            layout::Rect::new(x, m.top, col, 40.0),
            "Loading the apps on the hub",
        );
        Ok(y)
    }

    fn fade(&mut self, w: f32, h: f32) -> Option<u64> {
        let key = (((w / 8.0) as u32).max(2), ((h / 8.0) as u32).max(2));
        if let Some((_, fade)) = self.fades.iter().find(|(k, _)| *k == key) {
            return Some(*fade);
        }
        let fade = keep_fade(self.canvas, key.0, key.1, 0.86)?;
        if self.fades.len() > 6 {
            let (_, old) = self.fades.remove(0);
            canvas_images::forget(self.canvas, old);
        }
        self.fades.push((key, fade));
        Some(fade)
    }

    fn draw_featured(&mut self, m: &Metrics, t: &Theme, y: f32) -> Drawn<f32> {
        let canvas = self.canvas;
        let (x, col) = (m.x, m.col);
        let card_h = (col * 0.74)
            .clamp(250.0, 340.0)
            .min(((m.nav_top - m.top) * 0.72).max(180.0));
        let area = rect(x, y, col, card_h);
        let Some(feat) = self.featured.as_mut() else {
            return Ok(0.0);
        };
        let index = feat.index;
        if !Self::visible(m, y, card_h) {
            self.content_hit(m, area, Hit::Show(Target::Hub(index)));
            return Ok(card_h);
        }
        let corner = 22.0;
        let aspect = col / card_h;
        let ratio = (feat.raw.width as f32 / feat.raw.height as f32) / aspect;
        if ratio >= 0.7 {
            // Close enough in shape: the picture fills the card, cut from
            // its left so a title in the corner stays whole.
            let stale = feat
                .fill
                .is_none_or(|(_, a)| (a - aspect).abs() > aspect * 0.02);
            if stale {
                if let Some((old, _)) = feat.fill.take() {
                    canvas_images::forget(canvas, old);
                }
                let cut = region(&feat.raw, aspect, 0.0);
                let px = cut_out(&feat.raw, cut);
                feat.fill = canvas_images::keep(canvas, cut.2, cut.3, &px)
                    .ok()
                    .map(|h| (h, aspect));
            }
            if let Some((handle, _)) = feat.fill {
                canvas_images::draw_round(canvas, handle, area, radii(corner))?;
            }
        } else {
            // A phone-shaped picture stands whole on its own colour.
            if feat.backdrop.is_none() {
                feat.backdrop = keep_backdrop(canvas, feat.tint);
            }
            if feat.whole.is_none() {
                feat.whole =
                    canvas_images::keep(canvas, feat.raw.width, feat.raw.height, &feat.raw.rgba)
                        .ok();
            }
            match feat.backdrop {
                Some(b) => canvas_images::draw_round(canvas, b, area, radii(corner))?,
                None => canvas2d::fill_round_rect(canvas, area, radii(corner), t.soft)?,
            }
            let inner_h = card_h - 18.0 - card_h * 0.36;
            let inner_w = inner_h * feat.raw.width as f32 / feat.raw.height as f32;
            let inner = rect(x + col - 22.0 - inner_w, y + 18.0, inner_w, inner_h);
            if let Some(whole) = feat.whole {
                canvas2d::drop_shadow_round_rect(
                    canvas,
                    inner,
                    radii(10.0),
                    8.0,
                    rgba(0.0, 0.0, 0.0, 0.22),
                )?;
                canvas_images::draw_round(canvas, whole, inner, radii(10.0))?;
            }
        }
        let from = card_h * 0.36;
        if let Some(fade) = self.fade(col, card_h - from) {
            canvas_images::draw_round(
                canvas,
                fade,
                rect(x, y + from, col, card_h - from),
                corners(0.0, corner),
            )?;
        }
        canvas2d::stroke_round_rect(canvas, area, radii(corner), 1.0, t.hair)?;
        if self.pressed == Some(Hit::Show(Target::Hub(index))) {
            canvas2d::fill_round_rect(canvas, area, radii(corner), rgba(0.0, 0.0, 0.0, 0.12))?;
        }

        // The words, over the fade.
        let app = &self.hub[index];
        let (name, description, category) = (
            app.name.clone(),
            app.description.clone(),
            title_case(&app.category),
        );
        let done = self.download(index);
        let pad = 18.0;
        let size = m.button;
        let ww = self.text.width("Get", size, 650);
        let ph = (size + 15.0).max(30.0);
        let pw = (ww + 30.0).max(68.0);
        let pill = rect(x + col - pad - pw, y + card_h - pad - ph, pw, ph);
        let held = self.pressed == Some(Hit::Get(index));
        let shown = if held { shrink(pill, 0.97) } else { pill };
        canvas2d::fill_round_rect(canvas, shown, radii(shown.height / 2.0), WHITE)?;
        let label = match done {
            Some(d) => format!("{d}%"),
            None => "Get".to_string(),
        };
        let lw = self.text.width(&label, size, 650);
        self.text.draw(
            &label,
            shown.x + (shown.width - lw) / 2.0,
            mid(shown.y, shown.height, size),
            size,
            hex(0x3d6df0),
            650,
        )?;

        let text_w = pill.x - 14.0 - (x + pad);
        let name_size = 24.0 * m.scale.min(1.35);
        let lines = self.text.wrap(&description, m.body, 400, text_w, 2);
        let mut ty = y + card_h - pad - (lines.len().max(1) as f32 - 1.0) * m.body * 1.35;
        for line in &lines {
            self.text.draw(
                line,
                x + pad,
                ty - 2.0,
                m.body,
                rgba(1.0, 1.0, 1.0, 0.84),
                400,
            )?;
            ty += m.body * 1.35;
        }
        let mut ty = y + card_h - pad - lines.len() as f32 * m.body * 1.35 - 2.0;
        let shown = self.text.fit(&name, name_size, 700, text_w);
        self.text.draw(&shown, x + pad, ty, name_size, WHITE, 700)?;
        ty -= name_size * 1.12;
        let kicker = format!("Featured \u{00b7} {category}");
        self.text.draw(
            &kicker,
            x + pad,
            ty,
            m.small,
            rgba(1.0, 1.0, 1.0, 0.74),
            600,
        )?;

        self.content_hit(m, area, Hit::Show(Target::Hub(index)));
        self.content_hit(m, pill, Hit::Get(index));
        self.said
            .button(lay(area), &format!("Featured: {name}, {description}"));
        self.said.button(lay(pill), &format!("Get {name}"));
        Ok(card_h)
    }

    fn draw_discover(
        &mut self,
        m: &Metrics,
        t: &Theme,
        mut y: f32,
        page: gfx::Color,
    ) -> Drawn<f32> {
        if self.featured.is_some() {
            y += self.draw_featured(m, t, y)?;
            y += 8.0;
        }
        let sections = core::mem::take(&mut self.sections);
        let mut order = 1;
        for (category, apps) in &sections {
            y += 20.0;
            let head = 21.0 * m.scale.min(1.4);
            let title = title_case(category);
            if Self::visible(m, y, head * 2.0) {
                self.text.draw(&title, m.x, y + head, head, t.ink, 700)?;
                let count = if apps.len() == 1 {
                    "1 app".to_string()
                } else {
                    format!("{} apps", apps.len())
                };
                let cw = self.text.width(&count, m.small, 500);
                self.text
                    .draw(&count, m.x + m.col - cw, y + head, m.small, t.ink3, 500)?;
                self.said
                    .heading(layout::Rect::new(m.x, y, m.col, head * 1.3), &title);
            }
            y += head * 1.3 + 4.0;
            for (k, &i) in apps.iter().enumerate() {
                let (face, name, description, by) = {
                    let app = &self.hub[i];
                    (
                        app.face,
                        app.name.clone(),
                        app.description.clone(),
                        app.by.clone(),
                    )
                };
                let done = self.download(i);
                y += self.draw_row(
                    m,
                    t,
                    y,
                    order,
                    Target::Hub(i),
                    Hit::Get(i),
                    k + 1 == apps.len(),
                    face,
                    &name,
                    [&description, &by],
                    "Get",
                    done,
                    page,
                )?;
                order += 1;
            }
        }
        self.sections = sections;
        Ok(y)
    }

    // -- You ---------------------------------------------------------------

    fn draw_you(&mut self, m: &Metrics, t: &Theme, mut y: f32) -> Drawn<f32> {
        let canvas = self.canvas;
        let (x, col) = (m.x, m.col);
        if self.account.signed_in {
            // Who is signed in.
            let d = 64.0;
            let size = 21.0 * m.scale.min(1.35);
            let tx = x + 16.0 + d + 14.0;
            let room = col - (tx - x) - 16.0;
            let name = self.account.name.clone();
            let names = self.text.wrap(&name, size, 700, room, 2);
            let handle = if self.account.handle.is_empty() {
                String::new()
            } else {
                format!("@{}", self.account.handle)
            };
            let words = names.len() as f32 * size * 1.2
                + if handle.is_empty() { 0.0 } else { m.body * 1.4 };
            let card_h = (d + 32.0).max(words + 32.0);
            if Self::visible(m, y, card_h) {
                canvas2d::fill_round_rect(canvas, rect(x, y, col, card_h), radii(18.0), t.group)?;
                let face = self.account.avatar.unwrap_or(Face::KRATE);
                self.draw_tile(
                    rect(x + 16.0, y + (card_h - d) / 2.0, d, d),
                    &face,
                    &name,
                    t,
                    true,
                )?;
                let mut ty = y + (card_h - words) / 2.0;
                for line in &names {
                    ty += size * 1.2;
                    self.text
                        .draw(line, tx, ty - size * 0.22, size, t.ink, 700)?;
                }
                if !handle.is_empty() {
                    ty += m.body * 1.4;
                    let h = self.text.fit(&handle, m.body, 400, room);
                    self.text
                        .draw(&h, tx, ty - m.body * 0.3, m.body, t.ink3, 400)?;
                }
                self.said.heading(
                    lay(rect(x, y, col, card_h)),
                    &format!("Signed in as {name} {handle}"),
                );
            }
            y += card_h + 24.0;
            y = self.draw_waiting(m, t, y)?;

            let published: Vec<Row> = if self.account.published.is_empty() {
                alloc::vec![Row::new("Apps you publish show up here").quiet()]
            } else {
                self.account
                    .published
                    .iter()
                    .enumerate()
                    .map(|(p, app)| {
                        let (page, face) = match app.hub {
                            Some(i) => (Target::Hub(i), self.hub[i].face),
                            None => (Target::Pub(p), Face::KRATE),
                        };
                        Row::new(&app.name)
                            .sub(&app.line)
                            .tile(face, &app.name, 36.0, false)
                            .nav(Hit::Show(page))
                    })
                    .collect()
            };
            let count = if self.account.published.is_empty() {
                String::new()
            } else {
                format!("{}", self.account.published.len())
            };
            y = self.group(m, t, y, "Your published apps", &count, &published, "")?;
            let orgs: Vec<Row> = if self.account.orgs.is_empty() {
                alloc::vec![Row::new("Not part of an organisation yet").quiet()]
            } else {
                self.account
                    .orgs
                    .iter()
                    .map(|org| {
                        Row::new(&org.name)
                            .value(&org.role)
                            .tile(org.face, &org.name, 30.0, false)
                    })
                    .collect()
            };
            y = self.group(m, t, y, "Organisations", "", &orgs, "")?;
        } else {
            // Signed out: one calm card that says what signing in brings.
            let pad = 24.0;
            let d = 68.0;
            let body = "Your apps, organisations and data, on every device you use.";
            let lines = self
                .text
                .wrap(body, m.body, 400, (col - pad * 2.0).min(300.0), 4);
            let head = 21.0 * m.scale.min(1.4);
            let bh = (m.button + 25.0).max(48.0);
            let card_h = pad
                + d
                + 18.0
                + head * 1.3
                + 6.0
                + lines.len() as f32 * m.body * 1.45
                + 20.0
                + bh
                + pad;
            let cx = x + col / 2.0;
            if Self::visible(m, y, card_h) {
                canvas2d::fill_round_rect(canvas, rect(x, y, col, card_h), radii(20.0), t.group)?;
                canvas2d::fill_circle(canvas, pt(cx, y + pad + d / 2.0), d / 2.0, t.accent_soft)?;
                self.draw_icon(Icon::Person, t.accent, cx, y + pad + d / 2.0, 32.0, 1.8)?;
            }
            let mut ty = y + pad + d + 18.0;
            let title = "Sign in to Krate";
            let tw = self.text.width(title, head, 700);
            self.text
                .draw(title, cx - tw / 2.0, ty + head, head, t.ink, 700)?;
            self.said
                .heading(layout::Rect::new(cx - tw / 2.0, ty, tw, head * 1.3), title);
            ty += head * 1.3 + 6.0;
            for line in lines {
                let lw = self.text.width(&line, m.body, 400);
                self.text
                    .draw(&line, cx - lw / 2.0, ty + m.body, m.body, t.ink2, 400)?;
                self.said.text(
                    layout::Rect::new(cx - lw / 2.0, ty, lw, m.body * 1.4),
                    &line,
                );
                ty += m.body * 1.45;
            }
            ty += 20.0;
            let bw = (col - pad * 2.0).min(260.0);
            self.big_button(m, t, cx, ty, bw, "Sign in", Hit::SignIn)?;
            y += card_h + 24.0;
        }

        let mut more = Vec::new();
        if self.sync.known {
            more.push(
                Row::new("Sync across your devices")
                    .icon(Icon::Sync)
                    .value(&self.sync_word())
                    .nav(Hit::Show(Target::Sync)),
            );
        }
        more.push(
            Row::new("Settings")
                .icon(Icon::Cog)
                .nav(Hit::Show(Target::Settings)),
        );
        y = self.group(m, t, y, "", "", &more, "")?;
        if self.account.signed_in {
            let out = [Row::new("Sign out").action(Tone::Danger, Hit::SignOut)];
            y = self.group(m, t, y, "", "", &out, "")?;
        }
        Ok(y)
    }

    // -- Settings and its pages ------------------------------------------

    fn draw_settings(&mut self, m: &Metrics, t: &Theme, y: f32) -> Drawn<f32> {
        let mut y = self.page_title(m, t, y, "Settings")?;
        if self.sync.known {
            let rows = [Row::new("Sync across your devices")
                .icon(Icon::Sync)
                .value(&self.sync_word())
                .nav(Hit::Show(Target::Sync))];
            y = self.group(m, t, y, "Your devices", "", &rows, "")?;
        }
        let mut phone = Vec::new();
        if !self.account.device.is_empty() {
            phone.push(
                Row::new("This device")
                    .icon(Icon::Phone)
                    .value(&self.account.device),
            );
        }
        phone.push(
            Row::new("Storage")
                .icon(Icon::Disk)
                .value(&self.account.storage)
                .nav(Hit::Show(Target::Storage)),
        );
        y = self.group(m, t, y, "This phone", "", &phone, "")?;

        let appearance = if self.look.dark { "Dark" } else { "Light" };
        let size = format!("{:.0}%", self.look.text_scale * 100.0);
        let look = [
            Row::new("Appearance").icon(Icon::Sun).value(appearance),
            Row::new("Text size").glyph("Aa").value(&size),
        ];
        y = self.group(
            m,
            t,
            y,
            "Look",
            "",
            &look,
            "Krate follows your phone. Change dark mode and text size in your phone's settings.",
        )?;

        let opening = [Row::new("Apps from links and files")
            .icon(Icon::Link)
            .nav(Hit::Show(Target::Links))];
        y = self.group(m, t, y, "Opening apps", "", &opening, "")?;

        let help = [
            Row::new("How Krate keeps apps safe")
                .icon(Icon::Shield)
                .nav(Hit::Show(Target::Safety)),
            Row::new("Report a problem")
                .icon(Icon::Bubble)
                .action(Tone::Accent, Hit::Report),
        ];
        y = self.group(m, t, y, "Help", "", &help, "")?;

        let version = if self.account.version.is_empty() {
            "Krate".to_string()
        } else {
            self.account.version.clone()
        };
        let about = [Row::new("Version").icon(Icon::Layers).value(&version)];
        self.group(
            m,
            t,
            y,
            "About Krate",
            "",
            &about,
            "Krate is open source, under the MIT or Apache 2.0 licence.",
        )
    }

    fn draw_storage(&mut self, m: &Metrics, t: &Theme, y: f32) -> Drawn<f32> {
        let mut y = self.page_title(m, t, y, "Storage")?;
        // The total, large.
        if !self.account.storage.is_empty() {
            let size = 34.0 * m.scale.min(1.3);
            let card_h = size * 1.2 + m.body * 1.5 + 40.0;
            if Self::visible(m, y, card_h) {
                canvas2d::fill_round_rect(
                    self.canvas,
                    rect(m.x, y, m.col, card_h),
                    radii(16.0),
                    t.group,
                )?;
                let total = self.account.storage.clone();
                self.text
                    .draw(&total, m.x + 18.0, y + 18.0 + size, size, t.ink, 700)?;
                self.text.draw(
                    "Krate and your apps keep on this phone",
                    m.x + 18.0,
                    y + 18.0 + size * 1.2 + m.body * 1.2,
                    m.body,
                    t.ink2,
                    400,
                )?;
                self.said.text(
                    lay(rect(m.x, y, m.col, card_h)),
                    &format!("{total}: Krate and your apps keep on this phone"),
                );
            }
            y += card_h + 24.0;
        }
        // Each app as it counts toward the total above: the app itself and
        // what it keeps. Its data alone made a 12 MB total read as a list
        // of "None" and a few KB.
        let takes = |app: &crate::LibraryApp| line_size(&app.line) + bytes_of(&app.data);
        let mut order: Vec<usize> = (0..self.library.len()).collect();
        order.sort_by(|&a, &b| {
            takes(&self.library[b])
                .partial_cmp(&takes(&self.library[a]))
                .unwrap_or(core::cmp::Ordering::Equal)
        });
        let rows: Vec<Row> = if order.is_empty() {
            alloc::vec![Row::new("No apps kept yet").quiet()]
        } else {
            order
                .iter()
                .map(|&n| {
                    let app = &self.library[n];
                    Row::new(&app.name)
                        .tile(app.face, &app.name, 34.0, false)
                        .value(&human_bytes(takes(app)))
                        .nav(Hit::Show(Target::Lib(n)))
                })
                .collect()
        };
        self.group(
            m,
            t,
            y,
            "Apps, largest first",
            "",
            &rows,
            "Each app with what it keeps. Clear its data, or remove it, from its page.",
        )
    }

    /// A page of plain words under a badge: the help pages.
    fn draw_words(
        &mut self,
        m: &Metrics,
        t: &Theme,
        y: f32,
        icon: Icon,
        title: &str,
        points: &[(&str, &str)],
    ) -> Drawn<f32> {
        let canvas = self.canvas;
        let mut y = self.badge(m, t, y + 8.0, icon, 64.0)?;
        y = self.centred_words(m, t, y, title, "")?;
        y += 8.0;
        let pad = 18.0;
        let inner = m.col - pad * 2.0;
        let head = 16.5 * m.scale.min(1.4);
        let mut laid = Vec::new();
        let mut total = pad;
        for (k, (lead, body)) in points.iter().enumerate() {
            let heads = self.text.wrap(lead, head, 600, inner, 3);
            let bodies = self.text.wrap(body, m.body, 400, inner, 8);
            let h = heads.len() as f32 * head * 1.3 + 4.0 + bodies.len() as f32 * m.body * 1.45;
            total += h + if k + 1 < points.len() { 32.0 } else { pad };
            laid.push((heads, bodies, h));
        }
        if Self::visible(m, y, total) {
            canvas2d::fill_round_rect(canvas, rect(m.x, y, m.col, total), radii(16.0), t.group)?;
        }
        let mut py = y + pad;
        let count = laid.len();
        for (k, (heads, bodies, h)) in laid.into_iter().enumerate() {
            let mut ly = py;
            for line in &heads {
                self.text
                    .draw(line, m.x + pad, ly + head, head, t.ink, 600)?;
                ly += head * 1.3;
            }
            ly += 4.0;
            for line in &bodies {
                self.text
                    .draw(line, m.x + pad, ly + m.body, m.body, t.ink2, 400)?;
                ly += m.body * 1.45;
            }
            self.said.text(
                lay(rect(m.x, py, m.col, h)),
                &format!("{}. {}", points[k].0, points[k].1),
            );
            py += h;
            if k + 1 < count {
                canvas2d::fill_rect(canvas, rect(m.x + pad, py + 16.0, inner, 0.5), t.line)?;
                py += 32.0;
            }
        }
        Ok(y + total + 24.0)
    }

    // -- An app's page -------------------------------------------------------

    /// The top of an app's page: a big tile, the name, where it is from,
    /// and its one action. Returns the bottom.
    #[allow(clippy::too_many_arguments)]
    fn hero(
        &mut self,
        m: &Metrics,
        t: &Theme,
        y: f32,
        face: Face,
        name: &str,
        line: &str,
        word: &str,
        action: Hit,
        done: Option<u32>,
    ) -> Drawn<f32> {
        let side = if m.w < 600.0 { 96.0 } else { 108.0 };
        let (x, col) = (m.x, m.col);
        let tx = x + side + 16.0;
        let room = col - side - 16.0;
        let size = 22.0 * m.scale.min(1.35);
        let names = self.text.wrap(name, size, 700, room, 3);
        let lines = self.text.wrap(line, m.body, 400, room, 3);
        let ph = (m.button + 15.0).max(30.0);
        let words_h = names.len() as f32 * size * 1.18 + lines.len() as f32 * m.body * 1.38;
        let block = words_h + 12.0 + ph;
        let h = side.max(block);
        let ww = self.text.width(word, m.button, 650);
        let pw = (ww + 30.0).max(68.0);
        let top = y + (side - block).max(0.0) / 2.0;
        let pill_at = rect(tx, top + words_h + 12.0, pw, ph);
        if Self::visible(m, y, h) {
            self.draw_tile(rect(x, y, side, side), &face, name, t, false)?;
            let mut ty = top;
            for n in &names {
                ty += size * 1.18;
                self.text.draw(n, tx, ty - size * 0.2, size, t.ink, 700)?;
            }
            for l in &lines {
                ty += m.body * 1.38;
                self.text
                    .draw(l, tx, ty - m.body * 0.3, m.body, t.ink2, 400)?;
            }
            let held = self.pressed == Some(action);
            self.pill(m, tx + pw, pill_at.y + ph / 2.0, word, t, held, true, done)?;
            self.said
                .heading(lay(rect(tx, y, room, words_h)), &format!("{name}, {line}"));
            self.said.button(lay(pill_at), &format!("{word} {name}"));
        }
        self.content_hit(m, pill_at, action);
        Ok(y + h + 28.0)
    }

    fn draw_page(&mut self, m: &Metrics, t: &Theme, y: f32, target: Target) -> Drawn<f32> {
        match target {
            Target::Lib(n) => self.draw_library_page(m, t, y, n),
            Target::Hub(i) => self.draw_hub_page(m, t, y, i),
            Target::Pub(p) => {
                let (name, line) = {
                    let app = &self.account.published[p];
                    (app.name.clone(), app.line.clone())
                };
                let line = if line.is_empty() {
                    "Published by you".to_string()
                } else {
                    line
                };
                let y = self.hero(m, t, y, Face::KRATE, &name, &line, "Get", Hit::GetPub(p), None)?;
                let rows = [Row::new("Not listed on the hub yet").quiet()];
                self.group(m, t, y, "On the hub", "", &rows, "It shows in Discover once the hub lists it.")
            }
            Target::Settings => self.draw_settings(m, t, y),
            Target::Storage => self.draw_storage(m, t, y),
            Target::Sync => self.draw_sync(m, t, y),
            Target::Recover => self.draw_recover(m, t, y),
            Target::Safety => self.draw_words(
                m,
                t,
                y,
                Icon::Shield,
                "How Krate keeps apps safe",
                &[
                    (
                        "Every app runs sealed off",
                        "An app lives in its own space. It cannot see your other apps, or your photos and files, unless you hand it one.",
                    ),
                    (
                        "You see what it wants first",
                        "Before an app opens, Krate shows what it would use, and asks again each time it opens.",
                    ),
                    (
                        "The internet, only where it says",
                        "An app reaches only the addresses it names, and only once you allow it.",
                    ),
                    (
                        "Gone means gone",
                        "Removing an app deletes everything it saved on this phone.",
                    ),
                ],
            ),
            Target::Links => self.draw_words(
                m,
                t,
                y,
                Icon::Link,
                "Apps from links and files",
                &[
                    (
                        "A .krate is a whole app",
                        "One file holds everything an app needs. Tap one in Messages, Mail or Files and it opens in Krate.",
                    ),
                    (
                        "Links open here too",
                        "A link to an app on krate.tech opens it in Krate instead of the browser.",
                    ),
                    (
                        "Nothing opens unasked",
                        "Krate always shows what an app would use before it runs, wherever it came from.",
                    ),
                ],
            ),
        }
    }

    fn draw_library_page(&mut self, m: &Metrics, t: &Theme, y: f32, n: usize) -> Drawn<f32> {
        let (face, name, line, data, version) = {
            let app = &self.library[n];
            (
                app.face,
                app.name.clone(),
                app.line.clone(),
                app.data.clone(),
                app.version.clone(),
            )
        };
        let mut y = self.hero(m, t, y, face, &name, &line, "Open", Hit::Open(n), None)?;

        if let Some((_, perms)) = self.perms.clone().filter(|(k, _)| *k == n) {
            let rows: Vec<Row> = if perms.is_empty() {
                alloc::vec![Row::new("Nothing beyond its own window").quiet()]
            } else {
                perms
                    .iter()
                    .map(|(words, needed)| {
                        Row::new(words).value(if *needed { "Needed" } else { "Asked each time" })
                    })
                    .collect()
            };
            y = self.group(
                m,
                t,
                y,
                "What it can use",
                "",
                &rows,
                "Krate asks before every open. Change it on the permission card.",
            )?;
        }

        let mut data_rows = Vec::new();
        if !data.is_empty() {
            data_rows.push(Row::new("Kept on this phone").value(&data));
        }
        data_rows.push(Row::new("Clear data").action(Tone::Accent, Hit::Clear(n)));
        y = self.group(m, t, y, "Data", "", &data_rows, "")?;
        y = self.draw_app_sync(m, t, y, n)?;

        let mut version_rows = Vec::new();
        if !version.is_empty() {
            version_rows.push(Row::new("Version").value(&version));
        }
        version_rows.push(Row::new("Check for updates").action(Tone::Accent, Hit::Update(n)));
        y = self.group(m, t, y, "Version", "", &version_rows, "")?;

        let share = [Row::new("Share").action(Tone::Accent, Hit::Share(n))];
        y = self.group(m, t, y, "", "", &share, "")?;
        let remove = [Row::new("Remove from this phone").action(Tone::Danger, Hit::Remove(n))];
        self.group(m, t, y, "", "", &remove, "")
    }

    fn draw_hub_page(&mut self, m: &Metrics, t: &Theme, y: f32, i: usize) -> Drawn<f32> {
        let canvas = self.canvas;
        let (x, col) = (m.x, m.col);
        let (face, name, author, category, size, description) = {
            let app = &self.hub[i];
            (
                app.face,
                app.name.clone(),
                app.author.clone(),
                title_case(&app.category),
                app.size.clone(),
                app.description.clone(),
            )
        };
        let line = if author.is_empty() {
            "On the Krate hub".to_string()
        } else {
            author.clone()
        };
        let done = self.download(i);
        let mut y = self.hero(m, t, y, face, &name, &line, "Get", Hit::Get(i), done)?;

        // The facts in a row, the way a store shows them.
        let facts: Vec<(&str, String)> = [("Category", category), ("Size", size)]
            .into_iter()
            .filter(|(_, v)| !v.is_empty())
            .collect();
        if !facts.is_empty() {
            let cell = col / facts.len() as f32;
            let label = 12.0 * m.scale.min(1.4);
            let value = 16.0 * m.scale.min(1.4);
            let h = label * 1.4 + value * 1.4 + 24.0;
            if Self::visible(m, y, h) {
                canvas2d::fill_round_rect(canvas, rect(x, y, col, h), radii(16.0), t.group)?;
                for (k, (what, v)) in facts.iter().enumerate() {
                    let cx = x + cell * k as f32 + cell / 2.0;
                    if k > 0 {
                        canvas2d::fill_rect(
                            canvas,
                            rect(x + cell * k as f32, y + 14.0, 0.5, h - 28.0),
                            t.line,
                        )?;
                    }
                    let lw = self.text.width(what, label, 600);
                    self.text
                        .draw(what, cx - lw / 2.0, y + 12.0 + label, label, t.ink3, 600)?;
                    let shown = self.text.fit(v, value, 650, cell - 16.0);
                    let vw = self.text.width(&shown, value, 650);
                    self.text.draw(
                        &shown,
                        cx - vw / 2.0,
                        y + 12.0 + label * 1.4 + value,
                        value,
                        t.ink,
                        650,
                    )?;
                    self.said.text(
                        lay(rect(x + cell * k as f32, y, cell, h)),
                        &format!("{what}: {v}"),
                    );
                }
            }
            y += h + 24.0;
        }

        // The app as it runs.
        if let Some((_, shot, sw, sh)) = self.shot.filter(|s| s.0 == i) {
            let max_h = if m.w < 600.0 { 460.0 } else { 420.0 };
            let mut w = col;
            let mut h = col * sh as f32 / sw as f32;
            if h > max_h {
                h = max_h;
                w = h * sw as f32 / sh as f32;
            }
            let area = rect(x + (col - w) / 2.0, y, w, h);
            if Self::visible(m, y, h) {
                canvas_images::draw_round(canvas, shot, area, radii(18.0))?;
                canvas2d::stroke_round_rect(canvas, area, radii(18.0), 1.0, t.hair)?;
                self.said.image(lay(area), &format!("{name} as it runs"));
            }
            y += h + 28.0;
        }

        if !description.is_empty() {
            let head = 20.0 * m.scale.min(1.4);
            if Self::visible(m, y, head * 1.5) {
                self.text
                    .draw("About this app", x, y + head, head, t.ink, 700)?;
                self.said
                    .heading(layout::Rect::new(x, y, col, head * 1.3), "About this app");
            }
            y += head * 1.3 + 10.0;
            let size = m.body * 1.05;
            for line in self.text.wrap(&description, size, 400, col, 14) {
                if Self::visible(m, y, size * 1.5) {
                    self.text.draw(&line, x, y + size, size, t.ink2, 400)?;
                    self.said
                        .text(layout::Rect::new(x, y, col, size * 1.5), &line);
                }
                y += size * 1.5;
            }
            y += 10.0;
        }
        Ok(y)
    }

    // -- Sync across your devices ----------------------------------------

    /// How sync stands, in a word or two, beside its row.
    fn sync_word(&self) -> String {
        let choose = self.sync.conflicts.len();
        if choose > 0 && self.sync.state != SyncState::SignedOut {
            return format!("{choose} to choose");
        }
        match self.sync.state {
            SyncState::SignedOut => "Sign in",
            SyncState::Off => "Off",
            SyncState::Checking => "Checking",
            SyncState::On => "On",
            SyncState::Waiting => "Needs a yes",
            SyncState::Offline => "Offline",
            SyncState::Error => "Needs a look",
        }
        .to_string()
    }

    /// The width of a soft pill with `word` on it.
    fn button_w(&mut self, m: &Metrics, word: &str) -> f32 {
        (self.text.width(word, m.button, 650) + 30.0).max(68.0)
    }

    /// An on/off switch, its track's left edge at `x`, centred on `cy`.
    fn draw_switch(&mut self, t: &Theme, x: f32, cy: f32, on: bool) -> Drawn {
        let canvas = self.canvas;
        let track = rect(x, cy - SWITCH_H / 2.0, SWITCH_W, SWITCH_H);
        let off = if t.dark { hex(0x39393d) } else { hex(0xe3e3e8) };
        canvas2d::fill_round_rect(
            canvas,
            track,
            radii(SWITCH_H / 2.0),
            if on { t.accent } else { off },
        )?;
        let r = SWITCH_H / 2.0 - 2.0;
        let kx = if on {
            x + SWITCH_W - SWITCH_H / 2.0
        } else {
            x + SWITCH_H / 2.0
        };
        canvas2d::fill_circle(canvas, pt(kx, cy), r, WHITE)?;
        if !t.dark {
            canvas2d::stroke_circle(canvas, pt(kx, cy), r, 0.5, t.hair)?;
        }
        Ok(())
    }

    /// On You: what waits for the person -- the recovery code to write
    /// down, changes to choose between, devices to say yes to.
    fn draw_waiting(&mut self, m: &Metrics, t: &Theme, y: f32) -> Drawn<f32> {
        if !self.sync.known || self.sync.state == SyncState::SignedOut {
            return Ok(y);
        }
        let mut rows = Vec::new();
        if self.code.is_some() {
            rows.push(
                Row::new("Write down your recovery code")
                    .sub("Shown once. It lets a new device in when no other can.")
                    .icon(Icon::Shield)
                    .nav(Hit::Show(Target::Sync)),
            );
        }
        let choose = self.sync.conflicts.len();
        if choose > 0 {
            let mut apps: Vec<String> = Vec::new();
            for c in &self.sync.conflicts {
                if let Some(app) = self.library.get(c.app) {
                    if !apps.contains(&app.name) {
                        apps.push(app.name.clone());
                    }
                }
            }
            rows.push(
                Row::new(if choose == 1 {
                    "A change made on two devices"
                } else {
                    "Changes made on two devices"
                })
                .sub(&apps.join(", "))
                .icon(Icon::Sync)
                .value(&format!("{choose}"))
                .nav(Hit::Show(Target::Sync)),
            );
        }
        for d in self.sync.devices.iter().filter(|d| d.ask) {
            let name = if d.name.is_empty() {
                "A device"
            } else {
                d.name.as_str()
            };
            rows.push(
                Row::new(&format!("{name} wants to read your apps' data"))
                    .icon(Icon::Phone)
                    .nav(Hit::Show(Target::Sync)),
            );
        }
        if rows.is_empty() {
            return Ok(y);
        }
        self.group(m, t, y, "Waiting for you", "", &rows, "")
    }

    /// An app's page: its own switch and when it last synced.
    fn draw_app_sync(&mut self, m: &Metrics, t: &Theme, y: f32, n: usize) -> Drawn<f32> {
        if !self.sync.known {
            return Ok(y);
        }
        let state = self.sync.state;
        let mut rows = Vec::new();
        match self.sync.app(n).cloned() {
            _ if state == SyncState::SignedOut => rows.push(
                Row::new("Keep in sync")
                    .sub("Sign in to keep it the same on every device you use.")
                    .nav(Hit::Show(Target::Sync)),
            ),
            None | Some(AppSync::Nothing) => rows.push(
                Row::new("Nothing to sync")
                    .quiet()
                    .sub("This app keeps no data of its own."),
            ),
            Some(_) if state == SyncState::Off => rows.push(
                Row::new("Keep in sync")
                    .value("Off")
                    .sub("Sync is off for every app on this phone.")
                    .nav(Hit::Show(Target::Sync)),
            ),
            Some(AppSync::On(last)) => rows.push(
                Row::new("Keep in sync")
                    .sub(&if last.is_empty() {
                        "On. It syncs the next time you open it.".to_string()
                    } else {
                        format!("Last synced {last}")
                    })
                    .switch(true, Hit::SyncApp(n, false)),
            ),
            Some(AppSync::Off) => rows.push(
                Row::new("Keep in sync")
                    .sub("Its data stays on this phone.")
                    .switch(false, Hit::SyncApp(n, true)),
            ),
        }
        let choose = self.sync.conflicts.iter().filter(|c| c.app == n).count();
        if choose > 0 {
            rows.push(
                Row::new("Changed on two devices")
                    .action(Tone::Accent, Hit::Show(Target::Sync))
                    .value(&format!("{choose}"))
                    .nav(Hit::Show(Target::Sync)),
            );
        }
        self.group(m, t, y, "Sync", "", &rows, "")
    }

    /// A card that says how things stand, with the one thing to do.
    #[allow(clippy::too_many_arguments)]
    fn draw_notice(
        &mut self,
        m: &Metrics,
        t: &Theme,
        y: f32,
        icon: Icon,
        title: &str,
        body: &str,
        action: Option<(&str, Hit)>,
    ) -> Drawn<f32> {
        let canvas = self.canvas;
        let (x, col) = (m.x, m.col);
        let pad = 16.0;
        let lead = 30.0 + 12.0;
        let tx = x + pad + lead;
        let room = col - pad * 2.0 - lead;
        let head = m.name * 0.96;
        let titles = self.text.wrap(title, head, 600, room, 3);
        let bodies = self.text.wrap(body, m.body, 400, room, 10);
        let ph = (m.button + 15.0).max(30.0);
        let words_h = titles.len() as f32 * head * 1.3 + 4.0 + bodies.len() as f32 * m.body * 1.45;
        let h = pad + words_h + if action.is_some() { 14.0 + ph } else { 0.0 } + pad;
        if Self::visible(m, y, h) {
            canvas2d::fill_round_rect(canvas, rect(x, y, col, h), radii(14.0), t.group)?;
            let sq = rect(x + pad, y + pad, 30.0, 30.0);
            canvas2d::fill_round_rect(canvas, sq, radii(8.0), t.accent_soft)?;
            self.draw_icon(icon, t.accent, sq.x + 15.0, sq.y + 15.0, 19.0, 1.7)?;
            let mut ty = y + pad;
            for line in &titles {
                ty += head * 1.3;
                self.text
                    .draw(line, tx, ty - head * 0.27, head, t.ink, 600)?;
            }
            ty += 4.0;
            for line in &bodies {
                ty += m.body * 1.45;
                self.text
                    .draw(line, tx, ty - m.body * 0.35, m.body, t.ink2, 400)?;
            }
            self.said
                .text(lay(rect(x, y, col, h)), &format!("{title}. {body}"));
        }
        if let Some((word, hit)) = action {
            let pw = self.button_w(m, word);
            let held = self.pressed == Some(hit);
            let area = self.pill(
                m,
                tx + pw,
                y + h - pad - ph / 2.0,
                word,
                t,
                held,
                false,
                None,
            )?;
            self.content_hit(m, area, hit);
            self.said.button(lay(area), word);
        }
        Ok(y + h + 24.0)
    }

    /// The recovery code, shown once, until the person says they have it.
    fn draw_code(&mut self, m: &Metrics, t: &Theme, y: f32, code: &str) -> Drawn<f32> {
        let canvas = self.canvas;
        let (x, col) = (m.x, m.col);
        let pad = 18.0;
        let inner = col - pad * 2.0;
        let head = 18.0 * m.scale.min(1.35);
        let body = "Write it down and keep it somewhere safe. If none of your devices can let a new one in, this code is the only way to your apps' data. Krate shows it only this once.";
        let bodies = self.text.wrap(body, m.body, 400, inner, 8);
        let groups: Vec<&str> = code.split('-').filter(|g| !g.is_empty()).collect();
        let size = 21.0 * m.scale.min(1.3);
        let mut lines: Vec<String> = Vec::new();
        for chunk in groups.chunks(3) {
            lines.push(chunk.join("  "));
        }
        let code_h = lines.len() as f32 * size * 1.5 + 24.0;
        let bh = (m.button + 25.0).max(48.0);
        let h = pad
            + head * 1.3
            + 6.0
            + bodies.len() as f32 * m.body * 1.45
            + 16.0
            + code_h
            + 18.0
            + bh
            + pad;
        if Self::visible(m, y, h) {
            canvas2d::fill_round_rect(canvas, rect(x, y, col, h), radii(16.0), t.group)?;
            let mut ty = y + pad + head;
            self.text
                .draw("Your recovery code", x + pad, ty, head, t.ink, 700)?;
            ty += 6.0;
            for line in &bodies {
                ty += m.body * 1.45;
                self.text
                    .draw(line, x + pad, ty - m.body * 0.3, m.body, t.ink2, 400)?;
            }
            let box_y = ty + 16.0;
            let well = if t.dark { t.surface } else { t.grouped };
            canvas2d::fill_round_rect(
                canvas,
                rect(x + pad, box_y, inner, code_h),
                radii(12.0),
                well,
            )?;
            let mut cy = box_y + 12.0;
            for line in &lines {
                let lw = self.text.width(line, size, 650);
                cy += size * 1.5;
                self.text.draw(
                    line,
                    x + col / 2.0 - lw / 2.0,
                    cy - size * 0.4,
                    size,
                    t.ink,
                    650,
                )?;
            }
            self.said.text(
                lay(rect(x, y, col, box_y - y + code_h)),
                &format!("Your recovery code: {code}. {body}"),
            );
        }
        let by = y + h - pad - bh;
        self.big_button(
            m,
            t,
            x + col / 2.0,
            by,
            inner.min(320.0),
            "I've written it down",
            Hit::CodeKept,
        )?;
        Ok(y + h + 24.0)
    }

    /// One change made on two devices: both sides, and the choices.
    fn draw_conflict(&mut self, m: &Metrics, t: &Theme, y: f32, i: usize) -> Drawn<f32> {
        let canvas = self.canvas;
        let c = self.sync.conflicts[i].clone();
        let (app, face) = match self.library.get(c.app) {
            Some(app) => (app.name.clone(), app.face),
            None => ("An app".to_string(), Face::KRATE),
        };
        let (x, col) = (m.x, m.col);
        let pad = 16.0;
        let inner = col - pad * 2.0;
        let tile = 36.0;
        let head = m.name * 0.98;
        let title = match (c.mine.is_some(), c.theirs.is_some()) {
            (true, true) => format!("{app} changed on two devices"),
            (false, _) => format!("{app}: deleted here, changed on {}", c.device),
            (true, false) => format!("{app}: deleted on {}, changed here", c.device),
        };
        let room = inner - tile - 12.0;
        let titles = self.text.wrap(&title, head, 600, room, 3);
        let key = format!("\u{201c}{}\u{201d}", c.key);
        let keys = self.text.wrap(&key, m.small, 400, room, 2);
        let top_h =
            (titles.len() as f32 * head * 1.28 + keys.len() as f32 * m.small * 1.4).max(tile);

        // Both sides, in words.
        let sides = [
            ("This phone".to_string(), c.mine.clone()),
            (c.device.clone(), c.theirs.clone()),
        ];
        let mut laid = Vec::new();
        let mut sides_h = 0.0;
        for (label, value) in &sides {
            let labels = self.text.wrap(label, m.small, 600, inner - 24.0, 2);
            let values = match value {
                Some(v) => self.text.wrap(v, m.body, 400, inner - 24.0, 5),
                None => alloc::vec!["Deleted".to_string()],
            };
            let h = 12.0
                + labels.len() as f32 * m.small * 1.35
                + 4.0
                + values.len() as f32 * m.body * 1.42
                + 12.0;
            sides_h += h + 8.0;
            laid.push((labels, values, value.is_none(), h));
        }

        // The choices, two to a line.
        let mut choices: Vec<(String, Choice)> = Vec::new();
        let short = |name: &str, text: &mut Text| -> String {
            let label = format!("Keep {name}'s");
            if text.width(&label, m.button * 0.95, 600) <= inner / 2.0 - 4.0 - 16.0 {
                label
            } else {
                "Keep the other".to_string()
            }
        };
        match (c.mine.is_some(), c.theirs.is_some()) {
            (true, true) => {
                choices.push(("Keep this phone's".to_string(), Choice::This));
                choices.push((short(&c.device, &mut self.text), Choice::Other));
                choices.push(("Keep both".to_string(), Choice::Both));
                if c.merge {
                    choices.push(("Merge into one".to_string(), Choice::Merge));
                }
            }
            (false, _) => {
                choices.push(("Keep it deleted".to_string(), Choice::This));
                choices.push(("Keep the change".to_string(), Choice::Other));
            }
            (true, false) => {
                choices.push(("Keep the change".to_string(), Choice::This));
                choices.push(("Delete it".to_string(), Choice::Other));
            }
        }
        let bh = (m.button + 19.0).max(38.0);
        let rows_of = choices.len().div_ceil(2);
        let buttons_h = rows_of as f32 * (bh + 8.0) - 8.0;
        let more = if c.more > 0 {
            format!(
                "{} more {} changed it too.",
                c.more,
                if c.more == 1 { "device" } else { "devices" }
            )
        } else {
            String::new()
        };
        let more_h = if more.is_empty() {
            0.0
        } else {
            m.small * 1.6 + 4.0
        };
        let h = pad + top_h + 14.0 + sides_h + 6.0 + buttons_h + more_h + pad;

        if Self::visible(m, y, h) {
            canvas2d::fill_round_rect(canvas, rect(x, y, col, h), radii(16.0), t.group)?;
            self.draw_tile(rect(x + pad, y + pad, tile, tile), &face, &app, t, false)?;
            let tx = x + pad + tile + 12.0;
            let mut ty = y + pad;
            for line in &titles {
                ty += head * 1.28;
                self.text
                    .draw(line, tx, ty - head * 0.26, head, t.ink, 600)?;
            }
            for line in &keys {
                ty += m.small * 1.4;
                self.text
                    .draw(line, tx, ty - m.small * 0.3, m.small, t.ink3, 400)?;
            }
            self.said.heading(lay(rect(x, y, col, top_h + pad)), &title);
            let well = if t.dark { t.surface } else { t.grouped };
            let mut sy = y + pad + top_h + 14.0;
            for (k, (labels, values, deleted, sh)) in laid.iter().enumerate() {
                canvas2d::fill_round_rect(
                    canvas,
                    rect(x + pad, sy, inner, *sh),
                    radii(12.0),
                    well,
                )?;
                let mut ly = sy + 12.0;
                for line in labels {
                    ly += m.small * 1.35;
                    self.text.draw(
                        line,
                        x + pad + 12.0,
                        ly - m.small * 0.3,
                        m.small,
                        t.ink3,
                        600,
                    )?;
                }
                ly += 4.0;
                for line in values {
                    ly += m.body * 1.42;
                    self.text.draw(
                        line,
                        x + pad + 12.0,
                        ly - m.body * 0.32,
                        m.body,
                        if *deleted { t.ink3 } else { t.ink },
                        if *deleted { 500 } else { 400 },
                    )?;
                }
                let (label, value) = &sides[k];
                self.said.text(
                    lay(rect(x + pad, sy, inner, *sh)),
                    &format!("{label}: {}", value.as_deref().unwrap_or("deleted")),
                );
                sy += sh + 8.0;
            }
            if !more.is_empty() {
                let my = y + h - pad - m.small * 0.4;
                self.text.draw(&more, x + pad, my, m.small, t.ink3, 400)?;
            }
        }

        let by0 = y + pad + top_h + 14.0 + sides_h + 6.0;
        let half = (inner - 8.0) / 2.0;
        for (k, (word, choice)) in choices.iter().enumerate() {
            let row = k / 2;
            let alone = k + 1 == choices.len() && k % 2 == 0;
            let (bx, bw) = if alone {
                (x + pad, inner)
            } else {
                (x + pad + (k % 2) as f32 * (half + 8.0), half)
            };
            let area = rect(bx, by0 + row as f32 * (bh + 8.0), bw, bh);
            let hit = Hit::Resolve(i, *choice);
            if Self::visible(m, area.y, bh) {
                let held = self.pressed == Some(hit);
                let shown = if held { shrink(area, 0.97) } else { area };
                canvas2d::fill_round_rect(canvas, shown, radii(bh / 2.0), t.accent_soft)?;
                if held {
                    canvas2d::fill_round_rect(canvas, shown, radii(bh / 2.0), t.press)?;
                }
                let size = m.button * 0.95;
                let label = self.text.fit(word, size, 600, bw - 16.0);
                let lw = self.text.width(&label, size, 600);
                self.text.draw(
                    &label,
                    shown.x + (shown.width - lw) / 2.0,
                    mid(shown.y, shown.height, size),
                    size,
                    t.accent,
                    600,
                )?;
                self.said.button(lay(area), word);
            }
            self.content_hit(m, area, hit);
        }
        Ok(y + h + 12.0)
    }

    /// Settings -> Sync across your devices.
    fn draw_sync(&mut self, m: &Metrics, t: &Theme, y: f32) -> Drawn<f32> {
        let mut y = self.page_title(m, t, y, "Sync across your devices")?;
        let state = self.sync.state;
        if state == SyncState::SignedOut {
            y = self.badge(m, t, y + 8.0, Icon::Sync, 64.0)?;
            let words = if self.sync.words.is_empty() {
                "Sign in to keep your apps the same on every device you use.".to_string()
            } else {
                self.sync.words.clone()
            };
            y = self.centred_words(m, t, y, "The same apps, everywhere", &words)?;
            y += 22.0;
            let bw = (m.col - 48.0).min(260.0);
            y = self.big_button(m, t, m.x + m.col / 2.0, y, bw, "Sign in", Hit::SignIn)?;
            return Ok(y + 24.0);
        }
        if let Some(code) = self.code.clone() {
            y = self.draw_code(m, t, y, &code)?;
        }
        let choose = self.sync.conflicts.len();
        if choose > 0 {
            y = self.caption(m, t, y, "Changed on two devices", &format!("{choose}"))?;
            for i in 0..choose {
                y = self.draw_conflict(m, t, y, i)?;
            }
            for line in self.text.wrap(
                "Until you choose, both are kept and each device goes on using its own.",
                m.small,
                400,
                m.col - 8.0,
                3,
            ) {
                if Self::visible(m, y, m.small * 1.4) {
                    self.text
                        .draw(&line, m.x + 4.0, y + m.small, m.small, t.ink3, 400)?;
                    self.said
                        .text(layout::Rect::new(m.x, y, m.col, m.small * 1.4), &line);
                }
                y += m.small * 1.4;
            }
            y += 24.0;
        }

        let on = state != SyncState::Off;
        let switch = [Row::new("Sync your apps' data")
            .icon(Icon::Sync)
            .switch(on, Hit::SyncAll(!on))];
        let note = match state {
            SyncState::On | SyncState::Off => self.sync.words.clone(),
            _ => String::new(),
        };
        y = self.group(m, t, y, "", "", &switch, &note)?;
        let words = self.sync.words.clone();
        y = match state {
            SyncState::Waiting => self.draw_notice(
                m,
                t,
                y,
                Icon::Shield,
                "Waiting for a yes",
                &words,
                Some(("Use a recovery code", Hit::Show(Target::Recover))),
            )?,
            SyncState::Offline => self.draw_notice(
                m,
                t,
                y,
                Icon::Sync,
                "Out of reach for now",
                &words,
                Some(("Try again", Hit::SyncCheck)),
            )?,
            SyncState::Error => self.draw_notice(
                m,
                t,
                y,
                Icon::Bubble,
                "Sync needs a look",
                &words,
                Some(("Try again", Hit::SyncCheck)),
            )?,
            SyncState::Checking => {
                self.draw_notice(m, t, y, Icon::Sync, "Checking", &words, None)?
            }
            _ => y,
        };
        if state == SyncState::Off {
            return Ok(y);
        }

        let last = if self.sync.checking && self.sync.last.is_empty() {
            "Checking\u{2026}".to_string()
        } else if self.sync.last.is_empty() {
            "Not yet".to_string()
        } else {
            title_case(&self.sync.last)
        };
        let phone = [
            Row::new("Name").icon(Icon::Phone).value(&self.sync.device),
            Row::new("Last synced").icon(Icon::Sync).value(&last),
        ];
        y = self.group(m, t, y, "This phone", "", &phone, "")?;

        let devices: Vec<Row> = if self.sync.devices.is_empty() {
            alloc::vec![Row::new("No other devices yet").quiet()]
        } else {
            self.sync
                .devices
                .iter()
                .enumerate()
                .map(|(i, d)| {
                    let name = if d.name.is_empty() {
                        "A device".to_string()
                    } else {
                        d.name.clone()
                    };
                    let row = Row::new(&name).icon(Icon::Phone);
                    if d.ask {
                        row.sub("Waiting for your yes")
                            .button("Approve", Hit::Approve(i))
                    } else if d.reads {
                        row.sub("Reads your apps' data")
                    } else {
                        row.sub("Waiting for a yes from one of your devices")
                    }
                })
                .collect()
        };
        y = self.group(
            m,
            t,
            y,
            "Your other devices",
            "",
            &devices,
            "Sign in to Krate on your other phones and computers. Each new one waits for a yes from a device that already syncs.",
        )?;

        let apps: Vec<Row> = self
            .sync
            .apps
            .clone()
            .into_iter()
            .filter_map(|(n, app)| {
                let lib = self.library.get(n)?;
                let (sub, on) = match app {
                    AppSync::On(last) if last.is_empty() => ("Not synced yet".to_string(), true),
                    AppSync::On(last) => (format!("Last synced {last}"), true),
                    AppSync::Off => ("Stays on this phone".to_string(), false),
                    AppSync::Nothing => return None,
                };
                Some(
                    Row::new(&lib.name)
                        .tile(lib.face, &lib.name, 30.0, false)
                        .sub(&sub)
                        .switch(on, Hit::SyncApp(n, !on)),
                )
            })
            .collect();
        if !apps.is_empty() {
            y = self.group(
                m,
                t,
                y,
                "Apps",
                "",
                &apps,
                "Apps that keep no data of their own have nothing to sync.",
            )?;
        }
        Ok(y)
    }

    /// Typing the recovery code, when no other device can let this one in.
    fn draw_recover(&mut self, m: &Metrics, t: &Theme, y: f32) -> Drawn<f32> {
        let canvas = self.canvas;
        let mut y = self.badge(m, t, y + 8.0, Icon::Shield, 64.0)?;
        y = self.centred_words(
            m,
            t,
            y,
            "Use a recovery code",
            "Krate showed it once, on the first device you set up. Type it here and this phone can read your apps' data.",
        )?;
        y += 22.0;
        let (x, col) = (m.x, m.col);
        let size = m.body * 1.12;
        let h = (size + 32.0).max(52.0);
        let field_r = rect(x, y, col, h);
        if Self::visible(m, y, h) {
            canvas2d::fill_round_rect(canvas, field_r, radii(14.0), t.group)?;
            let (edge, width) = if self.typing {
                (t.accent, 1.5)
            } else {
                (t.hair, 1.0)
            };
            canvas2d::stroke_round_rect(canvas, field_r, radii(14.0), width, edge)?;
            let shown: String = {
                let chars: Vec<char> = self.typed.chars().collect();
                chars
                    .chunks(5)
                    .map(|c| c.iter().collect::<String>())
                    .collect::<Vec<_>>()
                    .join("-")
            };
            let tx = x + 16.0;
            let base = mid(y, h, size);
            let end = if shown.is_empty() {
                self.text
                    .draw("26 letters and numbers", tx, base, size, t.ink3, 400)?;
                tx
            } else {
                let fitted = self.text.fit(&shown, size, 600, col - 40.0);
                self.text.draw(&fitted, tx, base, size, t.ink, 600)?;
                tx + self.text.width(&fitted, size, 600)
            };
            if self.typing {
                canvas2d::fill_rect(
                    canvas,
                    rect(end + 1.5, y + (h - size * 1.25) / 2.0, 2.0, size * 1.25),
                    t.accent,
                )?;
            }
        }
        self.content_hit(m, field_r, Hit::CodeField);
        self.said.button(
            lay(field_r),
            &format!(
                "Recovery code, {} of {CODE_LEN} letters and numbers typed",
                self.typed.len()
            ),
        );
        y += h + 10.0;
        let count = self.typed.len();
        let (line, ink) = if self.code_short {
            (
                format!("That is {count} of {CODE_LEN}. A recovery code is {CODE_LEN} letters and numbers."),
                t.danger,
            )
        } else if count > 0 {
            (format!("{count} of {CODE_LEN}"), t.ink3)
        } else {
            (
                "Dashes and spaces do not matter, and neither do capitals.".to_string(),
                t.ink3,
            )
        };
        for l in self.text.wrap(&line, m.small, 400, col - 8.0, 3) {
            if Self::visible(m, y, m.small * 1.4) {
                self.text
                    .draw(&l, x + 4.0, y + m.small, m.small, ink, 400)?;
                self.said
                    .text(layout::Rect::new(x, y, col, m.small * 1.4), &l);
            }
            y += m.small * 1.4;
        }
        y += 18.0;
        let bw = col.min(320.0);
        y = self.big_button(m, t, x + col / 2.0, y, bw, "Use this code", Hit::CodeSubmit)?;
        Ok(y + 24.0)
    }

    // -- The sheet that asks --------------------------------------------------

    fn draw_ask(&mut self, m: &Metrics, t: &Theme, ask: Ask) -> Drawn {
        let canvas = self.canvas;
        let p = match self.sheet_anim {
            Some((true, since)) => progress(self.now, since, SHEET_NS),
            Some((false, since)) => 1.0 - progress(self.now, since, SHEET_NS),
            None => 1.0,
        };
        // Only the sheet answers now.
        self.hits.clear();
        self.said = Said::new();
        let dim = if t.dark { 0.55 } else { 0.32 };
        canvas2d::fill_rect(
            canvas,
            rect(0.0, 0.0, m.w, m.h),
            rgba(0.0, 0.0, 0.0, dim * p),
        )?;
        self.hits.push((rect(0.0, 0.0, m.w, m.h), Hit::No));

        let (title, body, word) = match ask {
            Ask::Remove(n) => (
                format!("Remove {}?", self.library[n].name),
                "The app and everything it saved are deleted from this phone.",
                "Remove",
            ),
            Ask::Clear(n) => (
                format!("Clear {}'s data?", self.library[n].name),
                if matches!(self.sync.app(n), Some(AppSync::On(_)))
                    && matches!(self.sync.state, SyncState::On | SyncState::Offline)
                {
                    "Everything it saved on this phone is deleted. The app stays, and your other devices keep their copy, which comes back here the next time it syncs."
                } else {
                    "Everything it saved on this phone is deleted. The app stays."
                },
                "Clear data",
            ),
            Ask::Approve(i) => (
                format!(
                    "Let {} read your apps' data?",
                    self.sync
                        .devices
                        .get(i)
                        .map(|d| d.name.as_str())
                        .filter(|n| !n.is_empty())
                        .unwrap_or("this device")
                ),
                "It will see everything your apps keep, the same as this phone does. Say yes only if it is yours and you signed in on it just now.",
                "Let it read",
            ),
        };
        let danger = !matches!(ask, Ask::Approve(_));
        let pad = 24.0;
        let compact = m.w < 600.0;
        let sw = if compact {
            (m.w - 16.0).min(440.0)
        } else {
            400.0
        };
        let inner = sw - pad * 2.0;
        let head = 19.0 * m.scale.min(1.35);
        let titles = self.text.wrap(&title, head, 700, inner, 2);
        let lines = self.text.wrap(body, m.body, 400, inner, 4);
        let bh = (m.button + 25.0).max(50.0);
        let sh = pad
            + titles.len() as f32 * head * 1.25
            + 6.0
            + lines.len() as f32 * m.body * 1.45
            + 22.0
            + bh * 2.0
            + 10.0
            + pad;
        let sx = (m.w - sw) / 2.0;
        let rest = if compact {
            m.h - m.safe_bottom.max(8.0) - 8.0 - sh
        } else {
            (m.h - sh) / 2.0
        };
        // Rising from below the screen into place.
        let sy = rest + (1.0 - p) * (m.h - rest + 20.0);
        let sheet = rect(sx, sy, sw, sh);
        canvas2d::fill_round_rect(canvas, sheet, radii(26.0), t.group)?;
        self.hits.push((sheet, Hit::Stay));
        let cx = sx + sw / 2.0;
        let mut y = sy + pad;
        for line in &titles {
            let lw = self.text.width(line, head, 700);
            self.text
                .draw(line, cx - lw / 2.0, y + head, head, t.ink, 700)?;
            y += head * 1.25;
        }
        self.said.heading(lay(rect(sx, sy, sw, y - sy)), &title);
        y += 6.0;
        let body_top = y;
        for line in &lines {
            let lw = self.text.width(line, m.body, 400);
            self.text
                .draw(line, cx - lw / 2.0, y + m.body, m.body, t.ink2, 400)?;
            y += m.body * 1.45;
        }
        self.said
            .text(lay(rect(sx, body_top, sw, y - body_top)), body);
        y += 22.0;
        let yes = rect(sx + pad, y, inner, bh);
        let held = self.pressed == Some(Hit::Yes);
        let shown = if held { shrink(yes, 0.97) } else { yes };
        let (fill, ink) = if danger {
            (hex(0xe5484d), WHITE)
        } else {
            (t.btn, t.btn_ink)
        };
        canvas2d::fill_round_rect(canvas, shown, radii(15.0), fill)?;
        if held {
            canvas2d::fill_round_rect(canvas, shown, radii(15.0), rgba(0.5, 0.5, 0.5, 0.18))?;
        }
        let lw = self.text.width(word, m.button, 650);
        self.text.draw(
            word,
            cx - lw / 2.0,
            mid(y, bh, m.button),
            m.button,
            ink,
            650,
        )?;
        self.hits.push((yes, Hit::Yes));
        self.said.button(lay(yes), word);
        y += bh + 10.0;
        let no = rect(sx + pad, y, inner, bh);
        let held = self.pressed == Some(Hit::No);
        let shown = if held { shrink(no, 0.97) } else { no };
        canvas2d::fill_round_rect(
            canvas,
            shown,
            radii(15.0),
            if t.dark { t.surface } else { t.soft },
        )?;
        let cancel = if danger { "Cancel" } else { "Not now" };
        let lw = self.text.width(cancel, m.button, 600);
        self.text.draw(
            cancel,
            cx - lw / 2.0,
            mid(y, bh, m.button),
            m.button,
            t.ink,
            600,
        )?;
        self.hits.push((no, Hit::No));
        self.said.button(lay(no), cancel);
        Ok(())
    }
}

fn icon_key(icon: Icon) -> u64 {
    icon as u64 & 0xff
}
