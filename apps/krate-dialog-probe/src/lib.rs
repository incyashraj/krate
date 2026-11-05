// The smallest app that asks (krate:ui/dialog message and confirm), so the
// platform's own dialog is proved through the binary a person runs, on a
// phone above all: there every question used to answer no.
//
// It asks "Delete the note?" at once and after every tap, shows the answer
// in the window -- green Yes, red No -- and prints `confirm=yes` or
// `confirm=no`; then a message, printed `message=closed` once it is closed.
#![no_std]
extern crate alloc;

use krate::{
    gfx::{canvas2d, types as gfx},
    io::{stdio, streams::OutputStreamExt},
    ui::{dialog, events, tree, types, window},
    Guest,
};

const W: f32 = 360.0;
const H: f32 = 640.0;

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

fn draw(canvas: u64, answer: Option<bool>, asked: u32) {
    let style = gfx::TextStyle {
        weight: 700,
        italic: false,
        letter_spacing: 0.0,
        family: gfx::FontFamily::Sans,
    };
    let (band, word) = match answer {
        Some(true) => (rgb(0.1, 0.6, 0.3), "Yes"),
        Some(false) => (rgb(0.8, 0.2, 0.2), "No"),
        None => (rgb(0.4, 0.4, 0.45), "Asking"),
    };
    let _ = canvas2d::clear(canvas, band);
    let _ = canvas2d::draw_text_styled(
        canvas,
        word,
        gfx::Point { x: 40.0, y: 300.0 },
        64.0,
        rgb(1.0, 1.0, 1.0),
        style,
    );
    let line = if asked == 1 {
        "Asked once. Tap to ask again."
    } else {
        "Tap to ask again."
    };
    let _ = canvas2d::draw_text_styled(
        canvas,
        line,
        gfx::Point { x: 40.0, y: 360.0 },
        20.0,
        rgb(1.0, 1.0, 1.0),
        gfx::TextStyle {
            weight: 400,
            ..style
        },
    );
    let _ = canvas2d::present(canvas);
}

impl Guest for Component {
    fn run() -> i32 {
        let out = stdio::stdout();
        let say = |line: &str| {
            let _ = out.write_line(line);
            let _ = out.flush();
        };
        let Ok(win) = window::create(
            "Dialogs",
            types::WindowSize {
                width: W as u32,
                height: H as u32,
            },
        ) else {
            say("window=no");
            return 1;
        };
        let _ = tree::set_root(win, &node(1, None, types::WidgetKind::Stack));
        let _ = tree::upsert_node(win, &node(2, Some(1), types::WidgetKind::Canvas));
        let Ok(canvas) = canvas2d::bind(win, 2) else {
            say("bind=no");
            return 1;
        };
        let _ = canvas2d::set_design_size(
            canvas,
            gfx::Size {
                width: W,
                height: H,
            },
        );
        let mut asked = 0;
        loop {
            draw(canvas, None, asked);
            asked += 1;
            let answer = dialog::confirm(win, "Delete the note?", "It cannot be brought back.");
            say(match answer {
                Ok(true) => "confirm=yes",
                Ok(false) => "confirm=no",
                Err(_) => "confirm=error",
            });
            draw(canvas, answer.ok(), asked);
            if dialog::message(win, "Done", "That was the answer.").is_ok() {
                say("message=closed");
            }
            loop {
                match events::wait(None) {
                    Some(types::Event::CloseRequested(_)) => return 0,
                    Some(types::Event::Pointer(p)) if !p.pressed => break,
                    _ => {}
                }
            }
        }
    }
}

krate::export!(Component);
