//! Darkroom -- a photo editor built by hand to find the memory ceiling.
//!
//! CP-D of Plan/Proof-Checkpoints-2026-09-16.md. The other three checkpoints
//! asked whether the runtime can carry a demanding app; this one has a
//! specific number in its sights. The guest memory ceiling defaults to
//! 256 MB, one RGBA copy of a 24 MP photo is 92 MB, and an editor wants the
//! original, a working copy, an undo step and the composited output. The
//! arithmetic says 24 MP does not fit. This finds out what actually happens.
//!
//! The app: load an image, crop, adjust exposure, contrast, saturation and
//! warmth, a histogram, undo, and export. Adjustments run on a downscaled
//! PREVIEW while a slider moves and on the full image when it stops, which is
//! how every real editor stays interactive -- and is itself a finding, since
//! doing it the naive way is what makes the ceiling bite.
//!
//! `#![no_std]`: the pixel buffers are the one thing that allocates, and they
//! are allocated deliberately rather than by accident.

#![no_std]
#![allow(clippy::needless_range_loop)]
#![allow(clippy::too_many_arguments)]

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use krate::bindings::krate::io::{args, stdio};
use krate::bindings::krate::time::clock;
use krate::gfx::{canvas2d, types as gfx};
use krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

const WIDTH: f32 = 960.0;
const HEIGHT: f32 = 620.0;

const PANEL_W: f32 = 240.0;
const TOP: f32 = 44.0;
const STATUS_H: f32 = 24.0;

/// The longest edge the preview is scaled to.
///
/// Every adjustment while a slider moves runs on this, not on the full image.
/// At 1024 that is about 1 MP of work per frame regardless of whether the
/// photo is 8 MP or 24 MP, which is the difference between a slider that
/// tracks your finger and one that does not.
const PREVIEW_EDGE: usize = 1024;

// ------------------------------------------------------------------ image

struct Image {
    w: usize,
    h: usize,
    /// Straight RGBA, four bytes per pixel, top row first -- the layout both
    /// `draw_pixels` and the image widget take.
    px: Vec<u8>,
}

impl Image {
    fn new(w: usize, h: usize) -> Image {
        Image {
            w,
            h,
            px: vec![0u8; w * h * 4],
        }
    }

    fn bytes(&self) -> usize {
        self.px.len()
    }

    /// Nearest-neighbour downscale to a preview.
    ///
    /// Nearest rather than a box filter on purpose: this runs on every load
    /// and the preview is for judging an adjustment, not for export. A proper
    /// filter belongs on the export path, where it is paid once.
    fn preview(&self, edge: usize) -> Image {
        let scale = if self.w.max(self.h) <= edge {
            1.0
        } else {
            edge as f32 / self.w.max(self.h) as f32
        };
        let pw = ((self.w as f32 * scale) as usize).max(1);
        let ph = ((self.h as f32 * scale) as usize).max(1);
        let mut out = Image::new(pw, ph);
        for y in 0..ph {
            let sy = (y * self.h) / ph;
            for x in 0..pw {
                let sx = (x * self.w) / pw;
                let s = (sy * self.w + sx) * 4;
                let d = (y * pw + x) * 4;
                out.px[d] = self.px[s];
                out.px[d + 1] = self.px[s + 1];
                out.px[d + 2] = self.px[s + 2];
                out.px[d + 3] = self.px[s + 3];
            }
        }
        out
    }
}

/// A photograph, synthesised. A real editor opens a file; this needs a subject
/// of an exact size, reproducibly, to measure against.
fn synth_photo(w: usize, h: usize) -> Image {
    let mut img = Image::new(w, h);
    for y in 0..h {
        let fy = y as f32 / h as f32;
        for x in 0..w {
            let fx = x as f32 / w as f32;
            // A sky gradient, a sun, and some ground texture: enough tonal
            // range that exposure and contrast visibly do something.
            let sky = 1.0 - fy * 0.8;
            let sun = {
                let dx = fx - 0.72;
                let dy = fy - 0.22;
                let d = dx * dx + dy * dy;
                if d < 0.010 {
                    1.0
                } else {
                    (0.010 / (d + 0.004)).min(1.0) * 0.55
                }
            };
            let ground = if fy > 0.62 {
                let n = ((x * 7 + y * 13) % 23) as f32 / 23.0;
                0.20 + n * 0.16
            } else {
                0.0
            };
            let (r, g, b) = if fy > 0.62 {
                (0.30 + ground, 0.26 + ground * 0.8, 0.18 + ground * 0.5)
            } else {
                (
                    (0.35 * sky + sun).min(1.0),
                    (0.55 * sky + sun * 0.9).min(1.0),
                    (0.85 * sky + sun * 0.7).min(1.0),
                )
            };
            let i = (y * w + x) * 4;
            img.px[i] = (r * 255.0) as u8;
            img.px[i + 1] = (g * 255.0) as u8;
            img.px[i + 2] = (b * 255.0) as u8;
            img.px[i + 3] = 255;
        }
    }
    img
}

// ------------------------------------------------------------ adjustments

#[derive(Clone, Copy, PartialEq)]
struct Adjust {
    /// Stops of exposure, -2 to +2.
    exposure: f32,
    /// -1 to +1.
    contrast: f32,
    /// -1 (grey) to +1.
    saturation: f32,
    /// -1 (cool) to +1 (warm).
    warmth: f32,
}

impl Adjust {
    fn none() -> Adjust {
        Adjust {
            exposure: 0.0,
            contrast: 0.0,
            saturation: 0.0,
            warmth: 0.0,
        }
    }

    fn is_identity(&self) -> bool {
        self.exposure == 0.0
            && self.contrast == 0.0
            && self.saturation == 0.0
            && self.warmth == 0.0
    }
}

/// Apply the adjustment from `src` into `dst`, which must be the same size.
///
/// Writes into a caller-owned buffer rather than returning a new one: an
/// editor applies this on every slider move, and allocating a full-size
/// buffer each time is what turns a 92 MB image into a memory problem.
fn apply(src: &Image, dst: &mut Image, a: Adjust) {
    debug_assert!(src.w == dst.w && src.h == dst.h);
    let gain = exp2(a.exposure);
    let c = a.contrast;
    let sat = 1.0 + a.saturation;
    let warm = a.warmth;

    for i in (0..src.px.len()).step_by(4) {
        let mut r = src.px[i] as f32 / 255.0;
        let mut g = src.px[i + 1] as f32 / 255.0;
        let mut b = src.px[i + 2] as f32 / 255.0;

        r *= gain;
        g *= gain;
        b *= gain;

        if c != 0.0 {
            // Pivot around mid grey, which is what a contrast slider means.
            r = (r - 0.5) * (1.0 + c) + 0.5;
            g = (g - 0.5) * (1.0 + c) + 0.5;
            b = (b - 0.5) * (1.0 + c) + 0.5;
        }

        if sat != 1.0 {
            let l = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            r = l + (r - l) * sat;
            g = l + (g - l) * sat;
            b = l + (b - l) * sat;
        }

        if warm != 0.0 {
            r += warm * 0.08;
            b -= warm * 0.08;
        }

        dst.px[i] = clamp8(r);
        dst.px[i + 1] = clamp8(g);
        dst.px[i + 2] = clamp8(b);
        dst.px[i + 3] = src.px[i + 3];
    }
}

fn clamp8(v: f32) -> u8 {
    if v <= 0.0 {
        0
    } else if v >= 1.0 {
        255
    } else {
        (v * 255.0) as u8
    }
}

/// 2^x for the exposure slider. `f32::exp2` is std; a no_std guest brings its
/// own, and over -2..2 a few terms are plenty.
fn exp2(x: f32) -> f32 {
    let mut n = x;
    let mut shift = 1.0f32;
    while n > 1.0 {
        shift *= 2.0;
        n -= 1.0;
    }
    while n < -1.0 {
        shift *= 0.5;
        n += 1.0;
    }
    // 2^n for n in [-1,1] via exp(n ln2), four terms.
    let t = n * 0.693_147_2;
    let series = 1.0 + t + t * t * 0.5 + t * t * t * 0.166_666_7 + t * t * t * t * 0.041_666_7;
    shift * series
}

/// 64-bucket luminance histogram of a preview.
fn histogram(img: &Image) -> [u32; 64] {
    let mut h = [0u32; 64];
    for i in (0..img.px.len()).step_by(4) {
        let l = (img.px[i] as u32 * 54 + img.px[i + 1] as u32 * 183 + img.px[i + 2] as u32 * 18)
            >> 8;
        h[(l as usize * 63) / 255] += 1;
    }
    h
}

// ------------------------------------------------------------------- state

#[derive(Clone, Copy, PartialEq)]
enum Slider {
    Exposure,
    Contrast,
    Saturation,
    Warmth,
}

const SLIDERS: [(Slider, &str); 4] = [
    (Slider::Exposure, "Exposure"),
    (Slider::Contrast, "Contrast"),
    (Slider::Saturation, "Saturation"),
    (Slider::Warmth, "Warmth"),
];

struct App {
    /// The photo as loaded. Never modified, so every adjustment is
    /// non-destructive and undo is free.
    original: Image,
    /// A downscaled copy, the thing every slider move works on.
    preview_src: Image,
    /// Where the adjusted preview is written each time. Allocated once.
    preview_out: Image,
    hist: [u32; 64],

    adjust: Adjust,
    committed: Adjust,
    undo: Vec<Adjust>,
    selected: usize,

    name: String,
    status: String,
    last_apply_us: u64,
    last_full_us: u64,
}

impl App {
    fn recompute(&mut self) {
        let t = clock::monotonic_nanos();
        apply(&self.preview_src, &mut self.preview_out, self.adjust);
        self.last_apply_us = clock::monotonic_nanos().saturating_sub(t) / 1_000;
        self.hist = histogram(&self.preview_out);
    }

    fn bump(&mut self, delta: f32) {
        self.undo.push(self.adjust);
        if self.undo.len() > 64 {
            self.undo.remove(0);
        }
        let a = &mut self.adjust;
        match SLIDERS[self.selected].0 {
            Slider::Exposure => a.exposure = (a.exposure + delta * 2.0).clamp(-2.0, 2.0),
            Slider::Contrast => a.contrast = (a.contrast + delta).clamp(-1.0, 1.0),
            Slider::Saturation => a.saturation = (a.saturation + delta).clamp(-1.0, 1.0),
            Slider::Warmth => a.warmth = (a.warmth + delta).clamp(-1.0, 1.0),
        }
        self.recompute();
    }
}

// ------------------------------------------------------------------ colour

fn rgb(r: f32, g: f32, b: f32) -> gfx::Color {
    gfx::Color { r, g, b, a: 1.0 }
}

fn rgba(r: f32, g: f32, b: f32, a: f32) -> gfx::Color {
    gfx::Color { r, g, b, a }
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> gfx::Rect {
    gfx::Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

fn u64_str(v: u64) -> String {
    let mut s = String::new();
    let mut d = [0u8; 24];
    let mut n = 0;
    let mut v = v;
    if v == 0 {
        s.push('0');
        return s;
    }
    while v > 0 {
        d[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    while n > 0 {
        n -= 1;
        s.push(d[n] as char);
    }
    s
}

fn f32_str(v: f32) -> String {
    let neg = v < 0.0;
    let a = if neg { -v } else { v };
    let whole = a as u64;
    let frac = ((a - whole as f32) * 100.0) as u64;
    let mut s = String::new();
    if neg {
        s.push('-');
    }
    s.push_str(&u64_str(whole));
    s.push('.');
    if frac < 10 {
        s.push('0');
    }
    s.push_str(&u64_str(frac));
    s
}

// -------------------------------------------------------------------- draw

fn draw(a: &App, canvas: u64) -> Result<(), gfx::GfxError> {
    canvas2d::clear(canvas, rgb(0.07, 0.07, 0.09))?;

    canvas2d::fill_rect(canvas, rect(0.0, 0.0, WIDTH, TOP), rgb(0.11, 0.12, 0.15))?;
    canvas2d::draw_text(
        canvas,
        &a.name,
        gfx::Point { x: 16.0, y: 27.0 },
        14.0,
        rgb(0.88, 0.91, 0.96),
    )?;

    let mut dims = String::new();
    dims.push_str(&u64_str(a.original.w as u64));
    dims.push_str(" x ");
    dims.push_str(&u64_str(a.original.h as u64));
    dims.push_str("   ");
    dims.push_str(&u64_str((a.original.bytes() / 1_048_576) as u64));
    dims.push_str(" MB in memory");
    canvas2d::draw_text(
        canvas,
        &dims,
        gfx::Point { x: 200.0, y: 27.0 },
        11.5,
        rgb(0.52, 0.60, 0.72),
    )?;

    // The photo, letterboxed into the space left of the panel.
    let stage_w = WIDTH - PANEL_W;
    let stage_h = HEIGHT - TOP - STATUS_H;
    let img = &a.preview_out;
    let scale = (stage_w / img.w as f32).min(stage_h / img.h as f32);
    let dw = img.w as f32 * scale;
    let dh = img.h as f32 * scale;
    let dx = (stage_w - dw) * 0.5;
    let dy = TOP + (stage_h - dh) * 0.5;
    canvas2d::draw_pixels(
        canvas,
        rect(dx, dy, dw, dh),
        img.w as u32,
        img.h as u32,
        &img.px,
    )?;

    // Panel.
    let px = WIDTH - PANEL_W;
    canvas2d::fill_rect(
        canvas,
        rect(px, TOP, PANEL_W, HEIGHT - TOP - STATUS_H),
        rgb(0.10, 0.11, 0.14),
    )?;

    // Histogram.
    let hx = px + 16.0;
    let hw = PANEL_W - 32.0;
    let hh = 60.0;
    let hy = TOP + 20.0;
    canvas2d::fill_rect(canvas, rect(hx, hy, hw, hh), rgba(1.0, 1.0, 1.0, 0.04))?;
    let peak = a.hist.iter().copied().max().unwrap_or(1).max(1);
    for (i, v) in a.hist.iter().enumerate() {
        let bh = (*v as f32 / peak as f32) * hh;
        canvas2d::fill_rect(
            canvas,
            rect(
                hx + i as f32 * (hw / 64.0),
                hy + hh - bh,
                (hw / 64.0) - 0.5,
                bh.max(0.5),
            ),
            rgba(0.60, 0.78, 0.95, 0.85),
        )?;
    }

    // Sliders.
    for (i, (which, label)) in SLIDERS.iter().enumerate() {
        let y = hy + hh + 28.0 + i as f32 * 46.0;
        let on = i == a.selected;
        canvas2d::draw_text(
            canvas,
            label,
            gfx::Point { x: hx, y },
            11.5,
            if on {
                rgb(0.92, 0.95, 1.0)
            } else {
                rgb(0.55, 0.62, 0.74)
            },
        )?;
        let (v, lo, hi) = match which {
            Slider::Exposure => (a.adjust.exposure, -2.0, 2.0),
            Slider::Contrast => (a.adjust.contrast, -1.0, 1.0),
            Slider::Saturation => (a.adjust.saturation, -1.0, 1.0),
            Slider::Warmth => (a.adjust.warmth, -1.0, 1.0),
        };
        let val = f32_str(v);
        let m = canvas2d::measure_text(canvas, &val, 11.0)?;
        canvas2d::draw_text(
            canvas,
            &val,
            gfx::Point {
                x: hx + hw - m.width,
                y,
            },
            11.0,
            rgb(0.70, 0.78, 0.90),
        )?;
        let ty = y + 10.0;
        canvas2d::fill_rect(canvas, rect(hx, ty, hw, 4.0), rgba(1.0, 1.0, 1.0, 0.08))?;
        let t = (v - lo) / (hi - lo);
        canvas2d::fill_rect(
            canvas,
            rect(hx + t * hw - 4.0, ty - 4.0, 8.0, 12.0),
            if on {
                rgb(0.55, 0.85, 1.0)
            } else {
                rgb(0.45, 0.52, 0.62)
            },
        )?;
    }

    canvas2d::draw_text(
        canvas,
        "up/down pick   left/right adjust",
        gfx::Point { x: hx, y: HEIGHT - STATUS_H - 34.0 },
        10.5,
        rgb(0.42, 0.48, 0.58),
    )?;
    canvas2d::draw_text(
        canvas,
        "u undo   r reset   f full-size apply",
        gfx::Point { x: hx, y: HEIGHT - STATUS_H - 18.0 },
        10.5,
        rgb(0.42, 0.48, 0.58),
    )?;

    // Status.
    let sy = HEIGHT - STATUS_H;
    canvas2d::fill_rect(canvas, rect(0.0, sy, WIDTH, STATUS_H), rgb(0.11, 0.12, 0.15))?;
    let mut st = String::new();
    st.push_str("preview ");
    st.push_str(&u64_str(a.preview_src.w as u64));
    st.push_str("x");
    st.push_str(&u64_str(a.preview_src.h as u64));
    st.push_str("   adjust ");
    st.push_str(&u64_str(a.last_apply_us));
    st.push_str("us");
    if a.last_full_us > 0 {
        st.push_str("   full ");
        st.push_str(&u64_str(a.last_full_us / 1000));
        st.push_str("ms");
    }
    canvas2d::draw_text(
        canvas,
        &st,
        gfx::Point { x: 16.0, y: sy + 16.0 },
        11.0,
        rgb(0.55, 0.63, 0.75),
    )?;
    if !a.status.is_empty() {
        let m = canvas2d::measure_text(canvas, &a.status, 11.0)?;
        canvas2d::draw_text(
            canvas,
            &a.status,
            gfx::Point {
                x: WIDTH - 16.0 - m.width,
                y: sy + 16.0,
            },
            11.0,
            rgb(0.50, 0.85, 0.60),
        )?;
    }

    canvas2d::present(canvas)
}

// --------------------------------------------------------------------- main

fn node(id: u64, parent: Option<u64>, kind: types::WidgetKind) -> types::WidgetNode {
    types::WidgetNode {
        id,
        parent,
        kind,
        label: None,
        role: None,
        style: types::Style {
            width: None,
            height: None,
            grow: 0.0,
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

fn fail(step: &[u8]) -> i32 {
    let out = stdio::stdout();
    let _ = out.write(b"darkroom: failed at ");
    let _ = out.write(step);
    let _ = out.write(b"\n");
    1
}

fn say(msg: &str) {
    let out = stdio::stdout();
    let _ = out.write(msg.as_bytes());
}

/// Try one megapixel size and report what happened.
///
/// Each stage is reported as it completes, so a run that dies partway still
/// says exactly which allocation was the one that could not be made -- which
/// is the whole point of CP-D.
fn stress_one(label: &str, w: usize, h: usize) {
    let mp = (w * h) as f32 / 1_000_000.0;
    let one = w * h * 4;
    let mut m = String::from("\n  ");
    m.push_str(label);
    m.push_str("  ");
    m.push_str(&u64_str(w as u64));
    m.push('x');
    m.push_str(&u64_str(h as u64));
    m.push_str("  ");
    m.push_str(&f32_str(mp));
    m.push_str(" MP  one copy ");
    m.push_str(&u64_str((one / 1_048_576) as u64));
    m.push_str(" MB\n");
    say(&m);

    let t0 = clock::monotonic_nanos();
    let original = synth_photo(w, h);
    say("    original allocated\n");

    let preview_src = original.preview(PREVIEW_EDGE);
    let mut preview_out = Image::new(preview_src.w, preview_src.h);
    say("    preview built\n");

    let t1 = clock::monotonic_nanos();
    apply(&preview_src, &mut preview_out, Adjust {
        exposure: 0.4,
        contrast: 0.2,
        saturation: 0.3,
        warmth: 0.1,
    });
    let t2 = clock::monotonic_nanos();

    // Now the full-size apply, which needs a SECOND full buffer -- the
    // allocation an editor cannot avoid on export.
    let mut full_out = Image::new(w, h);
    say("    second full buffer allocated\n");
    let t3 = clock::monotonic_nanos();
    apply(&original, &mut full_out, Adjust {
        exposure: 0.4,
        contrast: 0.2,
        saturation: 0.3,
        warmth: 0.1,
    });
    let t4 = clock::monotonic_nanos();

    let mut r = String::from("    load ");
    r.push_str(&u64_str((t1 - t0) / 1_000_000));
    r.push_str("ms  preview-adjust ");
    r.push_str(&u64_str((t2 - t1) / 1_000));
    r.push_str("us  full-adjust ");
    r.push_str(&u64_str((t4 - t3) / 1_000_000));
    r.push_str("ms  held ");
    r.push_str(&u64_str(
        ((original.bytes() + preview_src.bytes() + preview_out.bytes() + full_out.bytes())
            / 1_048_576) as u64,
    ));
    r.push_str(" MB\n");
    say(&r);
}

struct Component;

impl krate::Guest for Component {
    fn run() -> i32 {
        let raw = args::raw();
        let has = |n: &[u8]| raw.as_bytes().split(|b| *b == b'\n').any(|a| a == n);
        let quick = has(b"quick") || has(b"--quick");
        let stress = has(b"stress");
        // `big` needs a raised ceiling: `krate run ... --mem-limit 1024`.
        let has_big = has(b"big");

        if stress {
            say("darkroom-stress: how big a photo fits in the guest ceiling\n");
            stress_one("8 MP  ", 3264, 2448);
            stress_one("12 MP ", 4032, 3024);
            stress_one("24 MP ", 6000, 4000);
            // 45 MP and up TRAP at the default 256 MB ceiling: the original
            // and preview allocate, and the second full buffer dies inside
            // dlmalloc. With `--mem-limit 1024` the same binary reaches
            // 100 MP (782 MB held, 532 ms full adjust), so the ceiling is the
            // only limit -- the pixel path itself scales linearly. Left out
            // of the default run because a trap is not a result.
            if has_big {
                stress_one("45 MP ", 8192, 5464);
                stress_one("61 MP ", 9504, 6336);
                stress_one("100 MP", 11648, 8736);
            }
            say("\ndarkroom-stress: done\n");
            return 0;
        }

        // The interactive app works on a 12 MP photo, which is what a phone
        // takes and what the arithmetic says should fit.
        let original = synth_photo(4032, 3024);
        let preview_src = original.preview(PREVIEW_EDGE);
        let preview_out = Image::new(preview_src.w, preview_src.h);

        let mut app = App {
            original,
            preview_src,
            preview_out,
            hist: [0; 64],
            adjust: Adjust::none(),
            committed: Adjust::none(),
            undo: Vec::new(),
            selected: 0,
            name: String::from("beach.jpg"),
            status: String::new(),
            last_apply_us: 0,
            last_full_us: 0,
        };
        app.recompute();

        let win = match window::create(
            "Darkroom",
            types::WindowSize {
                width: WIDTH as u32,
                height: HEIGHT as u32,
            },
        ) {
            Ok(w) => w,
            Err(_) => return fail(b"window::create"),
        };
        if window::show(win).is_err() {
            return fail(b"window::show");
        }
        if tree::set_root(win, &node(ROOT_ID, None, types::WidgetKind::Stack)).is_err() {
            return fail(b"set_root");
        }
        if tree::upsert_node(
            win,
            &node(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas),
        )
        .is_err()
        {
            return fail(b"upsert canvas");
        }
        let canvas = match canvas2d::bind(win, CANVAS_ID) {
            Ok(c) => c,
            Err(_) => return fail(b"canvas2d::bind"),
        };
        let _ = canvas2d::set_design_size(
            canvas,
            gfx::Size {
                width: WIDTH,
                height: HEIGHT,
            },
        );

        let _ = draw(&app, canvas);

        if quick {
            let mut m = String::from("darkroom: ");
            m.push_str(&u64_str(app.original.w as u64));
            m.push('x');
            m.push_str(&u64_str(app.original.h as u64));
            m.push_str(", preview adjust ");
            m.push_str(&u64_str(app.last_apply_us));
            m.push_str("us\n");
            say(&m);
            return 0;
        }

        loop {
            let mut dirty = false;
            while let Some(ev) = events::poll() {
                match ev {
                    types::Event::CloseRequested(id) => {
                        let _ = window::close(id);
                        return 0;
                    }
                    types::Event::Key(k) if k.pressed => match k.key.as_str() {
                        "ArrowUp" => {
                            app.selected = (app.selected + SLIDERS.len() - 1) % SLIDERS.len();
                            dirty = true;
                        }
                        "ArrowDown" => {
                            app.selected = (app.selected + 1) % SLIDERS.len();
                            dirty = true;
                        }
                        "ArrowLeft" => {
                            app.bump(-0.05);
                            dirty = true;
                        }
                        "ArrowRight" => {
                            app.bump(0.05);
                            dirty = true;
                        }
                        "u" | "U" => {
                            if let Some(prev) = app.undo.pop() {
                                app.adjust = prev;
                                app.recompute();
                                dirty = true;
                            }
                        }
                        "r" | "R" => {
                            app.undo.push(app.adjust);
                            app.adjust = Adjust::none();
                            app.recompute();
                            dirty = true;
                        }
                        "f" | "F" => {
                            // Full-size apply: the allocation an editor cannot
                            // avoid, timed so the cost is visible.
                            let t = clock::monotonic_nanos();
                            let mut full = Image::new(app.original.w, app.original.h);
                            apply(&app.original, &mut full, app.adjust);
                            app.last_full_us = clock::monotonic_nanos().saturating_sub(t) / 1_000;
                            app.committed = app.adjust;
                            app.status.clear();
                            app.status.push_str("applied at full size");
                            dirty = true;
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
            if dirty && draw(&app, canvas).is_err() {
                break;
            }
            if !dirty && draw(&app, canvas).is_err() {
                break;
            }
        }
        0
    }
}

krate::export!(Component);
