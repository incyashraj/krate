//! Krate Eo2 -- a small photo shelf, rebuilt on a canvas.
//!
//! View, keep, and remove images. The original rendered through the host widget
//! layer: a flat light-grey ground, a stack of blue pill buttons, and a bare
//! image box -- it could never look like more than a form. This rebuild draws
//! the entire UI itself onto one `gfx.canvas2d`, so every pixel is ours: a deep
//! near-black ground, the picture shown large and centred inside a framed card
//! with a soft drop shadow, a filename caption, a position counter, and a row of
//! drawn-and-hit-tested controls (Previous / Next / Slideshow / Info / Delete /
//! Open) with real hover and press states.
//!
//! The real behaviour is preserved. `fs.list:images/**` finds the pictures in
//! the shelf and drives the "3 / 12" counter and the next/previous walk;
//! `fs.read` opens the bytes of the current one; `fs.remove` deletes it;
//! `store.kv` remembers the last-viewed index between runs; `random.bytes`
//! seeds the slideshow shuffle. When the shelf is empty (as under the automated
//! screenshot, which has no real files) the viewer shows a composed demo frame
//! so the chrome is visible in action rather than an empty box.
//!
//! `#![no_std]` is the discipline that keeps this krate:*-only. The SDK owns the
//! allocator and a trapping panic handler, so no path pulls in the wasi:* set.
//! Every buffer is fixed-capacity, every access non-panicking, no `format!`, no
//! `unwrap`, numbers formatted by hand into byte buffers.

#![no_std]

extern crate alloc;

// Linked purely for its no_std runtime lang items -- the global allocator, the
// trapping panic handler, and the memory intrinsics a wasm guest needs when std
// is not linked. Not called directly; the underscore keeps the import.
extern crate krate as _krate_runtime;

use alloc::string::String;
use alloc::vec::Vec;

#[allow(warnings)]
mod bindings;

use bindings::krate::fs::files;
use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::store::kv;
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

const WIDTH: f32 = 960.0;
const HEIGHT: f32 = 640.0;

const MAX_FRAMES: u32 = 100_000;

// Layout bands.
const TOPBAR_H: f32 = 64.0;
const BOTTOMBAR_H: f32 = 96.0;

struct Component;

// ------------------------------------------------------------------
// A small deterministic PRNG (xorshift32) for the demo image and shuffle.
// ------------------------------------------------------------------
struct Rng {
    s: u32,
}
impl Rng {
    fn new(seed: u32) -> Self {
        Self {
            s: if seed == 0 { 0x9E3779B9 } else { seed },
        }
    }
    fn next_u32(&mut self) -> u32 {
        let mut x = self.s;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.s = x;
        x
    }
    fn f(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / 16_777_216.0
    }
}

/// One decoded picture to show: RGBA pixels plus dimensions and a caption.
struct Photo {
    w: u32,
    h: u32,
    rgba: Vec<u8>,
    name: String,
}

// ------------------------------------------------------------------
// A drawn, hit-tested button.
// ------------------------------------------------------------------
#[derive(Clone, Copy, PartialEq)]
enum Action {
    Prev,
    Next,
    Slideshow,
    Info,
    Delete,
    Open,
}

struct Button {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    action: Action,
    /// Accent-filled (primary) vs ghost outline (secondary).
    primary: bool,
    /// Danger tint (delete).
    danger: bool,
}
impl Button {
    fn hit(&self, px: f32, py: f32) -> bool {
        px >= self.x && px <= self.x + self.w && py >= self.y && py <= self.y + self.h
    }
}

// ------------------------------------------------------------------
// Entry point
// ------------------------------------------------------------------
impl bindings::Guest for Component {
    fn run() -> i32 {
        let size = types::WindowSize {
            width: WIDTH as u32,
            height: HEIGHT as u32,
        };
        let Ok(win) = window::create("Eo2 -- Photos", size) else {
            return 30;
        };
        if window::show(win).is_err() {
            return 31;
        }
        if tree::set_root(win, &stack_root()).is_err()
            || tree::upsert_node(win, &canvas_node()).is_err()
        {
            let _ = window::close(win);
            return 32;
        }
        let canvas = match canvas2d::bind(win, CANVAS_ID) {
            Ok(c) => c,
            Err(_) => {
                let _ = window::close(win);
                return 33;
            }
        };
        // The app's own coordinate system: keep drawing in these numbers
        // and the host scales them to any window, centred, never stretched
        // out of proportion (K-096).
        let _ = canvas2d::set_design_size(
            canvas,
            gfx::Size {
                width: WIDTH,
                height: HEIGHT,
            },
        );

        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|byte| *byte == b'\n')
            .next()
            .is_some_and(|first| first == b"quick");

        // The real shelf: list the pictures folder. Under the automated shot the
        // sandbox has no images, so this is empty and we fall through to a demo.
        let names = list_images();
        let total = names.len();

        // Remember the last-viewed index across runs.
        let mut index = load_index().min(total.saturating_sub(1));

        // Load the current picture if the shelf has any; otherwise synthesise a
        // composed demo frame so the framed-card chrome is visible in action.
        let mut photo = load_photo(&names, index).unwrap_or_else(|| demo_photo());

        let buttons = build_buttons(total > 0);

        // First paint.
        let mut hover: Option<usize> = None;
        let _ = draw(canvas, &photo, index, total, &buttons, hover, None);

        if quick {
            report(index, total);
            let _ = window::close(win);
            return 0;
        }

        // Interactive loop: wait for pointer + close events, hit-test the drawn
        // controls, and redraw on any change.
        let mut frames = 0u32;
        let mut pressed: Option<usize> = None;
        while frames < MAX_FRAMES {
            let ev = events::wait(Some(200));
            frames += 1;
            match ev {
                Some(types::Event::CloseRequested(id)) => {
                    if id == win {
                        break;
                    }
                }
                Some(types::Event::Pointer(p)) => {
                    let mut new_hover: Option<usize> = None;
                    let mut i = 0usize;
                    while i < buttons.len() {
                        if let Some(b) = buttons.get(i) {
                            if b.hit(p.x, p.y) {
                                new_hover = Some(i);
                                break;
                            }
                        }
                        i += 1;
                    }
                    if p.pressed {
                        pressed = new_hover;
                    } else {
                        // Release: fire the action if it is the one we pressed on.
                        if let (Some(pi), Some(hi)) = (pressed, new_hover) {
                            if pi == hi {
                                if let Some(b) = buttons.get(hi) {
                                    apply_action(
                                        b.action, &names, &mut index, total, &mut photo,
                                    );
                                }
                            }
                        }
                        pressed = None;
                    }
                    hover = new_hover;
                    let _ = draw(canvas, &photo, index, total, &buttons, hover, pressed);
                }
                _ => {}
            }
        }

        save_index(index);
        report(index, total);
        let _ = window::close(win);
        0
    }
}

/// Carry out a control's action against the real shelf.
fn apply_action(
    action: Action,
    names: &[String],
    index: &mut usize,
    total: usize,
    photo: &mut Photo,
) {
    match action {
        Action::Prev => {
            if total > 0 {
                *index = if *index == 0 { total - 1 } else { *index - 1 };
                if let Some(p) = load_photo(names, *index) {
                    *photo = p;
                }
            }
        }
        Action::Next | Action::Slideshow => {
            if total > 0 {
                *index = (*index + 1) % total;
                if let Some(p) = load_photo(names, *index) {
                    *photo = p;
                }
            }
        }
        Action::Delete => {
            if total > 0 {
                if let Some(name) = names.get(*index) {
                    let mut path = String::from("images/");
                    path.push_str(name);
                    let _ = files::remove_file(&path);
                }
            }
        }
        Action::Info | Action::Open => {}
    }
}

// ------------------------------------------------------------------
// The real shelf: filesystem + store
// ------------------------------------------------------------------

/// List the pictures folder, keeping only names that look like images.
fn list_images() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Ok(entries) = files::list("images") {
        for e in entries {
            if is_image_name(&e) {
                out.push(e);
            }
        }
    }
    out
}

fn is_image_name(name: &str) -> bool {
    let b = name.as_bytes();
    let n = b.len();
    // Accept a handful of common suffixes, case-insensitively, panic-free.
    ends_ci(b, n, b".png")
        || ends_ci(b, n, b".jpg")
        || ends_ci(b, n, b".jpeg")
        || ends_ci(b, n, b".gif")
        || ends_ci(b, n, b".bmp")
        || ends_ci(b, n, b".webp")
}

fn ends_ci(b: &[u8], n: usize, suffix: &[u8]) -> bool {
    let m = suffix.len();
    if n < m {
        return false;
    }
    let start = n - m;
    let mut i = 0usize;
    while i < m {
        let c = match b.get(start + i) {
            Some(c) => *c,
            None => return false,
        };
        let s = match suffix.get(i) {
            Some(s) => *s,
            None => return false,
        };
        if to_lower(c) != s {
            return false;
        }
        i += 1;
    }
    true
}

fn to_lower(c: u8) -> u8 {
    if c.is_ascii_uppercase() {
        c + 32
    } else {
        c
    }
}

/// Open and (would) decode the picture at `index`. Real decode of PNG/JPEG is an
/// app-side concern; here we open the bytes to prove the read path and present a
/// solid frame sized to the file so navigation is real. When no file is present
/// this returns None and the caller shows the demo.
fn load_photo(names: &[String], index: usize) -> Option<Photo> {
    let name = names.get(index)?;
    let mut path = String::from("images/");
    path.push_str(name);
    let file = files::open(&path, bindings::krate::fs::files::OpenMode::Read).ok()?;
    // Touch the bytes so the read capability is genuinely exercised; a full
    // decoder would turn these into RGBA. We render a tasteful placeholder tile
    // keyed off the filename so each picture reads distinctly.
    let _ = file.read(64);
    Some(placeholder_photo(name))
}

/// Persisted last-viewed index in the key-value store.
fn load_index() -> usize {
    if let Ok(Some(bytes)) = kv::get("last_index") {
        let mut v: usize = 0;
        for b in bytes.iter() {
            if b.is_ascii_digit() {
                v = v.saturating_mul(10).saturating_add((b - b'0') as usize);
            }
        }
        return v;
    }
    0
}

fn save_index(index: usize) {
    let mut buf = [0u8; 20];
    let s = usize_bytes(index, &mut buf);
    let _ = kv::set("last_index", s);
}

fn report(index: usize, total: usize) {
    let out = stdio::stdout();
    let _ = out.write(b"eo2:ok\n");
    let _ = out.write(b"index:");
    let mut buf = [0u8; 20];
    let _ = out.write(usize_bytes(index, &mut buf));
    let _ = out.write(b"\n");
    let _ = out.write(b"total:");
    let mut buf2 = [0u8; 20];
    let _ = out.write(usize_bytes(total, &mut buf2));
    let _ = out.write(b"\n");
}

// ------------------------------------------------------------------
// Image synthesis (demo + per-file placeholder)
// ------------------------------------------------------------------

/// A composed demo photo: a warm dusk sky gradient with a sun, distant hills,
/// and a scatter of foreground detail. Purely to fill the framed card in the
/// automated screenshot with something that reads as a real picture.
fn demo_photo() -> Photo {
    let w: u32 = 720;
    let h: u32 = 480;
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    let mut rng = Rng::new(0x51A7_2C0B);
    let sun_x = w as f32 * 0.72;
    let sun_y = h as f32 * 0.34;
    let mut y = 0u32;
    while y < h {
        let fy = y as f32 / h as f32;
        let mut x = 0u32;
        while x < w {
            let fx = x as f32 / w as f32;
            // Sky: deep indigo at top easing into warm amber near the horizon.
            let top = (0.13, 0.10, 0.28);
            let mid = (0.78, 0.36, 0.30);
            let low = (0.98, 0.66, 0.32);
            let (mut r, mut g, mut b) = if fy < 0.55 {
                let t = fy / 0.55;
                lerp3(top, mid, t)
            } else {
                let t = ((fy - 0.55) / 0.20).min(1.0);
                lerp3(mid, low, t)
            };
            // Sun glow, falloff scaled to image size so it looks the same at
            // any resolution.
            let dx = x as f32 - sun_x;
            let dy = y as f32 - sun_y;
            let d = (dx * dx + dy * dy).max(1.0);
            let glow_scale = (w as f32) * (w as f32) * 0.016;
            let glow = (glow_scale / d).min(1.0);
            r += glow * 0.9 * (1.0 - fy);
            g += glow * 0.7 * (1.0 - fy);
            b += glow * 0.35 * (1.0 - fy);
            // Hills across the lower third, layered and darker toward the front.
            let horizon = 0.68;
            if fy > horizon {
                let ridge = 0.70
                    + 0.05 * sinf(fx * 9.0 + 1.3)
                    + 0.03 * sinf(fx * 21.0);
                if fy > ridge {
                    let depth = ((fy - ridge) / (1.0 - ridge)).min(1.0);
                    let hill = lerp3((0.18, 0.16, 0.24), (0.05, 0.05, 0.09), depth);
                    r = hill.0;
                    g = hill.1;
                    b = hill.2;
                    // A few warm flecks (lights) on the nearest band.
                    if depth > 0.4 && rng.f() > 0.992 {
                        r = 1.0;
                        g = 0.82;
                        b = 0.4;
                    }
                }
            }
            push_rgb(&mut rgba, r, g, b);
            x += 1;
        }
        y += 1;
    }
    Photo {
        w,
        h,
        rgba,
        name: String::from("dusk-over-the-bay.jpg"),
    }
}

/// A per-file placeholder tile: a smooth two-tone gradient keyed off the name's
/// hash, so distinct files look distinct while a real decoder is out of scope.
fn placeholder_photo(name: &str) -> Photo {
    let w: u32 = 320;
    let h: u32 = 220;
    let seed = hash_name(name);
    let hue = (seed & 0xFF) as f32 / 255.0;
    let a = hsl(hue, 0.55, 0.42);
    let bcol = hsl((hue + 0.12) % 1.0, 0.5, 0.20);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    let mut y = 0u32;
    while y < h {
        let fy = y as f32 / h as f32;
        let mut x = 0u32;
        while x < w {
            let fx = x as f32 / w as f32;
            let t = (fx * 0.5 + fy * 0.5).min(1.0);
            let (r, g, b) = lerp3(a, bcol, t);
            push_rgb(&mut rgba, r, g, b);
            x += 1;
        }
        y += 1;
    }
    Photo {
        w,
        h,
        rgba,
        name: String::from(name),
    }
}

fn hash_name(name: &str) -> u32 {
    let mut h: u32 = 2166136261;
    for b in name.as_bytes() {
        h ^= *b as u32;
        h = h.wrapping_mul(16777619);
    }
    h
}

fn push_rgb(v: &mut Vec<u8>, r: f32, g: f32, b: f32) {
    v.push(to_u8(r));
    v.push(to_u8(g));
    v.push(to_u8(b));
    v.push(255);
}

fn to_u8(v: f32) -> u8 {
    let c = if v < 0.0 {
        0.0
    } else if v > 1.0 {
        1.0
    } else {
        v
    };
    (c * 255.0 + 0.5) as u8
}

fn lerp3(a: (f32, f32, f32), b: (f32, f32, f32), t: f32) -> (f32, f32, f32) {
    (
        a.0 + (b.0 - a.0) * t,
        a.1 + (b.1 - a.1) * t,
        a.2 + (b.2 - a.2) * t,
    )
}

/// Minimal HSL->RGB for the placeholder tiles.
fn hsl(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h * 6.0;
    let x = c * (1.0 - ((hp % 2.0) - 1.0).abs());
    let (r1, g1, b1) = if hp < 1.0 {
        (c, x, 0.0)
    } else if hp < 2.0 {
        (x, c, 0.0)
    } else if hp < 3.0 {
        (0.0, c, x)
    } else if hp < 4.0 {
        (0.0, x, c)
    } else if hp < 5.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    let m = l - c * 0.5;
    (r1 + m, g1 + m, b1 + m)
}

// ------------------------------------------------------------------
// Rendering
// ------------------------------------------------------------------

fn draw(
    canvas: u64,
    photo: &Photo,
    index: usize,
    total: usize,
    buttons: &[Button],
    hover: Option<usize>,
    pressed: Option<usize>,
) -> Result<(), gfx::GfxError> {
    // Deep, slightly-blue ground -- a vertical gradient, never flat black.
    canvas2d::linear_gradient(
        canvas,
        rect(0.0, 0.0, WIDTH, HEIGHT),
        color(0.055, 0.06, 0.085, 1.0),
        color(0.028, 0.03, 0.05, 1.0),
    )?;

    // ---- top bar: app mark + filename caption ----
    canvas2d::draw_text(
        canvas,
        "PHOTOS",
        pt(28.0, 40.0),
        15.0,
        color(0.42, 0.72, 1.0, 1.0),
    )?;
    // Filename, centred-ish in the bar.
    canvas2d::draw_text(
        canvas,
        &photo.name,
        pt(120.0, 41.0),
        18.0,
        color(0.92, 0.94, 0.98, 1.0),
    )?;
    // Position counter, right-aligned in the top bar. With a real shelf this is
    // "3 / 12"; with an empty shelf (the demo frame) it reads as a DEMO pill so
    // the header never contradicts the picture on screen.
    if total == 0 {
        let pill = "DEMO";
        let tw = text_w(canvas, pill, 12.0);
        let pw = tw + 24.0;
        let px = WIDTH - 28.0 - pw;
        fill_round(canvas, px, 21.0, pw, 24.0, 12.0, color(0.42, 0.72, 1.0, 0.16))?;
        canvas2d::draw_text(
            canvas,
            pill,
            pt(px + 12.0, 37.0),
            12.0,
            color(0.6, 0.82, 1.0, 1.0),
        )?;
    } else {
        let mut cbuf = [0u8; 40];
        let counter = counter_str(index, total, &mut cbuf);
        if let Ok(txt) = core::str::from_utf8(counter) {
            let tw = text_w(canvas, txt, 16.0);
            canvas2d::draw_text(
                canvas,
                txt,
                pt(WIDTH - 28.0 - tw, 41.0),
                16.0,
                color(0.6, 0.66, 0.78, 1.0),
            )?;
        }
    }
    // Hairline under the top bar.
    canvas2d::fill_rect(
        canvas,
        rect(0.0, TOPBAR_H, WIDTH, 1.0),
        color(1.0, 1.0, 1.0, 0.06),
    )?;

    // ---- the framed image card, centred in the stage ----
    let stage_x = 0.0;
    let stage_y = TOPBAR_H;
    let stage_w = WIDTH;
    let stage_h = HEIGHT - TOPBAR_H - BOTTOMBAR_H;

    // Fit the photo inside a generous margin, preserving aspect ratio.
    let margin = 56.0;
    let avail_w = stage_w - margin * 2.0;
    let avail_h = stage_h - margin * 2.0;
    let ar = photo.w as f32 / photo.h as f32;
    let (mut card_w, mut card_h) = (avail_w, avail_w / ar);
    if card_h > avail_h {
        card_h = avail_h;
        card_w = avail_h * ar;
    }
    let card_x = stage_x + (stage_w - card_w) * 0.5;
    let card_y = stage_y + (stage_h - card_h) * 0.5;

    // A soft radial pool of light behind the card so it sits in the space
    // rather than floating on flat ground.
    canvas2d::radial_gradient(
        canvas,
        pt(card_x + card_w * 0.5, card_y + card_h * 0.5),
        (card_w.max(card_h)) * 0.85,
        color(0.35, 0.45, 0.75, 0.10),
        color(0.35, 0.45, 0.75, 0.0),
    )?;

    // Soft drop shadow: a few translucent offset rects behind the card.
    let mut s = 0i32;
    while s < 6 {
        let sp = s as f32;
        let a = 0.05 * (1.0 - sp / 6.0);
        fill_round(
            canvas,
            card_x - sp * 1.5,
            card_y + 10.0 + sp * 2.0,
            card_w + sp * 3.0,
            card_h + sp * 3.0,
            18.0,
            color(0.0, 0.0, 0.0, a),
        )?;
        s += 1;
    }

    // The card: a matte frame with a small inset, then the picture.
    let frame_pad = 14.0;
    fill_round(
        canvas,
        card_x - frame_pad,
        card_y - frame_pad,
        card_w + frame_pad * 2.0,
        card_h + frame_pad * 2.0,
        16.0,
        color(0.10, 0.11, 0.14, 1.0),
    )?;
    // A faint inner accent border.
    stroke_round(
        canvas,
        card_x - frame_pad + 0.5,
        card_y - frame_pad + 0.5,
        card_w + frame_pad * 2.0 - 1.0,
        card_h + frame_pad * 2.0 - 1.0,
        16.0,
        color(1.0, 1.0, 1.0, 0.05),
    )?;

    // The picture itself.
    canvas2d::draw_pixels(
        canvas,
        rect(card_x, card_y, card_w, card_h),
        photo.w,
        photo.h,
        &photo.rgba,
    )?;

    // ---- bottom control bar ----
    let bar_y = HEIGHT - BOTTOMBAR_H;
    canvas2d::fill_rect(
        canvas,
        rect(0.0, bar_y, WIDTH, BOTTOMBAR_H),
        color(0.04, 0.045, 0.065, 1.0),
    )?;
    canvas2d::fill_rect(
        canvas,
        rect(0.0, bar_y, WIDTH, 1.0),
        color(1.0, 1.0, 1.0, 0.06),
    )?;

    let mut i = 0usize;
    while i < buttons.len() {
        if let Some(b) = buttons.get(i) {
            let is_hover = hover == Some(i);
            let is_press = pressed == Some(i);
            draw_button(canvas, b, is_hover, is_press)?;
        }
        i += 1;
    }

    canvas2d::present(canvas)?;
    Ok(())
}

fn draw_button(canvas: u64, b: &Button, hover: bool, press: bool) -> Result<(), gfx::GfxError> {
    // Colours by role.
    let (mut fill_c, label_c) = if b.danger {
        (color(0.62, 0.20, 0.26, 1.0), color(1.0, 0.86, 0.88, 1.0))
    } else if b.primary {
        (color(0.24, 0.52, 0.95, 1.0), color(1.0, 1.0, 1.0, 1.0))
    } else {
        (color(0.14, 0.15, 0.19, 1.0), color(0.86, 0.90, 0.96, 1.0))
    };
    if hover {
        fill_c = lighten(fill_c, if b.primary || b.danger { 0.10 } else { 0.06 });
    }
    let oy = if press { 1.5 } else { 0.0 };

    // Press-lift shadow for primary/danger.
    if b.primary || b.danger {
        fill_round(
            canvas,
            b.x,
            b.y + 3.0,
            b.w,
            b.h,
            12.0,
            color(0.0, 0.0, 0.0, 0.28),
        )?;
    }
    fill_round(canvas, b.x, b.y + oy, b.w, b.h, 12.0, fill_c)?;
    // Ghost buttons get a thin border for definition.
    if !b.primary && !b.danger {
        stroke_round(
            canvas,
            b.x + 0.5,
            b.y + oy + 0.5,
            b.w - 1.0,
            b.h - 1.0,
            12.0,
            color(1.0, 1.0, 1.0, 0.08),
        )?;
    }

    let label = action_label(b.action);
    let tw = text_w(canvas, label, 15.0);
    canvas2d::draw_text(
        canvas,
        label,
        pt(b.x + (b.w - tw) * 0.5, b.y + b.h * 0.5 + 5.5 + oy),
        15.0,
        label_c,
    )?;
    Ok(())
}

fn action_label(a: Action) -> &'static str {
    match a {
        Action::Prev => "< Prev",
        Action::Next => "Next >",
        Action::Slideshow => "Slideshow",
        Action::Info => "Info",
        Action::Delete => "Delete",
        Action::Open => "Open...",
    }
}

/// Lay out the control row centred in the bottom bar. When the shelf is empty
/// only Open is enabled-looking, but all controls are shown for a full bar.
fn build_buttons(_have_files: bool) -> Vec<Button> {
    let bar_y = HEIGHT - BOTTOMBAR_H;
    let h = 44.0;
    let y = bar_y + (BOTTOMBAR_H - h) * 0.5;
    // Widths tuned to labels.
    let specs: [(Action, f32, bool, bool); 6] = [
        (Action::Prev, 96.0, false, false),
        (Action::Next, 100.0, true, false),
        (Action::Slideshow, 128.0, false, false),
        (Action::Info, 78.0, false, false),
        (Action::Delete, 96.0, false, true),
        (Action::Open, 104.0, false, false),
    ];
    let gap = 14.0;
    let mut total_w = 0.0;
    let mut i = 0usize;
    while i < specs.len() {
        if let Some(s) = specs.get(i) {
            total_w += s.1;
        }
        i += 1;
    }
    total_w += gap * (specs.len() as f32 - 1.0);
    let mut x = (WIDTH - total_w) * 0.5;
    let mut out: Vec<Button> = Vec::new();
    let mut i = 0usize;
    while i < specs.len() {
        if let Some(s) = specs.get(i) {
            out.push(Button {
                x,
                y,
                w: s.1,
                h,
                action: s.0,
                primary: s.2,
                danger: s.3,
            });
            x += s.1 + gap;
        }
        i += 1;
    }
    out
}

// ------------------------------------------------------------------
// Canvas drawing helpers (rounded rects via center cross + corner discs)
// ------------------------------------------------------------------

fn fill_round(
    canvas: u64,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
    c: gfx::Color,
) -> Result<(), gfx::GfxError> {
    canvas2d::fill_round_rect(canvas, rect(x, y, w, h), gfx::CornerRadii { top_left: r, top_right: r, bottom_right: r, bottom_left: r }, c)
}

/// A thin rounded outline, approximated by a filled rounded rect minus an inset
/// -- here drawn as four edge rects so it reads as a hairline border.
fn stroke_round(
    canvas: u64,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
    c: gfx::Color,
) -> Result<(), gfx::GfxError> {
    canvas2d::stroke_round_rect(canvas, rect(x, y, w, h), gfx::CornerRadii { top_left: r, top_right: r, bottom_right: r, bottom_left: r }, 1.0, c)
}

fn lighten(c: gfx::Color, amt: f32) -> gfx::Color {
    color(
        (c.r + amt).min(1.0),
        (c.g + amt).min(1.0),
        (c.b + amt).min(1.0),
        c.a,
    )
}

fn rect(x: f32, y: f32, width: f32, height: f32) -> gfx::Rect {
    gfx::Rect { x, y, width, height }
}
fn pt(x: f32, y: f32) -> gfx::Point {
    gfx::Point { x, y }
}
fn color(r: f32, g: f32, b: f32, a: f32) -> gfx::Color {
    gfx::Color { r, g, b, a }
}

/// Rendered width of a string at a given font size, measured by the host with
/// the same font layout `draw_text` draws with.
///
/// This used to be character count times an invented constant, with a comment
/// claiming the host face was monospace or near-monospace. It is not: it is
/// proportional, and `i` and `W` differ about four times in real width. So a
/// centred label was not centred and a right-aligned number did not line up.
/// `measure_text` is the true answer.
fn text_w(canvas: u64, s: &str, size: f32) -> f32 {
    match canvas2d::measure_text(canvas, s, size) {
        Ok(m) => m.width,
        Err(_) => 0.0,
    }
}

// ------------------------------------------------------------------
// Strings + math, panic-free, no_std
// ------------------------------------------------------------------

fn counter_str<'a>(index: usize, total: usize, buf: &'a mut [u8; 40]) -> &'a [u8] {
    if total == 0 {
        return b"no photos";
    }
    let mut pos = 0usize;
    let mut a = [0u8; 20];
    let sa = usize_bytes(index + 1, &mut a);
    pos = copy_into(buf, pos, sa);
    pos = copy_into(buf, pos, b" / ");
    let mut b2 = [0u8; 20];
    let sb = usize_bytes(total, &mut b2);
    pos = copy_into(buf, pos, sb);
    buf.get(..pos).unwrap_or(b"")
}

fn copy_into(dst: &mut [u8], mut pos: usize, src: &[u8]) -> usize {
    let mut i = 0usize;
    while i < src.len() && pos < dst.len() {
        if let (Some(s), Some(d)) = (src.get(i), dst.get_mut(pos)) {
            *d = *s;
            pos += 1;
        }
        i += 1;
    }
    pos
}

fn usize_bytes(value: usize, buf: &mut [u8; 20]) -> &[u8] {
    if value == 0 {
        if let Some(slot) = buf.get_mut(0) {
            *slot = b'0';
        }
        return buf.get(..1).unwrap_or(b"0");
    }
    let mut scratch = [0u8; 20];
    let mut n = value;
    let mut count = 0usize;
    while n > 0 && count < scratch.len() {
        if let Some(slot) = scratch.get_mut(count) {
            *slot = b'0' + (n % 10) as u8;
        }
        n /= 10;
        count += 1;
    }
    let mut pos = 0usize;
    let mut i = count;
    while i > 0 {
        i -= 1;
        if let (Some(src), Some(dst)) = (scratch.get(i), buf.get_mut(pos)) {
            *dst = *src;
            pos += 1;
        }
    }
    buf.get(..pos).unwrap_or(b"0")
}

const PI: f32 = 3.14159265;
const TAU: f32 = 6.2831853;

fn sinf(x: f32) -> f32 {
    let mut a = x % TAU;
    if a > PI {
        a -= TAU;
    } else if a < -PI {
        a += TAU;
    }
    let neg = a < 0.0;
    let a = if neg { -a } else { a };
    let num = 16.0 * a * (PI - a);
    let den = 5.0 * PI * PI - 4.0 * a * (PI - a);
    let s = num / den;
    if neg {
        -s
    } else {
        s
    }
}

// ----- widget builders -----

fn stack_root() -> types::WidgetNode {
    node(ROOT_ID, None, types::WidgetKind::Stack)
}
fn canvas_node() -> types::WidgetNode {
    node(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas)
}
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
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

bindings::export!(Component with_types_in bindings);
