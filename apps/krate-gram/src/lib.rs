//! Krategram -- the acceptance test the modern-UI plan is built around:
//! could an AI build an Instagram-looking app on Krate, and could Krate
//! host it well?
//!
//! A photo feed: stories with gradient rings, photo cards with rounded
//! corners and soft shadows, momentum scrolling with rubber-banding, and a
//! double-tap heart that springs and fades. Every pixel comes from canvas2d
//! primitives; the "photos" are generative art rendered by this app into
//! pixel buffers, because a feed app should not need the network to prove
//! the renderer.
//!
//! It lays out from the screen it is on: the canvas's real size, the edges
//! a phone covers (the notch, the home bar), and the person's look -- dark
//! or light, and every text size from `look.text`, with the rows growing
//! so larger text still fits. Each frame tells a screen reader what it
//! shows with `Said`: the title, the stories, and each post's name, photo,
//! like button, likes and caption.
//!
//! Note the manifest next to this file: the whole thing needs one
//! privileged capability -- a window.
//!
//! `quick` draws real frames with synthetic scrolling and prints markers,
//! so a headless check exercises the same paint path a person sees.

#![no_std]

extern crate alloc;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use krate::bindings::krate::io::{args, stdio};
use krate::bindings::krate::time::clock;
use krate::gfx::{canvas2d, canvas_images, types as gfx};
use krate::layout::{Look, Rect, Said, Screen};
use krate::motion::Spring;
use krate::ui::{events, tree, types, window};
use libm::{cosf, sinf, sqrtf};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

/// The size the window is asked for. What the host actually grants is the
/// law -- a phone hands back its whole screen -- so every frame lays out
/// from canvas-size, never from these numbers (K-087).
const REQUEST_W: f32 = 390.0;
const REQUEST_H: f32 = 720.0;

const TABBAR_H: f32 = 56.0;
const POST_GAP: f32 = 18.0;
/// The story ring's radius.
const RING: f32 = 29.0;
/// The like button's heart, drawn this many points square.
const HEART: f32 = 26.0;

/// The generated art's own resolution. It is drawn into whatever rect the
/// layout gives it; the sampler scales.
const ART_W: u32 = 358;
const ART_H: u32 = 300;

/// Everything the frame needs to know about the surface it is drawing on,
/// worked out again whenever the canvas, the covered edges or the text
/// size change, so a resize -- or a phone-sized surface, or larger text --
/// is simply the next frame's truth.
#[derive(Clone, Copy, PartialEq)]
struct Layout {
    w: f32,
    h: f32,
    /// The covered edges at the top and bottom: the notch and the home bar.
    safe_top: f32,
    safe_bottom: f32,
    /// The feed column: capped for readability, centered when the surface
    /// is wider than a phone, and clear of covered side edges.
    col_x: f32,
    col_w: f32,
    /// The photo rect inside the column, aspect matched to the art.
    photo_w: f32,
    photo_h: f32,
    /// Text sizes at the person's text size.
    title: f32,
    name: f32,
    user: f32,
    sub: f32,
    body: f32,
    /// Row heights, grown with the text.
    header_h: f32,
    stories_h: f32,
    post_header_h: f32,
    post_footer_h: f32,
    /// The caption goes under the name when the two do not share a line.
    caption_below: bool,
}

impl Layout {
    fn new(canvas: u64, w: f32, h: f32, screen: &Screen, posts: &[Post]) -> Self {
        let look = screen.look;
        let (safe_top, right, safe_bottom, left) = screen.safe;
        let room = (w - left - right).max(1.0);
        let col_w = room.min(480.0);
        let col_x = left + (room - col_w) / 2.0;
        let photo_w = col_w - 32.0;
        let photo_h = photo_w * ART_H as f32 / ART_W as f32;
        let (title, name, user, sub, body) = (
            look.text(24.0),
            look.text(12.0),
            look.text(14.0),
            look.text(12.0),
            look.text(13.5),
        );
        let caption_below = posts.iter().any(|post| {
            width(canvas, post.user, body, 600) + 6.0 + width(canvas, post.caption, body, 400)
                > photo_w - 8.0
        });
        let caption_lines = if caption_below { 2.0 } else { 1.0 };
        Layout {
            w,
            h,
            safe_top,
            safe_bottom,
            col_x,
            col_w,
            photo_w,
            photo_h,
            title,
            name,
            user,
            sub,
            body,
            // A phone turned sideways keeps its little height for the feed.
            header_h: if h - safe_top - safe_bottom < 500.0 {
                (title * 1.8).max(44.0)
            } else {
                (title * 2.3).max(56.0)
            },
            stories_h: 12.0 + 2.0 * RING + 8.0 + name * 1.5 + 12.0,
            post_header_h: (user * 1.2 + sub * 1.3 + 22.0).max(54.0),
            post_footer_h: 12.0 + HEART + 6.0 + body * (1.0 + 1.5 * caption_lines) + 18.0,
            caption_below,
        }
    }

    /// The band the feed scrolls in, between the header and the tab bar.
    fn view_top(&self) -> f32 {
        self.safe_top + self.header_h
    }

    fn view_bottom(&self) -> f32 {
        self.h - self.safe_bottom - TABBAR_H
    }

    fn post_h(&self) -> f32 {
        self.post_header_h + self.photo_h + self.post_footer_h + POST_GAP
    }

    fn content_height(&self, posts: usize) -> f32 {
        self.stories_h + posts as f32 * self.post_h() + 12.0
    }

    /// Where a post's photo is, at the feed's scroll offset.
    fn photo_rect(&self, index: usize, offset: f32) -> Rect {
        let py = self.view_top() - offset + self.stories_h + index as f32 * self.post_h();
        Rect::new(
            self.col_x + 16.0,
            py + self.post_header_h,
            self.photo_w,
            self.photo_h,
        )
    }

    /// Where a post's like button is: the heart under its photo, with room
    /// around it for a finger.
    fn heart_rect(&self, index: usize, offset: f32) -> Rect {
        let photo = self.photo_rect(index, offset);
        Rect::new(photo.x - 6.0, photo.bottom() + 3.0, 44.0, 44.0)
    }

    /// What of `r` is visible in the scrolling band, if any.
    fn seen(&self, r: Rect) -> Option<Rect> {
        let top = r.y.max(self.view_top());
        let bottom = r.bottom().min(self.view_bottom());
        (bottom > top).then(|| Rect::new(r.x, top, r.w, bottom - top))
    }
}

fn width(canvas: u64, text: &str, size: f32, weight: u16) -> f32 {
    canvas2d::measure_text_styled(canvas, text, size, style(weight, 0.1))
        .map(|m| m.width)
        .unwrap_or(text.len() as f32 * size * 0.55)
}

/// `text` cut to `max` wide, with an ellipsis when it had to be cut.
fn fit(canvas: u64, text: &str, size: f32, weight: u16, max: f32) -> String {
    if width(canvas, text, size, weight) <= max {
        return String::from(text);
    }
    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let cut: String = chars[..mid].iter().collect();
        if width(canvas, &format!("{cut}\u{2026}"), size, weight) <= max {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let cut: String = chars[..lo].iter().collect();
    format!("{cut}\u{2026}")
}

/// The feed's colours: the near-black it was designed in when the phone
/// is dark, white with dark ink when it is light.
#[derive(Clone, Copy)]
struct Theme {
    dark: bool,
    bg: gfx::Color,
    /// Ink at an opacity: white in the dark, near-black in the light.
    ink: (f32, f32, f32),
    shadow: gfx::Color,
}

impl Theme {
    fn of(look: &Look) -> Self {
        if look.dark {
            Theme {
                dark: true,
                bg: color(0.043, 0.051, 0.07, 1.0),
                ink: (1.0, 1.0, 1.0),
                shadow: color(0.0, 0.0, 0.0, 0.45),
            }
        } else {
            Theme {
                dark: false,
                bg: color(1.0, 1.0, 1.0, 1.0),
                ink: (0.05, 0.06, 0.09),
                shadow: color(0.05, 0.06, 0.12, 0.18),
            }
        }
    }

    fn ink(&self, alpha: f32) -> gfx::Color {
        // Secondary text reads lighter on white than on black at the same
        // opacity, so the light theme lifts it.
        let a = if self.dark {
            alpha
        } else {
            (alpha + 0.15).min(1.0)
        };
        color(self.ink.0, self.ink.1, self.ink.2, a)
    }
}

fn color(r: f32, g: f32, b: f32, a: f32) -> gfx::Color {
    gfx::Color { r, g, b, a }
}

fn rect(x: f32, y: f32, width: f32, height: f32) -> gfx::Rect {
    gfx::Rect {
        x,
        y,
        width,
        height,
    }
}

fn radii(all: f32) -> gfx::CornerRadii {
    gfx::CornerRadii {
        top_left: all,
        top_right: all,
        bottom_right: all,
        bottom_left: all,
    }
}

fn style(weight: u16, spacing: f32) -> gfx::TextStyle {
    gfx::TextStyle {
        weight,
        italic: false,
        letter_spacing: spacing,
        family: gfx::FontFamily::Sans,
    }
}

fn out(line: &str) {
    let handle = stdio::stdout();
    let _ = handle.write(line.as_bytes());
    let _ = handle.write(b"\n");
}

// ---------------------------------------------------------------- the art

/// A tiny deterministic palette per post, keyed by seed. Each generator is a
/// few sines -- the point is that the renderer shows photographs' worth of
/// color without any photograph.
fn art_pixel(seed: u32, x: f32, y: f32) -> (f32, f32, f32) {
    match seed % 4 {
        // Dunes at sunset: layered sine ridges over a warm sky.
        0 => {
            let sky_r = 0.98 - y * 0.55;
            let sky_g = 0.55 - y * 0.35;
            let sky_b = 0.45 + y * 0.05;
            let ridge = 0.62 + 0.08 * sinf(x * 9.0) + 0.05 * sinf(x * 23.0 + 1.7);
            if y > ridge {
                let d = (y - ridge) * 3.0;
                (0.35 - d * 0.15, 0.16 - d * 0.06, 0.24 - d * 0.05)
            } else {
                // The sun.
                let dx = x - 0.62;
                let dy = (y - 0.38) * 1.15;
                let sun = 1.0 - (sqrtf(dx * dx + dy * dy) * 9.0).min(1.0);
                (sky_r + sun * 0.9, sky_g + sun * 0.8, sky_b + sun * 0.5)
            }
        }
        // Aurora: cool plasma curtains.
        1 => {
            let v = sinf(x * 6.3 + sinf(y * 4.0) * 2.0) * cosf(y * 5.1 + x * 2.0);
            let w = sinf((x + y) * 8.0 + 2.0 * sinf(x * 3.0));
            (
                0.05 + 0.15 * (v * 0.5 + 0.5),
                0.25 + 0.55 * (v * 0.5 + 0.5),
                0.35 + 0.45 * (w * 0.5 + 0.5),
            )
        }
        // Ocean interference: crossing wavefronts.
        2 => {
            let a = sinf(sqrtf((x - 0.2) * (x - 0.2) + (y - 0.3) * (y - 0.3)) * 40.0);
            let b = sinf(sqrtf((x - 0.8) * (x - 0.8) + (y - 0.7) * (y - 0.7)) * 40.0);
            let m = (a + b) * 0.25 + 0.5;
            (0.04 + m * 0.10, 0.20 + m * 0.35, 0.35 + m * 0.45)
        }
        // Magma marble: warm bands folded by sines.
        _ => {
            let t = sinf(x * 5.0 + 3.0 * sinf(y * 3.0 + x * 2.0));
            let m = t * 0.5 + 0.5;
            (0.55 + m * 0.45, 0.15 + m * 0.35, 0.10 + m * 0.15)
        }
    }
}

/// Paint rows `from..to` of a picture into `px`, which holds the rows
/// before `from`. The pictures are made a slice at a time after the first
/// frame is up: all four at start-up kept a phone's interpreter from
/// showing anything for over a second.
fn render_art_rows(seed: u32, px: &mut Vec<u8>, from: u32, to: u32) {
    for row in from..to.min(ART_H) {
        let y = row as f32 / ART_H as f32;
        for col in 0..ART_W {
            let x = col as f32 / ART_W as f32;
            let (r, g, b) = art_pixel(seed, x, y);
            px.push((r.clamp(0.0, 1.0) * 255.0) as u8);
            px.push((g.clamp(0.0, 1.0) * 255.0) as u8);
            px.push((b.clamp(0.0, 1.0) * 255.0) as u8);
            px.push(255);
        }
    }
}

/// The classic implicit heart curve, rendered into an RGBA buffer with soft
/// edges. `alpha` scales the whole sprite so the like overlay can fade.
fn render_heart(size: u32, r: f32, g: f32, b: f32, alpha: f32) -> Vec<u8> {
    let mut px = Vec::with_capacity((size * size * 4) as usize);
    for row in 0..size {
        for col in 0..size {
            // Map to the curve's sweet spot: x in [-1.4, 1.4], y in [1.4, -1.4].
            let x = (col as f32 / size as f32 - 0.5) * 2.8;
            let y = (0.5 - row as f32 / size as f32) * 2.8 + 0.1;
            let f = {
                let a = x * x + y * y - 1.0;
                a * a * a - x * x * y * y * y
            };
            // Near-binary edge with a whisker of AA: the implicit value is a
            // cubic, so a wide window blurs the cleft between the lobes shut.
            let cover = (0.5 - f * 40.0).clamp(0.0, 1.0);
            px.push((r * 255.0) as u8);
            px.push((g * 255.0) as u8);
            px.push((b * 255.0) as u8);
            px.push((cover * alpha * 255.0) as u8);
        }
    }
    px
}

// --------------------------------------------------------------- the feed

struct Post {
    user: &'static str,
    caption: &'static str,
    likes: u32,
    liked: bool,
    /// The picture's pixels, filled in a slice at a time; whole when it
    /// holds every row.
    art: Vec<u8>,
    seed: u32,
    /// The finished picture, kept by the host so a frame draws it by its
    /// handle instead of sending 430 KB of pixels across again.
    kept: Option<u64>,
    avatar_hue: (f32, f32, f32),
}

impl Post {
    fn art_full(&self) -> bool {
        self.art.len() >= (ART_W * ART_H * 4) as usize
    }

    fn art_done(&self) -> bool {
        self.kept.is_some() || self.art_full()
    }
}

const STORIES: [(&str, (f32, f32, f32)); 5] = [
    ("you", (0.45, 0.45, 0.55)),
    ("dune.wanderer", (0.95, 0.55, 0.30)),
    ("aurora.lab", (0.30, 0.75, 0.65)),
    ("tidepool", (0.25, 0.55, 0.90)),
    ("magma.room", (0.90, 0.35, 0.25)),
];

fn posts() -> Vec<Post> {
    let specs: [(&'static str, &'static str, u32, (f32, f32, f32)); 4] = [
        (
            "dune.wanderer",
            "golden hour again",
            2841,
            (0.95, 0.55, 0.30),
        ),
        (
            "aurora.lab",
            "the sky owed us one",
            1203,
            (0.30, 0.75, 0.65),
        ),
        ("tidepool", "two stones, one pond", 977, (0.25, 0.55, 0.90)),
        (
            "magma.room",
            "slow fold, warm light",
            3410,
            (0.90, 0.35, 0.25),
        ),
    ];
    specs
        .into_iter()
        .enumerate()
        .map(|(i, (user, caption, likes, avatar_hue))| Post {
            user,
            caption,
            likes,
            liked: false,
            art: Vec::with_capacity((ART_W * ART_H * 4) as usize),
            seed: i as u32,
            kept: None,
            avatar_hue,
        })
        .collect()
}

/// One story circle: a ring of arc segments walking a warm gradient, an
/// avatar disc inside, a name below.

/// One story circle: a ring of arc segments walking a warm gradient, an
/// avatar disc inside, a name below (`label`, cut to fit its slot).
fn draw_story(
    canvas: u64,
    cx: f32,
    cy: f32,
    name: &str,
    label: &str,
    hue: (f32, f32, f32),
    l: &Layout,
    theme: &Theme,
) -> Result<(), gfx::GfxError> {
    const SEGMENTS: u32 = 20;
    let ring = [(0.95, 0.35, 0.25), (0.95, 0.65, 0.25), (0.85, 0.25, 0.55)];
    for i in 0..SEGMENTS {
        let t = i as f32 / SEGMENTS as f32;
        // Walk the three ring colors around the circle and back.
        let phase = t * 2.0;
        let (from, to, local) = if phase < 1.0 {
            (ring[0], ring[1], phase)
        } else {
            (ring[1], ring[2], phase - 1.0)
        };
        let mix = |a: f32, b: f32| a + (b - a) * local;
        canvas2d::stroke_arc(
            canvas,
            gfx::Point { x: cx, y: cy },
            RING,
            t * 360.0 - 90.0,
            360.0 / SEGMENTS as f32 + 1.0,
            3.0,
            color(mix(from.0, to.0), mix(from.1, to.1), mix(from.2, to.2), 1.0),
        )?;
    }
    canvas2d::radial_gradient(
        canvas,
        gfx::Point { x: cx, y: cy },
        24.0,
        color(hue.0, hue.1, hue.2, 1.0),
        color(hue.0 * 0.4, hue.1 * 0.4, hue.2 * 0.5, 1.0),
    )?;
    let initial = &name[..1];
    canvas2d::draw_text_styled(
        canvas,
        initial,
        gfx::Point {
            x: cx - 5.0,
            y: cy + 6.0,
        },
        17.0,
        color(1.0, 1.0, 1.0, 0.95),
        style(700, 0.0),
    )?;
    let metrics = canvas2d::measure_text(canvas, label, l.name)?;
    canvas2d::draw_text(
        canvas,
        label,
        gfx::Point {
            x: cx - metrics.width / 2.0,
            y: cy + RING + 8.0 + l.name,
        },
        l.name,
        theme.ink(0.6),
    )?;
    Ok(())
}

struct HeartBurst {
    post: usize,
    scale: Spring,
    alpha: f32,
}

struct Feed {
    posts: Vec<Post>,
    offset: f32,
    velocity: f32,
    burst: Option<HeartBurst>,
    /// The empty heart in the theme's ink, and which theme it was drawn for.
    heart_small: (bool, Vec<u8>),
    heart_small_red: Vec<u8>,
    /// The layout and the story names cut to fit it, kept until the canvas,
    /// the covered edges or the person's look change.
    layout: Option<(u64, Layout, Vec<String>, Vec<f32>)>,
}

impl Feed {
    fn new() -> Self {
        Feed {
            posts: posts(),
            offset: 0.0,
            velocity: 0.0,
            burst: None,
            heart_small: (true, render_heart(26, 1.0, 1.0, 1.0, 0.9)),
            heart_small_red: render_heart(26, 0.98, 0.22, 0.35, 1.0),
            layout: None,
        }
    }

    /// The layout for this canvas and screen, worked out again only when
    /// one of them changed: measuring text every frame would cost a
    /// phone's interpreter a frame's worth of time.
    fn layout(&mut self, canvas: u64, w: f32, h: f32, screen: &Screen) -> Layout {
        let (t, r, b, lft) = screen.safe;
        let key = [w, h, t, r, b, lft, screen.look.text_scale]
            .iter()
            .fold(0xcbf2_9ce4_8422_2325_u64, |k, v| {
                (k ^ u64::from(v.to_bits())).wrapping_mul(0x0100_0000_01b3)
            });
        if let Some((seen, layout, _, _)) = &self.layout {
            if *seen == key {
                return *layout;
            }
        }
        let layout = Layout::new(canvas, w, h, screen, &self.posts);
        let step = (layout.col_w - 88.0) / (STORIES.len() - 1) as f32;
        let labels = STORIES
            .iter()
            .map(|(name, _)| fit(canvas, name, layout.name, 400, step - 6.0))
            .collect();
        let name_w = self
            .posts
            .iter()
            .map(|post| width(canvas, post.user, layout.body, 600))
            .collect();
        self.layout = Some((key, layout, labels, name_w));
        layout
    }

    fn max_offset(&self, l: &Layout) -> f32 {
        (l.content_height(self.posts.len()) - (l.view_bottom() - l.view_top())).max(0.0)
    }

    /// Momentum + rubber-band. Direct deltas apply with resistance past the
    /// edges; the glide decays; overshoot springs home.
    fn scroll_by(&mut self, dy: f32, l: &Layout) {
        let max = self.max_offset(l);
        let resisted = if self.offset < 0.0 || self.offset > max {
            dy * 0.35
        } else {
            dy
        };
        self.offset += resisted;
        self.velocity = self.velocity * 0.35 + dy * 24.0;
    }

    fn glide(&mut self, dt: f32, l: &Layout) {
        let max = self.max_offset(l);
        if self.offset < 0.0 {
            self.offset += (0.0 - self.offset) * (dt * 12.0).min(1.0);
            self.velocity = 0.0;
        } else if self.offset > max {
            self.offset += (max - self.offset) * (dt * 12.0).min(1.0);
            self.velocity = 0.0;
        } else {
            self.offset += self.velocity * dt;
            self.velocity *= libm::powf(0.02, dt);
            if self.velocity.abs() < 1.0 {
                self.velocity = 0.0;
            }
        }
    }

    /// The post whose photo contains this screen point, if any.
    fn photo_at(&self, x: f32, y: f32, l: &Layout) -> Option<usize> {
        if y < l.view_top() || y > l.view_bottom() {
            return None;
        }
        (0..self.posts.len()).find(|i| l.photo_rect(*i, self.offset).contains(x, y))
    }

    /// The post whose like button is under this screen point, if any.
    fn heart_at(&self, x: f32, y: f32, l: &Layout) -> Option<usize> {
        if y < l.view_top() || y > l.view_bottom() {
            return None;
        }
        (0..self.posts.len()).find(|i| l.heart_rect(*i, self.offset).contains(x, y))
    }

    fn like(&mut self, index: usize) {
        let post = &mut self.posts[index];
        if !post.liked {
            post.liked = true;
            post.likes += 1;
        }
        self.burst = Some(HeartBurst {
            post: index,
            scale: Spring::rest_at(0.2, 22.0),
            alpha: 1.4,
        });
    }

    /// A tap on the heart: like, or take the like back.
    fn toggle_like(&mut self, index: usize) {
        if self.posts[index].liked {
            let post = &mut self.posts[index];
            post.liked = false;
            post.likes -= 1;
        } else {
            self.like(index);
        }
    }

    fn tick(&mut self, dt: f32, l: &Layout) {
        self.glide(dt, l);
        if let Some(burst) = &mut self.burst {
            burst.scale.tick(1.0, dt);
            burst.alpha -= dt * 1.6;
            if burst.alpha <= 0.0 {
                self.burst = None;
            }
        }
    }

    fn settled(&self, l: &Layout) -> bool {
        let max = self.max_offset(l);
        self.velocity == 0.0
            && self.offset >= -0.01
            && self.offset <= max + 0.01
            && self.burst.is_none()
    }

    /// Make up to `rows` more rows of the pictures, nearest the top of
    /// the feed first. True while any picture is unfinished.
    fn make_art(&mut self, canvas: u64, rows: u32) -> bool {
        let Some(post) = self.posts.iter_mut().find(|post| !post.art_done()) else {
            return false;
        };
        let done = (post.art.len() / (ART_W as usize * 4)) as u32;
        render_art_rows(post.seed, &mut post.art, done, done + rows);
        if post.art_full() {
            // Kept, the pixels cross to the host once; if the host will not
            // keep it, the frame sends them as before.
            if let Ok(handle) = canvas_images::keep(canvas, ART_W, ART_H, &post.art) {
                post.kept = Some(handle);
                post.art = Vec::new();
            }
        }
        true
    }

    /// Draw the empty hearts in the ink of the person's look.
    fn follow_look(&mut self, look: &Look) {
        let theme = Theme::of(look);
        if self.heart_small.0 != theme.dark {
            let (r, g, b) = theme.ink;
            self.heart_small = (theme.dark, render_heart(26, r, g, b, 0.9));
        }
    }

    /// What the screen shows, for a screen reader: the title, each story,
    /// and for each post on screen its name, photo, like button, likes and
    /// caption, at the rectangles they are drawn in.
    fn describe(&self, l: &Layout) -> Said {
        let mut said = Said::new();
        said.heading(
            Rect::new(l.col_x + 20.0, l.safe_top, l.col_w - 40.0, l.header_h),
            "Krategram",
        );
        let top = l.view_top() - self.offset;
        let step = (l.col_w - 88.0) / (STORIES.len() - 1) as f32;
        for (i, (name, _)) in STORIES.iter().enumerate() {
            let cx = l.col_x + 44.0 + i as f32 * step;
            if let Some(at) = l.seen(Rect::new(cx - step / 2.0, top, step, l.stories_h)) {
                if i == 0 {
                    said.text(at, "Your story");
                } else {
                    said.text(at, &format!("{name}'s story"));
                }
            }
        }
        let left = l.col_x + 16.0;
        for (i, post) in self.posts.iter().enumerate() {
            let py = top + l.stories_h + i as f32 * l.post_h();
            if let Some(at) = l.seen(Rect::new(left, py, l.photo_w, l.post_header_h)) {
                said.heading(at, &format!("{}, original art", post.user));
            }
            let photo = l.photo_rect(i, self.offset);
            if let Some(at) = l.seen(photo) {
                said.image(at, &format!("Photo by {}: {}", post.user, post.caption));
            }
            if let Some(at) = l.seen(l.heart_rect(i, self.offset)) {
                said.button(at, if post.liked { "Unlike" } else { "Like" });
            }
            let line = l.body * 1.5;
            let likes_top = photo.bottom() + 12.0 + HEART + 6.0;
            if let Some(at) = l.seen(Rect::new(left, likes_top, l.photo_w, line)) {
                said.text(at, &format!("{} likes", post.likes));
            }
            let lines = if l.caption_below { 2.0 } else { 1.0 };
            if let Some(at) = l.seen(Rect::new(left, likes_top + line, l.photo_w, line * lines)) {
                said.text(at, &format!("{}: {}", post.user, post.caption));
            }
        }
        said
    }

    /// A short key for what `describe` would say: the layout, the scroll
    /// position and the likes. The same key means the same description.
    fn said_key(&self) -> u64 {
        let layout = self.layout.as_ref().map_or(0, |(key, ..)| *key);
        let mut key = layout ^ u64::from(self.offset.to_bits()).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        for post in &self.posts {
            key = (key ^ u64::from(post.likes)).wrapping_mul(0x0100_0000_01b3);
        }
        key
    }

    fn draw(&self, canvas: u64, l: &Layout, look: &Look) -> Result<(), gfx::GfxError> {
        let theme = Theme::of(look);
        let (labels, name_w): (&[String], &[f32]) = match &self.layout {
            Some((_, _, labels, name_w)) => (labels, name_w),
            None => (&[], &[]),
        };
        canvas2d::clear(canvas, theme.bg)?;

        // Content scrolls; header and tab bar paint over it afterwards.
        let (view_top, view_bottom) = (l.view_top(), l.view_bottom());
        canvas2d::set_clip(canvas, 0.0, view_top, l.w, view_bottom - view_top)?;
        let top = view_top - self.offset;

        // Stories row, spread across the column.
        if top + l.stories_h > view_top && top < view_bottom {
            let step = (l.col_w - 88.0) / (STORIES.len() - 1) as f32;
            let cy = top + 12.0 + RING;
            for (i, (name, hue)) in STORIES.iter().enumerate() {
                let cx = l.col_x + 44.0 + i as f32 * step;
                let label = labels.get(i).map(String::as_str).unwrap_or(name);
                draw_story(canvas, cx, cy, name, label, *hue, l, &theme)?;
            }
        }

        // Posts, culled to the viewport.
        let left = l.col_x + 16.0;
        for (i, post) in self.posts.iter().enumerate() {
            let py = top + l.stories_h + i as f32 * l.post_h();
            if py > view_bottom || py + l.post_h() < view_top {
                continue;
            }

            // Header row: avatar + names, centred in the row.
            let mid = py + l.post_header_h / 2.0 - 1.0;
            canvas2d::radial_gradient(
                canvas,
                gfx::Point {
                    x: left + 18.0,
                    y: mid,
                },
                17.0,
                color(post.avatar_hue.0, post.avatar_hue.1, post.avatar_hue.2, 1.0),
                color(
                    post.avatar_hue.0 * 0.4,
                    post.avatar_hue.1 * 0.4,
                    post.avatar_hue.2 * 0.5,
                    1.0,
                ),
            )?;
            let block = l.user * 1.2 + l.sub * 1.3;
            let user_y = mid - block / 2.0 + l.user * 0.95;
            let sub_y = user_y + l.sub * 1.35;
            canvas2d::draw_text_styled(
                canvas,
                post.user,
                gfx::Point {
                    x: left + 44.0,
                    y: user_y,
                },
                l.user,
                theme.ink(0.95),
                style(650, 0.1),
            )?;
            canvas2d::draw_text(
                canvas,
                "original art",
                gfx::Point {
                    x: left + 44.0,
                    y: sub_y,
                },
                l.sub,
                theme.ink(0.45),
            )?;

            // The photo: rounded, shadowed, and the reason this app exists.
            let photo = l.photo_rect(i, self.offset);
            let art_rect = rect(photo.x, photo.y, photo.w, photo.h);
            canvas2d::drop_shadow_round_rect(
                canvas,
                rect(
                    art_rect.x,
                    art_rect.y + 8.0,
                    art_rect.width,
                    art_rect.height,
                ),
                radii(18.0),
                18.0,
                theme.shadow,
            )?;
            if let Some(handle) = post.kept {
                canvas_images::draw_round(canvas, handle, art_rect, radii(18.0))?;
            } else if post.art_full() {
                canvas2d::draw_pixels_round(
                    canvas,
                    art_rect,
                    radii(18.0),
                    ART_W,
                    ART_H,
                    &post.art,
                )?;
            } else {
                // Still being made: a soft wash of the post's colour.
                let (r, g, b) = post.avatar_hue;
                canvas2d::fill_round_rect(
                    canvas,
                    art_rect,
                    radii(18.0),
                    color(r * 0.5 + 0.1, g * 0.5 + 0.1, b * 0.5 + 0.12, 1.0),
                )?;
            }

            // Action row: the heart, then the counts and caption.
            let fy = photo.bottom() + 12.0;
            let heart = if post.liked {
                &self.heart_small_red
            } else {
                &self.heart_small.1
            };
            canvas2d::draw_pixels(canvas, rect(left + 4.0, fy, HEART, HEART), 26, 26, heart)?;
            let likes = format!("{} likes", post.likes);
            let likes_y = fy + HEART + 6.0 + l.body;
            canvas2d::draw_text_styled(
                canvas,
                &likes,
                gfx::Point {
                    x: left + 4.0,
                    y: likes_y,
                },
                l.body,
                theme.ink(0.95),
                style(650, 0.1),
            )?;
            let line = l.body * 1.5;
            let name_y = likes_y + line;
            canvas2d::draw_text_styled(
                canvas,
                post.user,
                gfx::Point {
                    x: left + 4.0,
                    y: name_y,
                },
                l.body,
                theme.ink(0.9),
                style(600, 0.1),
            )?;
            let (cap_x, cap_y) = if l.caption_below {
                (left + 4.0, name_y + line)
            } else {
                (left + 10.0 + name_w.get(i).copied().unwrap_or(0.0), name_y)
            };
            canvas2d::draw_text(
                canvas,
                post.caption,
                gfx::Point { x: cap_x, y: cap_y },
                l.body,
                theme.ink(0.6),
            )?;

            // The double-tap burst, centered on this photo.
            if let Some(burst) = &self.burst {
                if burst.post == i {
                    let size = 120.0 * burst.scale.value;
                    let alpha = burst.alpha.clamp(0.0, 1.0);
                    let sprite = render_heart(96, 1.0, 1.0, 1.0, alpha);
                    canvas2d::draw_pixels(
                        canvas,
                        rect(
                            art_rect.x + art_rect.width / 2.0 - size / 2.0,
                            art_rect.y + art_rect.height / 2.0 - size / 2.0,
                            size,
                            size,
                        ),
                        96,
                        96,
                        &sprite,
                    )?;
                }
            }
        }
        canvas2d::clear_clip(canvas)?;

        // Header, painted over the scrolled content, under the notch too.
        canvas2d::fill_rect(canvas, rect(0.0, 0.0, l.w, view_top), theme.bg)?;
        let title_y = l.safe_top + l.header_h / 2.0 + l.title * 0.35;
        canvas2d::draw_text_styled(
            canvas,
            "Krategram",
            gfx::Point {
                x: l.col_x + 20.0,
                y: title_y,
            },
            l.title,
            theme.ink(1.0),
            style(750, -0.6),
        )?;
        canvas2d::fill_rect(canvas, rect(0.0, view_top - 1.0, l.w, 1.0), theme.ink(0.08))?;

        // Tab bar: five glyphs from primitives, spread across the column,
        // above the home bar.
        let bar_y = view_bottom;
        canvas2d::fill_rect(canvas, rect(0.0, bar_y, l.w, l.h - bar_y), theme.bg)?;
        canvas2d::fill_rect(canvas, rect(0.0, bar_y, l.w, 1.0), theme.ink(0.08))?;
        let cy = bar_y + TABBAR_H / 2.0;
        let slot = l.col_w / 5.0;
        let sx = |n: f32| l.col_x + slot * n;
        // Home: filled rounded square. Search: ring. Create: plus in a round
        // square. Likes: the heart. Profile: a disc.
        canvas2d::fill_round_rect(
            canvas,
            rect(sx(0.5) - 9.0, cy - 9.0, 18.0, 18.0),
            radii(5.0),
            theme.ink(0.95),
        )?;
        canvas2d::stroke_circle(
            canvas,
            gfx::Point { x: sx(1.5), y: cy },
            8.0,
            2.0,
            theme.ink(0.6),
        )?;
        canvas2d::stroke_round_rect(
            canvas,
            rect(sx(2.5) - 9.0, cy - 9.0, 18.0, 18.0),
            radii(6.0),
            2.0,
            theme.ink(0.6),
        )?;
        canvas2d::fill_rect(
            canvas,
            rect(sx(2.5) - 5.0, cy - 1.0, 10.0, 2.0),
            theme.ink(0.6),
        )?;
        canvas2d::fill_rect(
            canvas,
            rect(sx(2.5) - 1.0, cy - 5.0, 2.0, 10.0),
            theme.ink(0.6),
        )?;
        canvas2d::draw_pixels(
            canvas,
            rect(sx(3.5) - 11.0, cy - 11.0, 22.0, 22.0),
            26,
            26,
            &self.heart_small.1,
        )?;
        canvas2d::radial_gradient(
            canvas,
            gfx::Point { x: sx(4.5), y: cy },
            9.0,
            color(0.45, 0.45, 0.55, 1.0),
            color(0.25, 0.25, 0.35, 1.0),
        )?;

        canvas2d::present(canvas)?;
        Ok(())
    }
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
            grow: 1.0,
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

struct Component;

impl krate::Guest for Component {
    fn run() -> i32 {
        let raw = args::raw();
        let quick = raw.split_whitespace().any(|word| word == "quick");

        let win = match window::create(
            "Krategram",
            types::WindowSize {
                width: REQUEST_W as u32,
                height: REQUEST_H as u32,
            },
        ) {
            Ok(win) => win,
            Err(_) => {
                out("window:no");
                return 1;
            }
        };
        if tree::set_root(win, &node(ROOT_ID, None, types::WidgetKind::Stack)).is_err() {
            out("tree:no");
            return 1;
        }
        let _ = tree::upsert_node(
            win,
            &node(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas),
        );
        let canvas = match canvas2d::bind(win, CANVAS_ID) {
            Ok(canvas) => canvas,
            Err(_) => {
                out("bind:no");
                return 1;
            }
        };

        let mut feed = Feed::new();
        out(&format!("posts:{}", feed.posts.len()));
        // The first picture is on the first screen, so it is made before
        // the first frame; the rest are made a slice per frame after it
        // (all of them first on the quick run, whose picture is checked).
        feed.make_art(canvas, ART_H);
        while quick && feed.make_art(canvas, ART_H) {}

        let mut last = clock::monotonic_nanos();
        let mut last_press_nanos: u64 = 0;
        let mut last_press_at = (0.0f32, 0.0f32);
        let mut frames: u32 = 0;
        let mut layout;
        let mut said_for = 0u64;

        loop {
            // The surface is the law, asked fresh every frame: a desktop
            // resize, a phone-sized surface or larger text is just the
            // next layout.
            let screen = Screen::of(win);
            let (w, h) = match canvas2d::canvas_size(canvas) {
                Ok(size) => (size.width.max(1.0), size.height.max(1.0)),
                Err(_) => (REQUEST_W, REQUEST_H),
            };
            layout = feed.layout(canvas, w, h, &screen);

            let now = clock::monotonic_nanos();
            let dt = if quick {
                1.0 / 60.0
            } else {
                let dt = ((now.saturating_sub(last)) as f32 / 1_000_000_000.0).min(0.05);
                last = now;
                dt
            };

            if quick {
                // Synthetic session: scroll deep, spring back, double-tap.
                match frames {
                    5..=20 => feed.scroll_by(60.0, &layout),
                    30 => feed.like(1),
                    _ => {}
                }
            }
            feed.tick(dt, &layout);

            feed.follow_look(&screen.look);
            if feed.draw(canvas, &layout, &screen.look).is_err() {
                out("draw:no");
                return 1;
            }
            // Tell the screen reader once the feed comes to rest and what it
            // shows has changed; a scroll in flight would only make it
            // repeat itself.
            if feed.settled(&layout) && feed.said_key() != said_for {
                said_for = feed.said_key();
                feed.describe(&layout).send(win);
            }
            // After the frame is shown: the next slice of an unfinished
            // picture, small enough not to hold up the next frame for long.
            let making = feed.make_art(canvas, 24);
            frames += 1;

            if quick {
                if frames >= 90 {
                    break;
                }
                continue;
            }

            let _ = window::request_redraw(win);
            // Take everything that is already queued before the next frame.
            // One event per frame is what made a fast finger feel laggy: a
            // 60 Hz drag produces a wheel delta every frame, so a loop that
            // consumes one and then spends a whole frame drawing can never
            // catch up -- the backlog grows for as long as the finger moves.
            // Polling first costs nothing when the queue is empty. Every
            // queued event is handled, a tap as much as a scroll.
            let mut queued = Vec::new();
            while let Some(event) = events::poll() {
                queued.push(event);
            }
            if queued.is_empty() {
                // Block for input when everything is at rest; poll while
                // moving or while a picture is still being made.
                let wait = if feed.settled(&layout) && !making {
                    None
                } else {
                    Some(16)
                };
                queued.extend(events::wait(wait));
            }
            let mut closing = false;
            for event in queued {
                match event {
                    types::Event::CloseRequested(_) => closing = true,
                    types::Event::Wheel(wheel) => feed.scroll_by(wheel.dy, &layout),
                    types::Event::Pointer(p) if p.pressed && p.button.is_some() => {
                        if let Some(index) = feed.heart_at(p.x, p.y, &layout) {
                            // The heart is a button: one tap likes.
                            feed.toggle_like(index);
                            last_press_nanos = 0;
                            continue;
                        }
                        let near = (p.x - last_press_at.0).abs() < 30.0
                            && (p.y - last_press_at.1).abs() < 30.0;
                        let quick_pair = now.saturating_sub(last_press_nanos) < 400_000_000;
                        if near && quick_pair {
                            if let Some(index) = feed.photo_at(p.x, p.y, &layout) {
                                feed.like(index);
                            }
                            last_press_nanos = 0;
                        } else {
                            last_press_nanos = now;
                            last_press_at = (p.x, p.y);
                        }
                    }
                    _ => {}
                }
            }
            if closing {
                break;
            }
        }

        if quick {
            out(&format!(
                "scrolled:{} liked:{} settled:{}",
                feed.offset as i32,
                feed.posts.iter().filter(|p| p.liked).count(),
                feed.settled(&layout)
            ));
        }
        out("gram:ready");
        0
    }
}

krate::export!(Component);
