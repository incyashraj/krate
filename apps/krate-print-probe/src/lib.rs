// The smallest app that prints (krate:ui/print), so the capability is
// proved through the binary a person runs.
//
//   krate run print-probe.krate -- one     print what the window shows
//   krate run print-probe.krate -- two     draw, add a page, redraw, add, finish
//
// Each run prints one line naming what came back. With KRATE_PRINT_TO set
// the host writes the document there instead of showing the dialog, so a
// test can open what would have printed.
#![no_std]
extern crate alloc;

use alloc::string::String;
use krate::{
    gfx::{canvas2d, types as gfx},
    io::{args, stdio, streams::OutputStreamExt},
    ui::{print, tree, types, window},
    Guest,
};

const W: f32 = 600.0;
const H: f32 = 800.0;

struct Component;

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

fn rgb(r: f32, g: f32, b: f32) -> gfx::Color {
    gfx::Color { r, g, b, a: 1.0 }
}

fn draw(canvas: u64, title: &str, band: gfx::Color) {
    let style = gfx::TextStyle {
        weight: 700,
        italic: false,
        letter_spacing: 0.0,
        family: gfx::FontFamily::Sans,
    };
    let _ = canvas2d::clear(canvas, rgb(1.0, 1.0, 1.0));
    let _ = canvas2d::fill_rect(canvas, gfx::Rect { x: 0.0, y: 0.0, width: W, height: 90.0 }, band);
    let _ = canvas2d::draw_text_styled(canvas, title, gfx::Point { x: 40.0, y: 60.0 }, 36.0, rgb(1.0, 1.0, 1.0), style);
    for (i, line) in ["Design, per day   800", "Build, per day    950", "Review, per hour  120"].iter().enumerate() {
        let _ = canvas2d::draw_text_styled(
            canvas,
            line,
            gfx::Point { x: 40.0, y: 170.0 + i as f32 * 50.0 },
            24.0,
            rgb(0.1, 0.1, 0.12),
            gfx::TextStyle { weight: 400, ..style },
        );
    }
    // A frame is what the window shows once it is presented; printing
    // prints what the window shows.
    let _ = canvas2d::present(canvas);
}

fn outcome(result: Result<print::PrintOutcome, types::UiError>) -> &'static str {
    match result {
        Ok(print::PrintOutcome::Printed) => "printed",
        Ok(print::PrintOutcome::Cancelled) => "cancelled",
        Ok(print::PrintOutcome::OpenedInViewer) => "opened-in-viewer",
        Err(types::UiError::InvalidWindow) => "error=invalid-window",
        Err(_) => "error=other",
    }
}

impl Guest for Component {
    fn run() -> i32 {
        let out = stdio::stdout();
        let say = |line: &str| {
            let _ = out.write_line(line);
            let _ = out.flush();
        };
        let raw = args::raw();
        let mode = raw.split('\n').find(|w| !w.is_empty()).unwrap_or("one");
        let Ok(win) = window::create("Rate card", types::WindowSize { width: W as u32, height: H as u32 }) else {
            say("window=no");
            return 1;
        };
        let _ = tree::set_root(win, &node(1, None, types::WidgetKind::Stack));
        let _ = tree::upsert_node(win, &node(2, Some(1), types::WidgetKind::Canvas));
        let Ok(canvas) = canvas2d::bind(win, 2) else {
            say("bind=no");
            return 1;
        };
        let _ = canvas2d::set_design_size(canvas, gfx::Size { width: W, height: H });
        let line: String = match mode {
            "two" => {
                draw(canvas, "Rate card", rgb(0.13, 0.35, 0.8));
                let first = print::add_page(win);
                draw(canvas, "Terms", rgb(0.8, 0.3, 0.1));
                let second = print::add_page(win);
                let done = outcome(print::finish(win, "Rate card and terms"));
                match (first, second) {
                    (Ok(1), Ok(2)) => ["pages=2 ", done].concat(),
                    _ => String::from("add-page=failed"),
                }
            }
            "bad-window" => String::from(outcome(print::window(win + 99, "x"))),
            "empty" => String::from(outcome(print::finish(win, "nothing"))),
            _ => {
                draw(canvas, "Rate card", rgb(0.13, 0.35, 0.8));
                String::from(outcome(print::window(win, "Rate card")))
            }
        };
        say(&line);
        0
    }
}

krate::export!(Component);
