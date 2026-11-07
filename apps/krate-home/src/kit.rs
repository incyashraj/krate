//! The pieces every screen is drawn from: Krate's colours, text measured
//! once, and pictures kept at the size they are shown.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use krate::bindings::krate::fs::files;
use krate::gfx::{canvas2d, canvas_images, types as gfx};
use krate::layout::{self, Look};
use krate::math::F32Ext as _;

// ---------------------------------------------------------------------------
// Colour: Krate's own theme (docs/landing/kit/site.css), light and dark
// ---------------------------------------------------------------------------

pub(crate) const fn rgb(r: f32, g: f32, b: f32) -> gfx::Color {
    gfx::Color { r, g, b, a: 1.0 }
}

pub(crate) const fn rgba(r: f32, g: f32, b: f32, a: f32) -> gfx::Color {
    gfx::Color { r, g, b, a }
}

pub(crate) const fn hex(v: u32) -> gfx::Color {
    rgb(
        ((v >> 16) & 0xff) as f32 / 255.0,
        ((v >> 8) & 0xff) as f32 / 255.0,
        (v & 0xff) as f32 / 255.0,
    )
}

pub(crate) const WHITE: gfx::Color = rgb(1.0, 1.0, 1.0);

#[derive(Clone, Copy)]
pub(crate) struct Theme {
    pub dark: bool,
    /// Behind the lists: Library and Discover.
    pub bg: gfx::Color,
    /// Behind grouped settings: an app's page and You.
    pub grouped: gfx::Color,
    /// A group of rows on `grouped`.
    pub group: gfx::Color,
    /// A quiet fill: the segmented control's track, an empty tile.
    pub soft: gfx::Color,
    /// The selected segment.
    pub surface: gfx::Color,
    pub ink: gfx::Color,
    pub ink2: gfx::Color,
    pub ink3: gfx::Color,
    pub line: gfx::Color,
    /// The edge of a picture.
    pub hair: gfx::Color,
    pub accent: gfx::Color,
    pub accent_soft: gfx::Color,
    /// The one black (in dark mode, white) button.
    pub btn: gfx::Color,
    pub btn_ink: gfx::Color,
    /// What cannot be undone.
    pub danger: gfx::Color,
    /// Laid over something held down.
    pub press: gfx::Color,
}

pub(crate) fn theme(look: &Look) -> Theme {
    let contrast = look.high_contrast;
    let pick = |plain: u32, strong: u32| if contrast { hex(strong) } else { hex(plain) };
    if look.dark {
        Theme {
            dark: true,
            bg: hex(0x08080a),
            grouped: hex(0x08080a),
            group: hex(0x18181b),
            soft: hex(0x18181b),
            surface: hex(0x2a2a2f),
            ink: hex(0xf4f4f5),
            ink2: pick(0xa1a1aa, 0xe4e4e7),
            ink3: pick(0x7c7c85, 0xc4c4cc),
            line: pick(0x232327, 0x52525b),
            hair: rgba(1.0, 1.0, 1.0, if contrast { 0.3 } else { 0.08 }),
            accent: hex(0x6f94ff),
            accent_soft: rgba(111.0 / 255.0, 148.0 / 255.0, 1.0, 0.13),
            btn: hex(0xf4f4f5),
            btn_ink: hex(0x08080a),
            danger: hex(0xff6369),
            press: rgba(1.0, 1.0, 1.0, 0.07),
        }
    } else {
        Theme {
            dark: false,
            bg: hex(0xfbfbfd),
            grouped: hex(0xf5f5f7),
            group: WHITE,
            soft: hex(0xf0f0f3),
            surface: WHITE,
            ink: hex(0x1c1c1e),
            ink2: pick(0x52525b, 0x27272a),
            ink3: pick(0x8a8a92, 0x3f3f46),
            line: pick(0xececee, 0x8a8a92),
            hair: rgba(
                24.0 / 255.0,
                24.0 / 255.0,
                27.0 / 255.0,
                if contrast { 0.3 } else { 0.07 },
            ),
            accent: pick(0x3d6df0, 0x2450d0),
            accent_soft: hex(0xedf2fe),
            btn: hex(0x0d0d0d),
            btn_ink: WHITE,
            danger: pick(0xe5484d, 0xb42318),
            press: rgba(24.0 / 255.0, 24.0 / 255.0, 27.0 / 255.0, 0.05),
        }
    }
}

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

pub(crate) fn rect(x: f32, y: f32, width: f32, height: f32) -> gfx::Rect {
    gfx::Rect {
        x,
        y,
        width,
        height,
    }
}

pub(crate) fn radii(all: f32) -> gfx::CornerRadii {
    gfx::CornerRadii {
        top_left: all,
        top_right: all,
        bottom_right: all,
        bottom_left: all,
    }
}

pub(crate) fn corners(top: f32, bottom: f32) -> gfx::CornerRadii {
    gfx::CornerRadii {
        top_left: top,
        top_right: top,
        bottom_right: bottom,
        bottom_left: bottom,
    }
}

pub(crate) fn pt(x: f32, y: f32) -> gfx::Point {
    gfx::Point { x, y }
}

pub(crate) fn lay(r: gfx::Rect) -> layout::Rect {
    layout::Rect::new(r.x, r.y, r.width, r.height)
}

pub(crate) fn inside(r: &gfx::Rect, x: f32, y: f32) -> bool {
    x >= r.x && x <= r.x + r.width && y >= r.y && y <= r.y + r.height
}

/// Tighter letters as text grows, the way display type is set.
pub(crate) fn style(size: f32, weight: u16) -> gfx::TextStyle {
    let spacing = if size >= 22.0 {
        -0.5
    } else if size >= 16.0 {
        -0.2
    } else {
        0.0
    };
    gfx::TextStyle {
        weight,
        italic: false,
        letter_spacing: spacing,
        family: gfx::FontFamily::Sans,
    }
}

/// A baseline that centres text of `size` in a band from `top`, `h` tall.
pub(crate) fn mid(top: f32, h: f32, size: f32) -> f32 {
    top + h / 2.0 + size * 0.35
}

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

pub(crate) fn read_all(path: &str) -> Option<Vec<u8>> {
    let size = files::stat(path).ok()?.size as usize;
    let file = files::open(path, files::OpenMode::Read).ok()?;
    let mut bytes = Vec::with_capacity(size);
    while bytes.len() < size {
        let chunk = file.read((size - bytes.len()).min(1 << 20) as u32).ok()?;
        if chunk.is_empty() {
            break;
        }
        bytes.extend_from_slice(&chunk);
    }
    Some(bytes)
}

/// A tab-separated file, one record a line, blank lines skipped.
pub(crate) fn lines(path: &str) -> Vec<Vec<String>> {
    let Some(bytes) = read_all(path) else {
        return Vec::new();
    };
    let text = core::str::from_utf8(&bytes).unwrap_or("");
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.split('\t').map(|f| f.trim().to_string()).collect())
        .collect()
}

/// Field `i` of a record, or nothing.
pub(crate) fn field(f: &[String], i: usize) -> String {
    match f.get(i).map(String::as_str) {
        None | Some("-") => String::new(),
        Some(v) => v.to_string(),
    }
}

pub(crate) fn title_case(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

pub(crate) fn initial(name: &str) -> String {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Text, measured once
// ---------------------------------------------------------------------------

pub(crate) struct Text {
    pub canvas: u64,
    widths: BTreeMap<(String, u32, u16), f32>,
    fits: BTreeMap<(String, u32, u16, u32), String>,
    wraps: BTreeMap<(String, u32, u16, u32, usize), Vec<String>>,
    /// The band text may be drawn in: a line wholly outside it is clipped
    /// away anyway, so it is not drawn at all.
    pub band: (f32, f32),
}

fn cut_with_ellipsis(chars: &[char], n: usize) -> String {
    let cut: String = chars[..n].iter().collect();
    format!(
        "{}\u{2026}",
        cut.trim_end_matches(|c: char| c.is_whitespace() || ",.;:-".contains(c))
    )
}

impl Text {
    pub fn new(canvas: u64) -> Self {
        Text {
            canvas,
            widths: BTreeMap::new(),
            fits: BTreeMap::new(),
            wraps: BTreeMap::new(),
            band: (f32::MIN, f32::MAX),
        }
    }

    pub fn width(&mut self, text: &str, size: f32, weight: u16) -> f32 {
        let key = (text.to_string(), size.to_bits(), weight);
        if let Some(w) = self.widths.get(&key) {
            return *w;
        }
        let w = canvas2d::measure_text_styled(self.canvas, text, size, style(size, weight))
            .map(|m| m.width)
            .unwrap_or(text.len() as f32 * size * 0.55);
        if self.widths.len() > 4000 {
            self.widths.clear();
        }
        self.widths.insert(key, w);
        w
    }

    /// `text` cut to `max` wide with an ellipsis.
    pub fn fit(&mut self, text: &str, size: f32, weight: u16, max: f32) -> String {
        let key = (text.to_string(), size.to_bits(), weight, max.to_bits());
        if let Some(s) = self.fits.get(&key) {
            return s.clone();
        }
        let mut fitted = String::new();
        if self.width(text, size, weight) <= max {
            fitted = text.to_string();
        } else {
            // Halve the search: a long description is cut in a few
            // measurements, not one per letter.
            let chars: Vec<char> = text.chars().collect();
            let (mut lo, mut hi) = (0usize, chars.len());
            while lo < hi {
                let m = (lo + hi).div_ceil(2);
                let candidate = cut_with_ellipsis(&chars, m);
                let w = canvas2d::measure_text_styled(
                    self.canvas,
                    &candidate,
                    size,
                    style(size, weight),
                )
                .map(|m| m.width)
                .unwrap_or(f32::MAX);
                if w <= max {
                    lo = m;
                } else {
                    hi = m - 1;
                }
            }
            if lo > 0 {
                fitted = cut_with_ellipsis(&chars, lo);
            }
        }
        if self.fits.len() > 2000 {
            self.fits.clear();
        }
        self.fits.insert(key, fitted.clone());
        fitted
    }

    /// Word-wrapped to `max` wide, at most `most` lines; the last one cut
    /// with an ellipsis when the text goes on.
    pub fn wrap(
        &mut self,
        text: &str,
        size: f32,
        weight: u16,
        max: f32,
        most: usize,
    ) -> Vec<String> {
        let key = (
            text.to_string(),
            size.to_bits(),
            weight,
            max.to_bits(),
            most,
        );
        if let Some(lines) = self.wraps.get(&key) {
            return lines.clone();
        }
        let lines = self.wrap_now(text, size, weight, max, most);
        if self.wraps.len() > 1000 {
            self.wraps.clear();
        }
        self.wraps.insert(key, lines.clone());
        lines
    }

    fn wrap_now(
        &mut self,
        text: &str,
        size: f32,
        weight: u16,
        max: f32,
        most: usize,
    ) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        let mut current = String::new();
        let mut more = false;
        for word in text.split_whitespace() {
            if lines.len() == most {
                more = true;
                break;
            }
            let candidate = if current.is_empty() {
                word.to_string()
            } else {
                format!("{current} {word}")
            };
            if self.width(&candidate, size, weight) <= max || current.is_empty() {
                current = candidate;
                continue;
            }
            lines.push(core::mem::take(&mut current));
            current = word.to_string();
        }
        if !current.is_empty() {
            if lines.len() < most {
                lines.push(current);
            } else {
                more = true;
            }
        }
        if more {
            if let Some(last) = lines.pop() {
                let cut = self.fit(&format!("{last}\u{2026}"), size, weight, max);
                lines.push(cut);
            }
        }
        lines
    }

    pub fn draw(
        &self,
        text: &str,
        x: f32,
        baseline: f32,
        size: f32,
        ink: gfx::Color,
        weight: u16,
    ) -> Result<(), gfx::GfxError> {
        if baseline - size * 0.8 > self.band.1 || baseline + size * 0.25 < self.band.0 {
            return Ok(());
        }
        canvas2d::draw_text_styled(
            self.canvas,
            text,
            pt(x, baseline),
            size,
            ink,
            style(size, weight),
        )
    }
}

// ---------------------------------------------------------------------------
// Pictures
// ---------------------------------------------------------------------------

/// A picture as the player wrote it: straight RGBA.
pub(crate) struct Raw {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub(crate) fn read_raw(name: &str) -> Option<Raw> {
    if name == "-" || name.is_empty() || name.contains("..") || name.contains('/') {
        return None;
    }
    let mut bytes = read_all(&format!("art/{name}"))?;
    if bytes.len() < 8 {
        return None;
    }
    let width = u32::from_le_bytes(bytes[0..4].try_into().ok()?);
    let height = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
    if width == 0 || height == 0 || bytes.len() - 8 != (width as usize) * (height as usize) * 4 {
        return None;
    }
    bytes.drain(0..8);
    Some(Raw {
        width,
        height,
        rgba: bytes,
    })
}

/// The part of `raw` with the shape `aspect` (width over height). Of a
/// picture wider than that, the part `across` of the way from its left
/// (0 keeps the left edge, where an app's title sits; 0.5 the middle); of a
/// taller one, a band a little above the middle, under an app's title.
pub(crate) fn region(raw: &Raw, aspect: f32, across: f32) -> (u32, u32, u32, u32) {
    let (w, h) = (raw.width as f32, raw.height as f32);
    if w / h > aspect {
        let cw = ((h * aspect) as u32).clamp(1, raw.width);
        (((raw.width - cw) as f32 * across) as u32, 0, cw, raw.height)
    } else {
        let ch = ((w / aspect) as u32).clamp(1, raw.height);
        let y0 = (((h - ch as f32) * 0.3) as u32).min(raw.height - ch);
        (0, y0, raw.width, ch)
    }
}

/// `region` of `raw` scaled to `tw` by `th`, each pixel the average of a
/// 2 by 2 grid of samples. A phone may run this under an interpreter, at
/// start, for every app: four reads and a few bit operations a pixel keep
/// a screen of tiles to milliseconds, and a 4x shrink still reads smooth.
pub(crate) fn resample(
    raw: &Raw,
    (x0, y0, cw, ch): (u32, u32, u32, u32),
    tw: u32,
    th: u32,
) -> Vec<u8> {
    // One bounds check a pixel: a 4-byte window, then fixed indexes.
    let at = |row: &[u8], i: usize| -> u32 {
        let b = &row[i..i + 4];
        u32::from_le_bytes([b[0], b[1], b[2], b[3]])
    };
    // The average of two pixels, every channel at once.
    let avg = |a: u32, b: u32| (a & b) + (((a ^ b) & 0xfefe_fefe) >> 1);
    let column = |t: u32, n: u32, c: u32, o: u32, q: f32| -> u32 {
        (o + ((t as f32 + q) * c as f32 / n as f32) as u32).min(o + c - 1)
    };
    let xs: Vec<(usize, usize)> = (0..tw)
        .map(|tx| {
            (
                column(tx, tw, cw, x0, 0.25) as usize * 4,
                column(tx, tw, cw, x0, 0.75) as usize * 4,
            )
        })
        .collect();
    let stride = raw.width as usize * 4;
    let mut px = alloc::vec![0u8; (tw * th * 4) as usize];
    for (ty, out) in px.chunks_exact_mut(tw as usize * 4).enumerate() {
        let ty = ty as u32;
        let ya = column(ty, th, ch, y0, 0.25) as usize * stride;
        let yb = column(ty, th, ch, y0, 0.75) as usize * stride;
        let (ra, rb) = (&raw.rgba[ya..ya + stride], &raw.rgba[yb..yb + stride]);
        for (dst, &(xa, xb)) in out.chunks_exact_mut(4).zip(xs.iter()) {
            let v = avg(avg(at(ra, xa), at(ra, xb)), avg(at(rb, xa), at(rb, xb)));
            dst.copy_from_slice(&v.to_le_bytes());
        }
    }
    px
}

/// `region` of `raw` as it is, row by row: the host scales it as it draws.
pub(crate) fn cut_out(raw: &Raw, (x0, y0, cw, ch): (u32, u32, u32, u32)) -> Vec<u8> {
    let mut px = Vec::with_capacity((cw * ch * 4) as usize);
    for y in y0..y0 + ch {
        let start = ((y * raw.width + x0) * 4) as usize;
        px.extend_from_slice(&raw.rgba[start..start + (cw * 4) as usize]);
    }
    px
}

/// The overall colour of a picture.
pub(crate) fn tint(raw: &Raw) -> (f32, f32, f32) {
    let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
    for p in raw.rgba.chunks_exact(4).step_by(97) {
        r += p[0] as u32;
        g += p[1] as u32;
        b += p[2] as u32;
        n += 1;
    }
    let n = n.max(1) as f32 * 255.0;
    (r as f32 / n, g as f32 / n, b as f32 / n)
}

/// Pixels on a side for an app's tile: 60 points at a large phone's
/// 2.625x.
const TILE_PX: u32 = 160;

/// An app's face for its tile: its own picture, cropped square, or --
/// with no picture -- Krate's one app icon, the same for every such app.
/// A person or an organisation with no picture wears a soft colour and
/// an initial instead (`Face::hue`).
#[derive(Clone, Copy)]
pub(crate) struct Face {
    pub tile: Option<u64>,
    /// A colour tile with an initial (people and organisations).
    pub initial: bool,
    /// Its picture is still to be kept: Krate's icon stands in until then.
    pub pending: bool,
}

impl Face {
    /// An app with no picture of its own: Krate's app icon.
    pub const KRATE: Face = Face {
        tile: None,
        initial: false,
        pending: false,
    };

    /// A face whose picture is kept later, when the screen is idle.
    pub const WAITING: Face = Face {
        tile: None,
        initial: false,
        pending: true,
    };

    /// A face in a colour picked from `name`, with its initial.
    pub fn hue(canvas: u64, name: &str) -> Face {
        Face {
            tile: keep_hue(canvas, name),
            initial: true,
            pending: false,
        }
    }
}

/// Whether `art` names a picture at all.
pub(crate) fn names_art(art: &str) -> bool {
    !art.is_empty() && art != "-"
}

pub(crate) fn keep_face(canvas: u64, raw: Option<&Raw>) -> Face {
    if let Some(raw) = raw {
        let px = resample(raw, region(raw, 1.0, 0.5), TILE_PX, TILE_PX);
        if let Ok(handle) = canvas_images::keep(canvas, TILE_PX, TILE_PX, &px) {
            return Face {
                tile: Some(handle),
                initial: false,
                pending: false,
            };
        }
    }
    Face::KRATE
}

/// Pairs of colours, light to deep, that sit well beside the accent.
const HUES: [(u32, u32); 8] = [
    (0x7a8cff, 0x3d58e8),
    (0xff9a7a, 0xe8506a),
    (0x5fd6bd, 0x14898f),
    (0xb69bff, 0x6a46e0),
    (0xffc766, 0xf2782e),
    (0x6cc3ff, 0x2a74e0),
    (0x8bdc84, 0x2e9a5c),
    (0xff8fbf, 0xd23c86),
];

pub(crate) fn keep_hue(canvas: u64, name: &str) -> Option<u64> {
    let seed = name
        .bytes()
        .fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
    let (a, b) = HUES[(seed as usize) % HUES.len()];
    let (a, b) = (hex(a), hex(b));
    let side = 32u32;
    let mut px = Vec::with_capacity((side * side * 4) as usize);
    for y in 0..side {
        for x in 0..side {
            let t = ((x + y) as f32 / (2 * (side - 1)) as f32).clamp(0.0, 1.0);
            let t = t * t * (3.0 - 2.0 * t);
            px.push(((a.r * (1.0 - t) + b.r * t) * 255.0) as u8);
            px.push(((a.g * (1.0 - t) + b.g * t) * 255.0) as u8);
            px.push(((a.b * (1.0 - t) + b.b * t) * 255.0) as u8);
            px.push(255);
        }
    }
    canvas_images::keep(canvas, side, side, &px).ok()
}

/// A fade from clear to `alpha` black, top to bottom, made at an eighth of
/// the size it is drawn: text over a picture. Drawn with rounded corners it
/// fills its rectangle, so it is made in that rectangle's proportions.
pub(crate) fn keep_fade(canvas: u64, w: u32, h: u32, alpha: f32) -> Option<u64> {
    let (w, h) = (w.max(2), h.max(2));
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        let t = y as f32 / (h - 1) as f32;
        let a = (t * t * (3.0 - 2.0 * t) * alpha * 255.0) as u8;
        for _ in 0..w {
            px.extend_from_slice(&[0, 0, 0, a]);
        }
    }
    canvas_images::keep(canvas, w, h, &px).ok()
}

/// A tall backdrop in a picture's own colour, lighter at the top.
pub(crate) fn keep_backdrop(canvas: u64, (r, g, b): (f32, f32, f32)) -> Option<u64> {
    let (w, h) = (4u32, 48u32);
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        let t = y as f32 / (h - 1) as f32;
        let k = 0.95 - 0.45 * t;
        for _ in 0..w {
            px.extend_from_slice(&[
                (r * k * 255.0) as u8,
                (g * k * 255.0) as u8,
                (b * k * 255.0) as u8,
                255,
            ]);
        }
    }
    canvas_images::keep(canvas, w, h, &px).ok()
}

/// A chevron, drawn smooth at 3 pixels a point: `back` points left (the
/// way back), otherwise right (more behind this row). `w` by `h` points.
pub(crate) fn keep_chevron(
    canvas: u64,
    ink: gfx::Color,
    back: bool,
    w: f32,
    h: f32,
    stroke: f32,
) -> Option<u64> {
    let k = 3.0;
    let (pw, ph) = ((w * k) as u32, (h * k) as u32);
    let half = stroke * k / 2.0;
    let pad = half + 1.0;
    let (fw, fh) = (pw as f32, ph as f32);
    let (tip_x, end_x) = if back {
        (pad, fw - pad)
    } else {
        (fw - pad, pad)
    };
    let a = (end_x, pad);
    let b = (tip_x, fh / 2.0);
    let c = (end_x, fh - pad);
    let dist = |p: (f32, f32), s: (f32, f32), e: (f32, f32)| -> f32 {
        let (dx, dy) = (e.0 - s.0, e.1 - s.1);
        let len = dx * dx + dy * dy;
        let t = (((p.0 - s.0) * dx + (p.1 - s.1) * dy) / len).clamp(0.0, 1.0);
        let (qx, qy) = (s.0 + dx * t - p.0, s.1 + dy * t - p.1);
        qx * qx + qy * qy
    };
    let (r, g, bl) = (
        (ink.r * 255.0) as u8,
        (ink.g * 255.0) as u8,
        (ink.b * 255.0) as u8,
    );
    let mut px = Vec::with_capacity((pw * ph * 4) as usize);
    for y in 0..ph {
        for x in 0..pw {
            let p = (x as f32 + 0.5, y as f32 + 0.5);
            let d2 = dist(p, a, b).min(dist(p, b, c));
            let d = d2.sqrt();
            let cover = (half + 0.5 - d).clamp(0.0, 1.0) * ink.a;
            px.extend_from_slice(&[r, g, bl, (cover * 255.0) as u8]);
        }
    }
    canvas_images::keep(canvas, pw, ph, &px).ok()
}

// ---------------------------------------------------------------------------
// Outline icons, drawn smooth once and kept
// ---------------------------------------------------------------------------

/// A stroke through points, in 24-unit space.
type Polyline = Vec<(f32, f32)>;
/// A circle's centre and radius, in 24-unit space.
type Ring = (f32, f32, f32);

/// The outline icons, drawn on a 24-unit square like a type designer's
/// grid, with round ends.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Icon {
    /// Library: stacked layers.
    Layers,
    /// Discover: a compass.
    Compass,
    /// You: a head and shoulders.
    Person,
    /// Open a file: a crate.
    Crate,
    /// Settings: a cog.
    Cog,
    /// Safety: a shield with a tick.
    Shield,
    /// Links: two links of a chain.
    Link,
    /// Help and problems: a speech bubble.
    Bubble,
    /// Storage: a stack of disks.
    Disk,
    /// Appearance: a sun.
    Sun,
    /// This device: a phone.
    Phone,
    /// A computer among a person's devices: a laptop.
    Laptop,
    /// Sync across devices: two arrows chasing round a circle.
    Sync,
}

impl Icon {
    /// The picture for one of a person's devices, from the only thing the
    /// hub knows of it: its name ("Yashraj's MacBook Pro", "Galaxy S22 Ultra").
    pub(crate) fn for_device(name: &str) -> Icon {
        let name = name.to_lowercase();
        let computer = [
            "mac", "pc", "windows", "linux", "laptop", "desktop", "computer", "thinkpad", "surface",
        ];
        if name.contains("iphone") || name.contains("ipad") {
            return Icon::Phone;
        }
        if computer.iter().any(|word| name.contains(word)) {
            Icon::Laptop
        } else {
            Icon::Phone
        }
    }

    /// The icon's strokes, as polylines and rings in 24-unit space.
    fn shape(self) -> (Vec<Polyline>, Vec<Ring>) {
        let arc = |cx: f32, cy: f32, r: f32, from: f32, to: f32| -> Vec<(f32, f32)> {
            let n = 10;
            (0..=n)
                .map(|i| {
                    let a = (from + (to - from) * i as f32 / n as f32).to_radians();
                    (cx + r * a.cos(), cy + r * a.sin())
                })
                .collect()
        };
        match self {
            Icon::Layers => (
                alloc::vec![
                    alloc::vec![
                        (12.0, 2.5),
                        (21.5, 7.2),
                        (12.0, 11.9),
                        (2.5, 7.2),
                        (12.0, 2.5)
                    ],
                    alloc::vec![(2.5, 12.0), (12.0, 16.7), (21.5, 12.0)],
                    alloc::vec![(2.5, 16.8), (12.0, 21.5), (21.5, 16.8)],
                ],
                Vec::new(),
            ),
            Icon::Compass => (
                alloc::vec![alloc::vec![
                    (16.2, 7.8),
                    (14.1, 14.1),
                    (7.8, 16.2),
                    (9.9, 9.9),
                    (16.2, 7.8)
                ]],
                alloc::vec![(12.0, 12.0, 9.5)],
            ),
            Icon::Person => {
                let mut body = alloc::vec![(4.5, 21.0)];
                body.extend(arc(9.0, 18.5, 4.5, 180.0, 270.0));
                body.extend(arc(15.0, 18.5, 4.5, 270.0, 360.0));
                body.push((19.5, 21.0));
                (alloc::vec![body], alloc::vec![(12.0, 8.0, 4.2)])
            }
            Icon::Crate => (
                alloc::vec![
                    alloc::vec![
                        (12.0, 2.5),
                        (20.5, 7.2),
                        (20.5, 16.8),
                        (12.0, 21.5),
                        (3.5, 16.8),
                        (3.5, 7.2),
                        (12.0, 2.5)
                    ],
                    alloc::vec![(3.5, 7.2), (12.0, 12.0), (20.5, 7.2)],
                    alloc::vec![(12.0, 12.0), (12.0, 21.5)],
                ],
                Vec::new(),
            ),
            Icon::Cog => {
                let mut teeth = Vec::new();
                for k in 0..8 {
                    let a = (k as f32 * 45.0).to_radians();
                    teeth.push(alloc::vec![
                        (12.0 + 7.2 * a.cos(), 12.0 + 7.2 * a.sin()),
                        (12.0 + 9.6 * a.cos(), 12.0 + 9.6 * a.sin())
                    ]);
                }
                (teeth, alloc::vec![(12.0, 12.0, 6.6), (12.0, 12.0, 2.8)])
            }
            Icon::Shield => (
                alloc::vec![
                    alloc::vec![
                        (12.0, 21.5),
                        (7.5, 19.0),
                        (4.5, 15.0),
                        (4.0, 6.0),
                        (12.0, 2.5),
                        (20.0, 6.0),
                        (19.5, 15.0),
                        (16.5, 19.0),
                        (12.0, 21.5)
                    ],
                    alloc::vec![(8.5, 12.0), (11.0, 14.5), (15.8, 9.5)],
                ],
                Vec::new(),
            ),
            Icon::Link => {
                let mut a = arc(8.0, 12.0, 4.5, 90.0, 270.0);
                a.insert(0, (11.0, 16.5));
                a.push((11.0, 7.5));
                let mut b = arc(16.0, 12.0, 4.5, -90.0, 90.0);
                b.insert(0, (13.0, 7.5));
                b.push((13.0, 16.5));
                (
                    alloc::vec![a, b, alloc::vec![(8.5, 12.0), (15.5, 12.0)]],
                    Vec::new(),
                )
            }
            Icon::Bubble => {
                let mut outline = arc(12.0, 11.0, 8.5, 120.0, 480.0);
                outline.push((5.0, 20.5));
                outline.push((7.75, 18.36));
                (alloc::vec![outline], Vec::new())
            }
            Icon::Sun => {
                let mut rays = Vec::new();
                for k in 0..8 {
                    let a = (k as f32 * 45.0).to_radians();
                    rays.push(alloc::vec![
                        (12.0 + 7.4 * a.cos(), 12.0 + 7.4 * a.sin()),
                        (12.0 + 9.8 * a.cos(), 12.0 + 9.8 * a.sin())
                    ]);
                }
                (rays, alloc::vec![(12.0, 12.0, 4.2)])
            }
            Icon::Phone => {
                let mut outline = arc(9.0, 5.0, 2.5, 180.0, 270.0);
                outline.extend(arc(15.0, 5.0, 2.5, 270.0, 360.0));
                outline.extend(arc(15.0, 19.0, 2.5, 0.0, 90.0));
                outline.extend(arc(9.0, 19.0, 2.5, 90.0, 180.0));
                outline.push((6.5, 5.0));
                (
                    alloc::vec![outline, alloc::vec![(10.5, 18.5), (13.5, 18.5)]],
                    Vec::new(),
                )
            }
            Icon::Laptop => {
                // A screen with rounded top corners, and the base it sits on.
                let mut screen = arc(6.5, 7.5, 1.5, 180.0, 270.0);
                screen.extend(arc(17.5, 7.5, 1.5, 270.0, 360.0));
                screen.push((19.0, 16.0));
                screen.push((5.0, 16.0));
                screen.push((5.0, 7.5));
                (
                    alloc::vec![screen, alloc::vec![(3.0, 18.5), (21.0, 18.5)]],
                    Vec::new(),
                )
            }
            Icon::Sync => {
                // Each arrow's head points the way its arc runs (clockwise
                // on a screen, where y grows downward).
                let head = |deg: f32| -> Vec<(f32, f32)> {
                    let a = deg.to_radians();
                    let (ex, ey) = (12.0 + 7.5 * a.cos(), 12.0 + 7.5 * a.sin());
                    let back = a + core::f32::consts::FRAC_PI_2 + core::f32::consts::PI;
                    let spread = 0.7;
                    alloc::vec![
                        (
                            ex + 3.6 * (back - spread).cos(),
                            ey + 3.6 * (back - spread).sin()
                        ),
                        (ex, ey),
                        (
                            ex + 3.6 * (back + spread).cos(),
                            ey + 3.6 * (back + spread).sin()
                        ),
                    ]
                };
                (
                    alloc::vec![
                        arc(12.0, 12.0, 7.5, 200.0, 335.0),
                        head(335.0),
                        arc(12.0, 12.0, 7.5, 20.0, 155.0),
                        head(155.0),
                    ],
                    Vec::new(),
                )
            }
            Icon::Disk => {
                let mut top = arc(12.0, 6.0, 8.0, 0.0, 360.0);
                for p in top.iter_mut() {
                    p.1 = 6.0 + (p.1 - 6.0) * 0.35;
                }
                let mut mid = arc(12.0, 12.0, 8.0, 0.0, 180.0);
                for p in mid.iter_mut() {
                    p.1 = 12.0 + (p.1 - 12.0) * 0.35;
                }
                let mut bottom = arc(12.0, 18.0, 8.0, 0.0, 180.0);
                for p in bottom.iter_mut() {
                    p.1 = 18.0 + (p.1 - 18.0) * 0.35;
                }
                (
                    alloc::vec![
                        top,
                        mid,
                        bottom,
                        alloc::vec![(4.0, 6.0), (4.0, 18.0)],
                        alloc::vec![(20.0, 6.0), (20.0, 18.0)],
                    ],
                    Vec::new(),
                )
            }
        }
    }
}

/// `icon` in `ink`, `side` points square, strokes `stroke` points wide,
/// drawn smooth at 3 pixels a point.
pub(crate) fn keep_icon(
    canvas: u64,
    icon: Icon,
    ink: gfx::Color,
    side: f32,
    stroke: f32,
) -> Option<u64> {
    let px_side = (side * 3.0) as u32;
    let k = px_side as f32 / 24.0;
    let half = stroke * 3.0 / 2.0;
    let (lines, rings) = icon.shape();
    let segs: Vec<((f32, f32), (f32, f32))> = lines
        .iter()
        .flat_map(|l| {
            l.windows(2)
                .map(|w| ((w[0].0 * k, w[0].1 * k), (w[1].0 * k, w[1].1 * k)))
        })
        .collect();
    let rings: Vec<(f32, f32, f32)> = rings
        .iter()
        .map(|&(x, y, r)| (x * k, y * k, r * k))
        .collect();
    let (r, g, b) = (
        (ink.r * 255.0) as u8,
        (ink.g * 255.0) as u8,
        (ink.b * 255.0) as u8,
    );
    // Each stroke only touches the pixels near it: walk its own box, not
    // the whole icon, keeping the nearest distance per pixel.
    let n = px_side as usize;
    let mut dist = alloc::vec![f32::MAX; n * n];
    let reach = half + 1.5;
    let span = |lo: f32, hi: f32| -> (usize, usize) {
        (
            (lo - reach).floor().max(0.0) as usize,
            ((hi + reach).ceil().max(0.0) as usize).min(n),
        )
    };
    for &(s, e) in &segs {
        let (dx, dy) = (e.0 - s.0, e.1 - s.1);
        let len = (dx * dx + dy * dy).max(1e-6);
        let (x0, x1) = span(s.0.min(e.0), s.0.max(e.0));
        let (y0, y1) = span(s.1.min(e.1), s.1.max(e.1));
        for y in y0..y1 {
            let py = y as f32 + 0.5;
            for x in x0..x1 {
                let px = x as f32 + 0.5;
                let t = (((px - s.0) * dx + (py - s.1) * dy) / len).clamp(0.0, 1.0);
                let (qx, qy) = (s.0 + dx * t - px, s.1 + dy * t - py);
                let d = (qx * qx + qy * qy).sqrt();
                let slot = &mut dist[y * n + x];
                if d < *slot {
                    *slot = d;
                }
            }
        }
    }
    for &(cx, cy, rr) in &rings {
        let (x0, x1) = span(cx - rr, cx + rr);
        let (y0, y1) = span(cy - rr, cy + rr);
        for y in y0..y1 {
            let dy = y as f32 + 0.5 - cy;
            for x in x0..x1 {
                let dx = x as f32 + 0.5 - cx;
                let d = ((dx * dx + dy * dy).sqrt() - rr).abs();
                let slot = &mut dist[y * n + x];
                if d < *slot {
                    *slot = d;
                }
            }
        }
    }
    let mut px = Vec::with_capacity(n * n * 4);
    for d in dist {
        let cover = (half + 0.5 - d).clamp(0.0, 1.0) * ink.a;
        px.extend_from_slice(&[r, g, b, (cover * 255.0) as u8]);
    }
    canvas_images::keep(canvas, px_side, px_side, &px).ok()
}

/// A soft shadow for a rounded rectangle `w` by `h` points with corner
/// `radius`, spread `blur` points, darkest at `alpha`: made once at half a
/// pixel a point (a shadow has no edges to keep), drawn as a picture every
/// frame instead of blurred every frame.
pub(crate) fn keep_shadow(
    canvas: u64,
    w: f32,
    h: f32,
    radius: f32,
    blur: f32,
    alpha: f32,
) -> Option<u64> {
    let k = 0.5;
    let (fw, fh) = (w + blur * 2.0, h + blur * 2.0);
    let (pw, ph) = (((fw * k) as u32).max(2), ((fh * k) as u32).max(2));
    let mut px = Vec::with_capacity((pw * ph * 4) as usize);
    let (hx, hy) = (w / 2.0 - radius, h / 2.0 - radius);
    for y in 0..ph {
        for x in 0..pw {
            // Distance outside the rounded rectangle, centred.
            let qx = ((x as f32 + 0.5) / k - fw / 2.0).abs() - hx;
            let qy = ((y as f32 + 0.5) / k - fh / 2.0).abs() - hy;
            let outside = (qx.max(0.0) * qx.max(0.0) + qy.max(0.0) * qy.max(0.0)).sqrt()
                + qx.max(qy).min(0.0)
                - radius;
            let t = ((outside + blur * 0.25) / blur).clamp(0.0, 1.0);
            let a = (1.0 - t) * (1.0 - t) * alpha;
            px.extend_from_slice(&[0, 0, 0, (a * 255.0) as u8]);
        }
    }
    canvas_images::keep(canvas, pw, ph, &px).ok()
}
