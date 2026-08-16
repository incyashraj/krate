//! Krate glow — the modern-UI reference card.
//!
//! One phone-shaped screen built entirely from the Phase 1 modern primitives:
//! an angled three-stop gradient background, glass cards with rounded corners
//! and soft drop shadows, a pill button, a glow, a progress ring, and a
//! heat-dot chart. It exists to answer one question with pixels: can a Krate
//! app look like a design reference from this year, not from a few years
//! back? Every visual trick here is available to any AI-written app, and this
//! file is the worked example the authoring pack points at.
//!
//! The glow breathes and the ring sweeps, both time-based, so the screen is
//! quietly alive the way current apps are — motion as polish, not spectacle.

#[allow(warnings)]
mod bindings;

use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::time::clock;
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

const WIDTH: f32 = 360.0;
const HEIGHT: f32 = 640.0;

/// Simulated frames the `quick` run draws before reporting.
// Keep this small. In quick mode `t` is frames/60, synthetic time: 12 frames
// and 90 animate identically, but every extra frame spends fuel from the
// headless budget -- an expensive per-pixel app copying a large count here
// exhausts it and fails check-app with no loop bug anywhere.
const QUICK_FRAMES: u32 = 12;

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

fn style(weight: u16, spacing: f32) -> gfx::TextStyle {
    gfx::TextStyle {
        weight,
        italic: false,
        letter_spacing: spacing,
        family: gfx::FontFamily::Sans,
    }
}

fn mono(weight: u16) -> gfx::TextStyle {
    gfx::TextStyle {
        weight,
        italic: false,
        letter_spacing: 0.0,
        family: gfx::FontFamily::Mono,
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

/// One glass card: shadow first, then the translucent body, then a hairline
/// edge. This trio is the whole "glassmorphism" recipe, and the order
/// matters — shadow behind, never on top.
fn glass_card(canvas: u64, area: gfx::Rect, corner: f32) -> Result<(), gfx::GfxError> {
    let r = radii(corner);
    canvas2d::drop_shadow_round_rect(
        canvas,
        rect(area.x, area.y + 6.0, area.width, area.height),
        r,
        18.0,
        color(0.0, 0.0, 0.0, 0.45),
    )?;
    canvas2d::fill_round_rect(canvas, area, r, color(1.0, 1.0, 1.0, 0.055))?;
    canvas2d::stroke_round_rect(canvas, area, r, 1.0, color(1.0, 1.0, 1.0, 0.14))?;
    Ok(())
}

/// The whole screen, once per frame. `t` is seconds since launch.
fn draw(canvas: u64, t: f32) -> Result<(), gfx::GfxError> {
    // Background: a three-stop gradient running down-and-right, the rich
    // navy every current dark design uses instead of flat black.
    canvas2d::linear_gradient_stops(
        canvas,
        rect(0.0, 0.0, WIDTH, HEIGHT),
        115.0,
        &[
            gfx::GradientStop {
                offset: 0.0,
                color: color(0.043, 0.055, 0.10, 1.0),
            },
            gfx::GradientStop {
                offset: 0.55,
                color: color(0.070, 0.086, 0.17, 1.0),
            },
            gfx::GradientStop {
                offset: 1.0,
                color: color(0.11, 0.13, 0.25, 1.0),
            },
        ],
    )?;

    // Header.
    canvas2d::draw_text_styled(
        canvas,
        "Workouts",
        gfx::Point { x: 24.0, y: 64.0 },
        34.0,
        color(1.0, 1.0, 1.0, 1.0),
        style(700, -0.8),
    )?;
    canvas2d::draw_text(
        canvas,
        "This week",
        gfx::Point { x: 24.0, y: 88.0 },
        14.0,
        color(1.0, 1.0, 1.0, 0.45),
    )?;

    // Card one: the progress ring, breathing glow behind it.
    let card1 = rect(24.0, 112.0, 152.0, 168.0);
    glass_card(canvas, card1, 20.0)?;
    let cx = card1.x + card1.width / 2.0;
    let cy = card1.y + 66.0;
    // The glow breathes: radius and strength ease on a slow sine.
    let pulse = 0.5 + 0.5 * libm::sinf(t * 1.6);
    canvas2d::radial_gradient(
        canvas,
        gfx::Point { x: cx, y: cy },
        34.0 + 6.0 * pulse,
        color(0.42, 0.55, 1.0, 0.32 + 0.18 * pulse),
        color(0.42, 0.55, 1.0, 0.0),
    )?;
    // Track ring, then the sweep: a dot orbiting to suggest progress in
    // motion until an arc primitive exists.
    canvas2d::stroke_circle(
        canvas,
        gfx::Point { x: cx, y: cy },
        30.0,
        5.0,
        color(1.0, 1.0, 1.0, 0.12),
    )?;
    // 65% progress, drawn as a real arc from 12 o'clock; the tip dot
    // rides the arc's end and the whole ring eases in over the first
    // moments so the screen opens alive.
    let progress = 0.65 * (t * 1.2).min(1.0);
    let sweep_deg = progress * 360.0;
    canvas2d::stroke_arc(
        canvas,
        gfx::Point { x: cx, y: cy },
        30.0,
        -90.0,
        sweep_deg,
        5.0,
        color(0.55, 0.66, 1.0, 1.0),
    )?;
    let tip = (-90.0 + sweep_deg).to_radians();
    canvas2d::fill_circle(
        canvas,
        gfx::Point {
            x: cx + 30.0 * libm::cosf(tip),
            y: cy + 30.0 * libm::sinf(tip),
        },
        4.5,
        color(0.85, 0.90, 1.0, 1.0),
    )?;
    canvas2d::draw_text(
        canvas,
        "1",
        gfx::Point {
            x: cx - 5.0,
            y: cy + 8.0,
        },
        24.0,
        color(1.0, 1.0, 1.0, 1.0),
    )?;
    canvas2d::draw_text(
        canvas,
        "Chest + tricep",
        gfx::Point {
            x: card1.x + 18.0,
            y: card1.y + 132.0,
        },
        15.0,
        color(1.0, 1.0, 1.0, 0.92),
    )?;
    canvas2d::draw_text(
        canvas,
        "Fridays",
        gfx::Point {
            x: card1.x + 18.0,
            y: card1.y + 152.0,
        },
        12.5,
        color(1.0, 1.0, 1.0, 0.40),
    )?;

    // Card two: the big number.
    let card2 = rect(192.0, 112.0, 144.0, 168.0);
    glass_card(canvas, card2, 20.0)?;
    canvas2d::draw_text_styled(
        canvas,
        "190",
        gfx::Point {
            x: card2.x + 18.0,
            y: card2.y + 64.0,
        },
        40.0,
        color(1.0, 1.0, 1.0, 1.0),
        style(650, -1.0),
    )?;
    canvas2d::draw_text(
        canvas,
        "lbs",
        gfx::Point {
            x: card2.x + 96.0,
            y: card2.y + 64.0,
        },
        16.0,
        color(1.0, 1.0, 1.0, 0.5),
    )?;
    canvas2d::draw_text(
        canvas,
        "Body weight",
        gfx::Point {
            x: card2.x + 18.0,
            y: card2.y + 132.0,
        },
        15.0,
        color(1.0, 1.0, 1.0, 0.92),
    )?;
    canvas2d::draw_text(
        canvas,
        "31 min ago",
        gfx::Point {
            x: card2.x + 18.0,
            y: card2.y + 152.0,
        },
        12.5,
        color(1.0, 1.0, 1.0, 0.40),
    )?;

    // Card three: the heat-dot months chart, wide.
    let card3 = rect(24.0, 296.0, 312.0, 148.0);
    glass_card(canvas, card3, 22.0)?;
    let labels = ["Jan", "Feb", "Mar"];
    for (month, label) in labels.iter().enumerate() {
        let ox = card3.x + 26.0 + month as f32 * 100.0;
        canvas2d::draw_text(
            canvas,
            label,
            gfx::Point {
                x: ox + 18.0,
                y: card3.y + 30.0,
            },
            12.0,
            color(1.0, 1.0, 1.0, 0.55),
        )?;
        for row in 0..4u32 {
            for col in 0..8u32 {
                // A deterministic pseudo-pattern: bright dots where the
                // hash says a workout happened, faint dots elsewhere.
                let seed = (month as u32 * 61 + row * 17 + col * 5) % 13;
                let bright = seed == 3 || seed == 7;
                let a = if bright { 0.95 } else { 0.14 };
                let r = if bright { 3.4 } else { 2.2 };
                canvas2d::fill_circle(
                    canvas,
                    gfx::Point {
                        x: ox + col as f32 * 10.0,
                        y: card3.y + 52.0 + row as f32 * 14.0,
                    },
                    r,
                    color(1.0, 1.0, 1.0, a),
                )?;
            }
        }
    }
    canvas2d::draw_text(
        canvas,
        "Back + bicep + legs",
        gfx::Point {
            x: card3.x + 26.0,
            y: card3.y + 130.0,
        },
        15.0,
        color(1.0, 1.0, 1.0, 0.92),
    )?;

    // The accent bar: a two-stop gradient strip framed by a rounded pill --
    // gradient fill first, pill stroke to shape it visually.
    let bar = rect(24.0, 468.0, 312.0, 64.0);
    canvas2d::drop_shadow_round_rect(
        canvas,
        rect(bar.x, bar.y + 5.0, bar.width, bar.height),
        radii(18.0),
        14.0,
        color(0.0, 0.0, 0.0, 0.4),
    )?;
    canvas2d::fill_round_rect(canvas, bar, radii(18.0), color(0.16, 0.19, 0.33, 1.0))?;
    canvas2d::draw_text(
        canvas,
        "Volume lifted",
        gfx::Point {
            x: bar.x + 20.0,
            y: bar.y + 30.0,
        },
        15.0,
        color(1.0, 1.0, 1.0, 0.92),
    )?;
    canvas2d::draw_text(
        canvas,
        "Last 7 days",
        gfx::Point {
            x: bar.x + 20.0,
            y: bar.y + 48.0,
        },
        12.0,
        color(1.0, 1.0, 1.0, 0.40),
    )?;
    canvas2d::draw_text_styled(
        canvas,
        "3,200 lbs",
        gfx::Point {
            x: bar.x + 178.0,
            y: bar.y + 40.0,
        },
        21.0,
        color(1.0, 1.0, 1.0, 1.0),
        mono(600),
    )?;

    // The pill button: full-round corners make the pill, and the gradient
    // runs shallow so the button reads lit from the top-left.
    let pill = rect(24.0, 556.0, 312.0, 52.0);
    canvas2d::drop_shadow_round_rect(
        canvas,
        rect(pill.x, pill.y + 5.0, pill.width, pill.height),
        radii(26.0),
        16.0,
        color(0.30, 0.42, 1.0, 0.45),
    )?;
    canvas2d::fill_round_rect(canvas, pill, radii(26.0), color(0.42, 0.55, 1.0, 1.0))?;
    canvas2d::stroke_round_rect(
        canvas,
        pill,
        radii(26.0),
        1.0,
        color(1.0, 1.0, 1.0, 0.35),
    )?;
    canvas2d::draw_text_styled(
        canvas,
        "Start workout",
        gfx::Point {
            x: pill.x + 102.0,
            y: pill.y + 33.0,
        },
        17.0,
        color(1.0, 1.0, 1.0, 1.0),
        style(600, 0.2),
    )?;

    canvas2d::present(canvas)
}

fn out(line: &str) {
    let handle = stdio::stdout();
    let _ = handle.write(line.as_bytes());
    let _ = handle.write(b"\n");
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
            "Glow",
            types::WindowSize {
                width: WIDTH as u32,
                height: HEIGHT as u32,
            },
        ) {
            Ok(win) => win,
            Err(_) => {
                out("window:no");
                return 1;
            }
        };
        // Full-bleed: the scene runs to the very top edge with the host's
        // window controls overlaid, the way every modern editor and terminal
        // window looks. `let _ =` because a host that cannot do it says
        // unsupported and the window simply keeps its standard title bar.
        let _ = window::set_full_bleed(win, true);
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

        let started = clock::monotonic_nanos();
        let mut frames: u32 = 0;

        // When the next frame is due; see the note where a redraw is NOT
        // requested below.
        const FRAME_NANOS: u64 = 1_000_000_000 / 60;
        let mut next_frame = clock::monotonic_nanos();

        loop {
            let t = if quick {
                frames as f32 / 60.0
            } else {
                (clock::monotonic_nanos().saturating_sub(started)) as f32 / 1_000_000_000.0
            };

            if draw(canvas, t).is_err() {
                out("draw:no");
                return 1;
            }
            frames += 1;

            if quick {
                if frames >= QUICK_FRAMES {
                    break;
                }
                continue;
            }

            // Deliberately no `request-redraw` here. This loop draws and
            // presents on its own schedule, and asking for a redraw posts an
            // event that comes straight back -- the queue is never empty,
            // every `wait` returns instantly, and the app pins a core while
            // looking idle. request-redraw is for waking a loop that would
            // otherwise sit idle in `wait`; a continuously animating loop is
            // never idle. Pace against the clock instead.
            next_frame = next_frame.saturating_add(FRAME_NANOS);
            let after_draw = clock::monotonic_nanos();
            if next_frame < after_draw {
                next_frame = after_draw;
            }
            let mut closing = false;
            loop {
                let now = clock::monotonic_nanos();
                let remaining = next_frame.saturating_sub(now);
                if remaining == 0 {
                    break;
                }
                let millis = (remaining / 1_000_000) as u32;
                match events::wait(Some(millis.max(1))) {
                    Some(types::Event::CloseRequested(_)) => {
                        closing = true;
                        break;
                    }
                    Some(_) | None => {}
                }
            }
            if closing {
                break;
            }
        }

        out("cards:4");
        out("glow:yes");
        0
    }
}

bindings::export!(Component with_types_in bindings);
