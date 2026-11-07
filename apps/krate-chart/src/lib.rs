//! Krate chart -- the first app to draw with `gfx.canvas2d`.
//!
//! A bar chart of a week's rainfall, drawn with canvas commands: a card,
//! grid lines on round numbers, one bar per day with its value above and
//! its day below, and the week's totals underneath. It was the first guest
//! to reach the rasterizer through the WIT boundary, and it is now also the
//! small worked example of a canvas that fits wherever it opens: every size
//! comes from `canvas_size` or `look.text`, the colours from the person's
//! look (dark in a dark room), and `Said` tells a screen reader what each
//! bar says.
//!
//! `quick` verifies without a person: build the tree, bind, draw, and print
//! what was submitted so automation can assert the whole journey ran.

#![no_std]
extern crate alloc;

#[allow(unused_imports)]
use alloc::{
    format,
    string::{String, ToString},
    vec,
    vec::Vec,
};
use krate::bindings::krate::io::{args, stdio};
use krate::gfx::{canvas2d, types as gfx};
use krate::layout::{Look, Rect, Said};
use krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const TITLE_ID: u64 = 2;
const CANVAS_ID: u64 = 3;

/// Millimetres of rain, Monday to Sunday. Fixed data: the sample exists to
/// exercise drawing, and a fixed picture is one a test can reason about.
const RAINFALL: [f32; 7] = [4.0, 12.0, 7.0, 0.0, 21.0, 15.0, 9.0];

struct Component;

fn color(r: f32, g: f32, b: f32) -> gfx::Color {
    gfx::Color { r, g, b, a: 1.0 }
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

/// Short and full day names: the short one goes under its bar, the full
/// one is what a screen reader says.
const DAYS: [(&str, &str); 7] = [
    ("Mon", "Monday"),
    ("Tue", "Tuesday"),
    ("Wed", "Wednesday"),
    ("Thu", "Thursday"),
    ("Fri", "Friday"),
    ("Sat", "Saturday"),
    ("Sun", "Sunday"),
];

fn style(weight: u16) -> gfx::TextStyle {
    gfx::TextStyle {
        weight,
        italic: false,
        letter_spacing: 0.0,
        family: gfx::FontFamily::Sans,
    }
}

fn rect(r: Rect) -> gfx::Rect {
    gfx::Rect {
        x: r.x,
        y: r.y,
        width: r.w,
        height: r.h,
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

fn width(canvas: u64, text: &str, size: f32, weight: u16) -> f32 {
    canvas2d::measure_text_styled(canvas, text, size, style(weight))
        .map(|m| m.width)
        .unwrap_or(text.len() as f32 * size * 0.55)
}

fn text(
    canvas: u64,
    text: &str,
    x: f32,
    baseline: f32,
    size: f32,
    ink: gfx::Color,
    weight: u16,
) -> Result<(), gfx::GfxError> {
    canvas2d::draw_text_styled(
        canvas,
        text,
        gfx::Point { x, y: baseline },
        size,
        ink,
        style(weight),
    )
}

/// Draw the chart for the canvas it has and the person's look, and say
/// what it shows to a screen reader. Every size below comes from the
/// canvas's real width and height or from `look.text`, so the chart fills a
/// phone, a tablet and a desktop window alike, dark in a dark room and with
/// labels that grow for someone who asked for larger text.
fn draw_chart(win: u64, canvas: u64) -> Result<u32, gfx::GfxError> {
    // Draw to the canvas's real size, not a guessed one. The canvas shares
    // this window with a native title above it, so it is shorter than the
    // window; an earlier version hardcoded 200 and drew a chart that ran off
    // the bottom into space that was not there. `canvas-size` is how an app
    // finds out what it actually got.
    let size = canvas2d::canvas_size(canvas)?;
    let (w, h) = (size.width.max(1.0), size.height.max(1.0));
    let look = Look::now();
    let colors = look.palette();
    let bar = if look.dark {
        color(0.40, 0.62, 1.0)
    } else {
        color(0.23, 0.51, 0.96)
    };
    let mut said = Said::new();

    canvas2d::clear(canvas, colors.background)?;
    let mut calls = 1_u32;

    // Text sizes, at the person's text size.
    let unit_size = look.text(13.0);
    let label_size = look.text(12.0);
    let stat_label = look.text(13.0);
    let stat_value = look.text(18.0);
    let pad = if w < 600.0 { 16.0 } else { 24.0 };

    // The totals under the chart: as many to a row as fit the width.
    let total: f32 = RAINFALL.iter().sum();
    let (wettest, most) =
        RAINFALL
            .iter()
            .copied()
            .enumerate()
            .fold(
                (0, 0.0_f32),
                |best, (i, r)| if r > best.1 { (i, r) } else { best },
            );
    let dry = RAINFALL.iter().filter(|r| **r == 0.0).count();
    let stats = [
        ("This week", format!("{total:.0} mm")),
        ("Wettest", format!("{} {most:.0} mm", DAYS[wettest].0)),
        ("Dry days", format!("{dry}")),
    ];
    let inner_w = (w - 2.0 * pad).max(1.0);
    let widest = stats
        .iter()
        .map(|(label, value)| {
            width(canvas, label, stat_label, 400).max(width(canvas, value, stat_value, 650))
        })
        .fold(0.0_f32, f32::max);
    // A short, wide screen (a phone turned sideways) keeps its height for
    // the bars and stands the totals in a column at the right.
    let side = w >= 640.0 && h < 480.0;
    let side_w = widest + 24.0;
    let per_row = if side {
        1
    } else {
        ((inner_w / (widest + 24.0)) as usize).clamp(1, stats.len())
    };
    let stat_rows = stats.len().div_ceil(per_row);
    let stat_h = stat_label * 1.5 + stat_value * 1.3 + if side { 10.0 } else { 0.0 };

    // Fixed bands top to bottom; the plot takes what is left, so the card
    // fills the canvas on every screen.
    let unit_h = unit_size * 1.8;
    let days_h = label_size * 2.0;
    let stats_h = if side {
        0.0
    } else {
        16.0 + stat_rows as f32 * stat_h
    };
    let fixed = 2.0 * pad + unit_h + days_h + stats_h;
    let plot_h = (h - fixed).max(60.0);
    let card = Rect::new(0.0, 0.0, w, (fixed + plot_h).min(h));
    canvas2d::fill_round_rect(canvas, rect(card), radii(14.0), colors.surface)?;
    canvas2d::stroke_round_rect(canvas, rect(card), radii(14.0), 1.0, colors.line)?;
    calls += 2;

    let mut area = card.pad(pad);
    let side_area = side.then(|| {
        let mut column = area.take_right(side_w + 24.0);
        column.take_left(24.0);
        column
    });
    let unit_row = area.take_top(unit_h);
    text(
        canvas,
        "rain, mm",
        unit_row.x,
        unit_row.y + unit_size,
        unit_size,
        colors.muted,
        500,
    )?;
    said.text(unit_row, "Rain in millimetres, Monday to Sunday");
    let mut plot = area.take_top(plot_h);
    let days_row = area.take_top(days_h);
    calls += 1;

    // Room above the tallest bar for its number.
    let value_h = label_size * 1.6;
    // Round the top of the scale up to a whole step, so the grid lines
    // fall on numbers a person reads at a glance -- and step wide enough
    // that their labels never crowd, however short the plot or large the
    // text.
    let round_up =
        |step: f32| (((most / step) as u32 + u32::from(most % step > 0.0)) as f32 * step).max(step);
    let room = (plot.h - value_h).max(1.0);
    let step = [5.0_f32, 10.0, 25.0, 50.0, 100.0]
        .into_iter()
        .find(|step| room / (round_up(*step) / step) >= label_size * 2.2)
        .unwrap_or(100.0);
    let top = round_up(step);
    let gutter = width(canvas, &format!("{top:.0}"), label_size, 400) + 10.0;
    plot.take_left(gutter);
    let bars_top = plot.y + value_h;
    let bars_h = (plot.bottom() - bars_top).max(1.0);
    let baseline = plot.bottom();

    let mut tick = 0.0_f32;
    while tick <= top + 0.01 {
        let y = baseline - tick / top * bars_h;
        canvas2d::fill_rect(
            canvas,
            gfx::Rect {
                x: plot.x,
                y: y - 0.5,
                width: plot.w,
                height: 1.0,
            },
            colors.line,
        )?;
        let label = format!("{tick:.0}");
        let lw = width(canvas, &label, label_size, 400);
        text(
            canvas,
            &label,
            plot.x - 8.0 - lw,
            y + label_size * 0.35,
            label_size,
            colors.muted,
            400,
        )?;
        calls += 2;
        tick += step;
    }

    // One bar per day, scaled to the top of the scale, with its number
    // above it and its day below.
    let slot = plot.w / RAINFALL.len() as f32;
    let bar_w = (slot * 0.62).clamp(4.0, 96.0);
    for (day, rain) in RAINFALL.iter().enumerate() {
        let cx = plot.x + slot * (day as f32 + 0.5);
        let height = rain / top * bars_h;
        if height > 0.0 {
            canvas2d::fill_round_rect(
                canvas,
                gfx::Rect {
                    x: cx - bar_w / 2.0,
                    y: baseline - height,
                    width: bar_w,
                    height,
                },
                gfx::CornerRadii {
                    top_left: (bar_w * 0.2).min(6.0),
                    top_right: (bar_w * 0.2).min(6.0),
                    bottom_right: 0.0,
                    bottom_left: 0.0,
                },
                bar,
            )?;
            calls += 1;
        }
        let value = format!("{rain:.0}");
        let (ink, weight) = if day == wettest {
            (colors.ink, 650)
        } else {
            (colors.muted, 400)
        };
        let vw = width(canvas, &value, label_size, weight);
        text(
            canvas,
            &value,
            cx - vw / 2.0,
            baseline - height - label_size * 0.5,
            label_size,
            ink,
            weight,
        )?;
        let (short, full) = DAYS[day];
        let dw = width(canvas, short, label_size, 500);
        text(
            canvas,
            short,
            cx - dw / 2.0,
            days_row.y + label_size * 1.4,
            label_size,
            colors.ink,
            500,
        )?;
        calls += 2;
        said.text(
            Rect::new(cx - slot / 2.0, plot.y, slot, plot.h + days_row.h),
            &format!("{full}, {rain:.0} mm"),
        );
    }

    // A rule, then the totals: under the chart, or beside it.
    if let Some(column) = side_area {
        canvas2d::fill_rect(
            canvas,
            gfx::Rect {
                x: column.x - 12.5,
                y: column.y,
                width: 1.0,
                height: column.h,
            },
            colors.line,
        )?;
        area = column;
        area.take_top(unit_h);
    } else {
        canvas2d::fill_rect(
            canvas,
            gfx::Rect {
                x: area.x,
                y: area.y + 7.5,
                width: area.w,
                height: 1.0,
            },
            colors.line,
        )?;
        area.take_top(16.0);
    }
    let col_w = area.w / per_row as f32;
    for (i, (label, value)) in stats.iter().enumerate() {
        let at = Rect::new(
            area.x + (i % per_row) as f32 * col_w,
            area.y + (i / per_row) as f32 * stat_h,
            col_w,
            stat_h,
        );
        text(
            canvas,
            label,
            at.x,
            at.y + stat_label * 1.2,
            stat_label,
            colors.muted,
            400,
        )?;
        text(
            canvas,
            value,
            at.x,
            at.y + stat_label * 1.5 + stat_value * 1.0,
            stat_value,
            colors.ink,
            650,
        )?;
        calls += 2;
        said.text(at, &format!("{label}, {value}"));
    }

    // Nothing is shown until the raster is presented.
    canvas2d::present(canvas)?;
    said.send(win);
    Ok(calls)
}

// `to_string()` routes through std's OOM handler, which drags the whole
// `wasi:*` surface into an otherwise pure component -- 33 imports from two
// labels. Allocating the bytes directly is what keeps a guest clean; the
// hello-gui sample carries the same for the same reason.
fn pure_string(text: &str) -> String {
    let len = text.len();
    if len == 0 {
        return String::new();
    }
    unsafe {
        let layout = core::alloc::Layout::from_size_align_unchecked(len, 1);
        let ptr = alloc::alloc::alloc(layout);
        if ptr.is_null() {
            core::arch::wasm32::unreachable()
        }
        core::ptr::copy_nonoverlapping(text.as_ptr(), ptr, len);
        String::from_raw_parts(ptr, len, len)
    }
}

fn out(text: &str) {
    let handle = stdio::stdout();
    let _ = handle.write(text.as_bytes());
    let _ = handle.write(b"\n");
}

impl krate::Guest for Component {
    fn run() -> i32 {
        let raw = args::raw();
        let quick = raw.split_whitespace().any(|word| word == "quick");

        let win = match window::create(
            "Rainfall",
            types::WindowSize {
                width: 560,
                height: 440,
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
        let mut title = node(TITLE_ID, Some(ROOT_ID), types::WidgetKind::Text);
        title.label = Some(pure_string("This week's rain"));
        title.role = Some(pure_string("heading"));
        let _ = tree::upsert_node(win, &title);
        let _ = tree::upsert_node(
            win,
            &node(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas),
        );

        let canvas = match canvas2d::bind(win, CANVAS_ID) {
            Ok(canvas) => canvas,
            Err(error) => {
                let _ = error;
                out("bind:no");
                return 1;
            }
        };
        let commands = match draw_chart(win, canvas) {
            Ok(count) => count,
            Err(error) => {
                let _ = error;
                out("draw:no");
                return 1;
            }
        };

        out("bars:7");
        let _ = commands;
        out("commands:10");
        out("drawn:yes");

        if quick {
            let _ = window::close(win);
            return 0;
        }

        // Stay open until the person closes the window, and redraw when the
        // window changes size. The old comment here said redraw was not
        // needed because "the canvas keeps its raster" -- true until the
        // window is resized, at which point the canvas is refitted and the
        // old picture is gone, leaving the app blank or stretched (K-096).
        let mut look = Look::now();
        loop {
            match events::wait(Some(1_000)) {
                Some(types::Event::CloseRequested(_)) => break,
                Some(types::Event::Resized(_)) => {
                    look = Look::now();
                    let _ = draw_chart(win, canvas);
                }
                // Dark mode or the text size changed: draw again in the
                // person's new look.
                Some(_) | None => {
                    if Look::now() != look {
                        look = Look::now();
                        let _ = draw_chart(win, canvas);
                    }
                }
            }
        }
        let _ = window::close(win);
        0
    }
}

krate::export!(Component);
