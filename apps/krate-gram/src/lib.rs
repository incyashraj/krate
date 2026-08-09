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
//! Note the manifest next to this file: the whole thing needs one
//! privileged capability -- a window.
//!
//! `quick` draws real frames with synthetic scrolling and prints markers,
//! so a headless check exercises the same paint path a person sees.

#![no_std]

#[allow(warnings)]
mod bindings;

extern crate alloc;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::time::clock;
use bindings::krate::ui::{events, tree, types, window};
use krate::motion::Spring;
use libm::{cosf, sinf, sqrtf};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

/// The size the window is asked for. What the host actually grants is the
/// law -- a phone hands back its whole screen -- so every frame lays out
/// from canvas-size, never from these numbers (K-087).
const REQUEST_W: f32 = 390.0;
const REQUEST_H: f32 = 720.0;

const HEADER_H: f32 = 56.0;
const TABBAR_H: f32 = 56.0;

/// The generated art's own resolution. It is drawn into whatever rect the
/// layout gives it; the sampler scales.
const ART_W: u32 = 358;
const ART_H: u32 = 300;

/// Everything the frame needs to know about the surface it is drawing on,
/// asked fresh every frame so a resize -- or a phone-sized surface -- is
/// simply the next frame's truth.
#[derive(Clone, Copy)]
struct Layout {
    w: f32,
    h: f32,
    /// The feed column: capped for readability, centered when the surface
    /// is wider than a phone.
    col_x: f32,
    col_w: f32,
    /// The photo rect inside the column, aspect matched to the art.
    photo_w: f32,
    photo_h: f32,
}

impl Layout {
    fn from_canvas(w: f32, h: f32) -> Self {
        let col_w = w.min(480.0);
        let col_x = (w - col_w) / 2.0;
        let photo_w = col_w - 32.0;
        let photo_h = photo_w * ART_H as f32 / ART_W as f32;
        Layout {
            w,
            h,
            col_x,
            col_w,
            photo_w,
            photo_h,
        }
    }

    fn post_h(&self) -> f32 {
        POST_HEADER_H + self.photo_h + POST_FOOTER_H + POST_GAP
    }

    fn content_height(&self, posts: usize) -> f32 {
        STORIES_H + posts as f32 * self.post_h() + 12.0
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

fn render_art(seed: u32) -> Vec<u8> {
    let mut px = Vec::with_capacity((ART_W * ART_H * 4) as usize);
    for row in 0..ART_H {
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
    px
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
    art: Vec<u8>,
    avatar_hue: (f32, f32, f32),
}

const POST_HEADER_H: f32 = 54.0;
const POST_FOOTER_H: f32 = 96.0;
const POST_GAP: f32 = 18.0;
const STORIES_H: f32 = 108.0;

fn posts() -> Vec<Post> {
    let specs: [(&'static str, &'static str, u32, (f32, f32, f32)); 4] = [
        ("dune.wanderer", "golden hour again", 2841, (0.95, 0.55, 0.30)),
        ("aurora.lab", "the sky owed us one", 1203, (0.30, 0.75, 0.65)),
        ("tidepool", "two stones, one pond", 977, (0.25, 0.55, 0.90)),
        ("magma.room", "slow fold, warm light", 3410, (0.90, 0.35, 0.25)),
    ];
    specs
        .into_iter()
        .enumerate()
        .map(|(i, (user, caption, likes, avatar_hue))| Post {
            user,
            caption,
            likes,
            liked: false,
            art: render_art(i as u32),
            avatar_hue,
        })
        .collect()
}

/// One story circle: a ring of arc segments walking a warm gradient, an
/// avatar disc inside, a name below.
fn draw_story(
    canvas: u64,
    cx: f32,
    cy: f32,
    name: &str,
    hue: (f32, f32, f32),
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
            29.0,
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
    let label = &name[..1];
    canvas2d::draw_text_styled(
        canvas,
        label,
        gfx::Point {
            x: cx - 5.0,
            y: cy + 6.0,
        },
        17.0,
        color(1.0, 1.0, 1.0, 0.95),
        style(700, 0.0),
    )?;
    let metrics = canvas2d::measure_text(canvas, name, 11.0)?;
    canvas2d::draw_text(
        canvas,
        name,
        gfx::Point {
            x: cx - metrics.width / 2.0,
            y: cy + 48.0,
        },
        11.0,
        color(1.0, 1.0, 1.0, 0.6),
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
    heart_small: Vec<u8>,
    heart_small_red: Vec<u8>,
}

impl Feed {
    fn new() -> Self {
        Feed {
            posts: posts(),
            offset: 0.0,
            velocity: 0.0,
            burst: None,
            heart_small: render_heart(26, 1.0, 1.0, 1.0, 0.9),
            heart_small_red: render_heart(26, 0.98, 0.22, 0.35, 1.0),
        }
    }

    fn max_offset(&self, l: &Layout) -> f32 {
        (l.content_height(self.posts.len()) - (l.h - HEADER_H - TABBAR_H)).max(0.0)
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
        let content_y = y - HEADER_H + self.offset;
        if y < HEADER_H || y > l.h - TABBAR_H {
            return None;
        }
        let feed_y = content_y - STORIES_H;
        if feed_y < 0.0 {
            return None;
        }
        let index = (feed_y / l.post_h()) as usize;
        let within = feed_y - index as f32 * l.post_h();
        let on_photo = within >= POST_HEADER_H && within < POST_HEADER_H + l.photo_h;
        let px = l.col_x + 16.0;
        (index < self.posts.len() && on_photo && x >= px && x <= px + l.photo_w)
            .then_some(index)
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

    fn draw(&self, canvas: u64, l: &Layout) -> Result<(), gfx::GfxError> {
        canvas2d::clear(canvas, color(0.043, 0.051, 0.07, 1.0))?;

        // Content scrolls; header and tab bar paint over it afterwards.
        canvas2d::set_clip(canvas, 0.0, HEADER_H, l.w, l.h - HEADER_H - TABBAR_H)?;
        let top = HEADER_H - self.offset;

        // Stories row, spread across the column.
        let stories = [
            ("you", (0.45, 0.45, 0.55)),
            ("dune.wanderer", (0.95, 0.55, 0.30)),
            ("aurora.lab", (0.30, 0.75, 0.65)),
            ("tidepool", (0.25, 0.55, 0.90)),
            ("magma.room", (0.90, 0.35, 0.25)),
        ];
        if top + STORIES_H > HEADER_H && top < l.h {
            let step = (l.col_w - 88.0) / (stories.len() - 1) as f32;
            for (i, (name, hue)) in stories.iter().enumerate() {
                draw_story(
                    canvas,
                    l.col_x + 44.0 + i as f32 * step,
                    top + 40.0,
                    name,
                    *hue,
                )?;
            }
        }

        // Posts, culled to the viewport.
        let left = l.col_x + 16.0;
        for (i, post) in self.posts.iter().enumerate() {
            let py = top + STORIES_H + i as f32 * l.post_h();
            if py > l.h || py + l.post_h() < HEADER_H {
                continue;
            }

            // Header row: avatar + names.
            canvas2d::radial_gradient(
                canvas,
                gfx::Point {
                    x: left + 18.0,
                    y: py + 26.0,
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
            canvas2d::draw_text_styled(
                canvas,
                post.user,
                gfx::Point {
                    x: left + 44.0,
                    y: py + 24.0,
                },
                14.0,
                color(1.0, 1.0, 1.0, 0.95),
                style(650, 0.1),
            )?;
            canvas2d::draw_text(
                canvas,
                "original art",
                gfx::Point {
                    x: left + 44.0,
                    y: py + 40.0,
                },
                11.5,
                color(1.0, 1.0, 1.0, 0.45),
            )?;

            // The photo: rounded, shadowed, and the reason this app exists.
            let art_rect = rect(left, py + POST_HEADER_H, l.photo_w, l.photo_h);
            canvas2d::drop_shadow_round_rect(
                canvas,
                rect(art_rect.x, art_rect.y + 8.0, art_rect.width, art_rect.height),
                radii(18.0),
                18.0,
                color(0.0, 0.0, 0.0, 0.45),
            )?;
            canvas2d::draw_pixels_round(
                canvas,
                art_rect,
                radii(18.0),
                ART_W,
                ART_H,
                &post.art,
            )?;

            // Action row: the heart, then the counts and caption.
            let fy = py + POST_HEADER_H + l.photo_h + 12.0;
            let heart = if post.liked {
                &self.heart_small_red
            } else {
                &self.heart_small
            };
            canvas2d::draw_pixels(canvas, rect(left + 4.0, fy, 26.0, 26.0), 26, 26, heart)?;
            canvas2d::draw_text_styled(
                canvas,
                &format!("{} likes", post.likes),
                gfx::Point {
                    x: left + 4.0,
                    y: fy + 46.0,
                },
                13.5,
                color(1.0, 1.0, 1.0, 0.95),
                style(650, 0.1),
            )?;
            canvas2d::draw_text_styled(
                canvas,
                post.user,
                gfx::Point {
                    x: left + 4.0,
                    y: fy + 66.0,
                },
                13.0,
                color(1.0, 1.0, 1.0, 0.9),
                style(600, 0.1),
            )?;
            let name_w = canvas2d::measure_text_styled(canvas, post.user, 13.0, style(600, 0.1))?;
            canvas2d::draw_text(
                canvas,
                post.caption,
                gfx::Point {
                    x: left + 10.0 + name_w.width,
                    y: fy + 66.0,
                },
                13.0,
                color(1.0, 1.0, 1.0, 0.6),
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

        // Header, painted over the scrolled content.
        canvas2d::fill_rect(
            canvas,
            rect(0.0, 0.0, l.w, HEADER_H),
            color(0.043, 0.051, 0.07, 1.0),
        )?;
        canvas2d::draw_text_styled(
            canvas,
            "Krategram",
            gfx::Point {
                x: l.col_x + 20.0,
                y: 38.0,
            },
            24.0,
            color(1.0, 1.0, 1.0, 1.0),
            style(750, -0.6),
        )?;
        canvas2d::fill_rect(
            canvas,
            rect(0.0, HEADER_H - 1.0, l.w, 1.0),
            color(1.0, 1.0, 1.0, 0.08),
        )?;

        // Tab bar: five glyphs from primitives, spread across the column.
        let bar_y = l.h - TABBAR_H;
        canvas2d::fill_rect(
            canvas,
            rect(0.0, bar_y, l.w, TABBAR_H),
            color(0.043, 0.051, 0.07, 1.0),
        )?;
        canvas2d::fill_rect(
            canvas,
            rect(0.0, bar_y, l.w, 1.0),
            color(1.0, 1.0, 1.0, 0.08),
        )?;
        let cy = bar_y + TABBAR_H / 2.0;
        let slot = l.col_w / 5.0;
        let sx = |n: f32| l.col_x + slot * n;
        // Home: filled rounded square. Search: ring. Create: plus in a round
        // square. Likes: the heart. Profile: a disc.
        canvas2d::fill_round_rect(
            canvas,
            rect(sx(0.5) - 9.0, cy - 9.0, 18.0, 18.0),
            radii(5.0),
            color(1.0, 1.0, 1.0, 0.95),
        )?;
        canvas2d::stroke_circle(
            canvas,
            gfx::Point { x: sx(1.5), y: cy },
            8.0,
            2.0,
            color(1.0, 1.0, 1.0, 0.6),
        )?;
        canvas2d::stroke_round_rect(
            canvas,
            rect(sx(2.5) - 9.0, cy - 9.0, 18.0, 18.0),
            radii(6.0),
            2.0,
            color(1.0, 1.0, 1.0, 0.6),
        )?;
        canvas2d::fill_rect(
            canvas,
            rect(sx(2.5) - 5.0, cy - 1.0, 10.0, 2.0),
            color(1.0, 1.0, 1.0, 0.6),
        )?;
        canvas2d::fill_rect(
            canvas,
            rect(sx(2.5) - 1.0, cy - 5.0, 2.0, 10.0),
            color(1.0, 1.0, 1.0, 0.6),
        )?;
        canvas2d::draw_pixels(
            canvas,
            rect(sx(3.5) - 11.0, cy - 11.0, 22.0, 22.0),
            26,
            26,
            &self.heart_small,
        )?;
        canvas2d::radial_gradient(
            canvas,
            gfx::Point { x: sx(4.5), y: cy },
            9.0,
            color(0.45, 0.45, 0.55, 1.0),
            color(0.25, 0.25, 0.35, 1.0),
        )?;

        canvas2d::present(canvas)
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
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

struct Component;

impl bindings::Guest for Component {
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
        let _ = tree::upsert_node(win, &node(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas));
        let canvas = match canvas2d::bind(win, CANVAS_ID) {
            Ok(canvas) => canvas,
            Err(_) => {
                out("bind:no");
                return 1;
            }
        };

        let mut feed = Feed::new();
        out(&format!("posts:{}", feed.posts.len()));

        let mut last = clock::monotonic_nanos();
        let mut last_press_nanos: u64 = 0;
        let mut last_press_at = (0.0f32, 0.0f32);
        let mut frames: u32 = 0;

        loop {
            // The surface is the law, asked fresh every frame: a desktop
            // resize or a phone-sized surface is just the next layout.
            let layout = match canvas2d::canvas_size(canvas) {
                Ok(size) => Layout::from_canvas(size.width.max(1.0), size.height.max(1.0)),
                Err(_) => Layout::from_canvas(REQUEST_W, REQUEST_H),
            };

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

            if feed.draw(canvas, &layout).is_err() {
                out("draw:no");
                return 1;
            }
            frames += 1;

            if quick {
                if frames >= 90 {
                    break;
                }
                continue;
            }

            let _ = window::request_redraw(win);
            // Block for input when everything is at rest; poll while moving.
            let wait = if feed.settled(&layout) { None } else { Some(16) };
            match events::wait(wait) {
                Some(types::Event::CloseRequested(_)) => break,
                Some(types::Event::Wheel(wheel)) => {
                    feed.scroll_by(wheel.dy, &layout);
                }
                Some(types::Event::Pointer(p)) => {
                    if p.pressed && p.button.is_some() {
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
                }
                Some(_) | None => {}
            }
        }

        if quick {
            let layout = Layout::from_canvas(REQUEST_W, REQUEST_H);
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

bindings::export!(Component with_types_in bindings);
