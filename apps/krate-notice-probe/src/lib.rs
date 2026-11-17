// Notice Board: one notification shown, replaced and taken back by the
// app's own id, through krate:ui/notifications (IC-271).
//
//   krate run notice-probe.krate -- quick   post, replace, withdraw once and
//                                           print each answer, then exit:
//                                           `post:ok`, `replace:ok`,
//                                           `withdraw:done`
//   krate run notice-probe.krate -- show    one step and exit (also
//                                           `replace`, `take-back`)
//   krate run notice-probe.krate            three buttons; the person sees
//                                           one notification change and go
//
// The app is never told whether anyone saw one (F-323), so the page says
// only what it asked for.
#![no_std]
extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use krate::gfx::{canvas2d, types as gfx};
use krate::layout::{Rect, Screen};
use krate::ui::notifications;
use krate::{
    io::{args, stdio},
    ui::{
        events, tree,
        types::{self, Event},
        window,
    },
    Guest,
};

const ID: &str = "upload";

const BUTTONS: [(&str, &str); 3] = [
    ("Show", "Uploading: 1 of 3 photos"),
    ("Replace", "The same notification, now 3 of 3"),
    ("Take back", "Remove it from the screen"),
];

fn rgb(r: f32, g: f32, b: f32) -> gfx::Color {
    gfx::Color { r, g, b, a: 1.0 }
}

fn rect(r: Rect) -> gfx::Rect {
    gfx::Rect { x: r.x, y: r.y, width: r.w, height: r.h }
}

fn radii(all: f32) -> gfx::CornerRadii {
    gfx::CornerRadii { top_left: all, top_right: all, bottom_right: all, bottom_left: all }
}

fn style(weight: u16) -> gfx::TextStyle {
    gfx::TextStyle { weight, italic: false, letter_spacing: 0.0, family: gfx::FontFamily::Sans }
}

fn say(line: &str) {
    let out = stdio::stdout();
    let _ = out.write(line.as_bytes());
    let _ = out.write(b"\n");
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

/// What a button does; the answer is what the page shows under it.
fn act(which: usize) -> String {
    let answer = match which {
        0 => notifications::post(ID, "Uploading", "1 of 3 photos"),
        1 => notifications::post(ID, "Uploading", "3 of 3 photos, done"),
        _ => {
            notifications::withdraw(ID);
            return String::from("Asked to take it back.");
        }
    };
    match answer {
        Ok(()) => String::from(if which == 0 { "Asked to show it." } else { "Asked to replace it." }),
        Err(err) => format!("Refused: {err:?}"),
    }
}

struct Page {
    buttons: Vec<Rect>,
    pressed: Option<usize>,
    last: String,
}

fn draw(canvas: u64, screen: &Screen, page: &mut Page) -> Result<(), gfx::GfxError> {
    let ink = rgb(0.07, 0.07, 0.09);
    let muted = rgb(0.43, 0.44, 0.48);
    let accent = rgb(0.23, 0.30, 0.95);
    canvas2d::clear(canvas, rgb(0.96, 0.96, 0.97))?;
    let area = screen.safe_area().inset(16.0, 20.0, 16.0, 20.0);
    let col = area.w.min(520.0);
    let x = area.x + (area.w - col) / 2.0;
    let mut y = area.y + 34.0;
    canvas2d::draw_text_styled(canvas, "Notice Board", gfx::Point { x, y }, 32.0, ink, style(800))?;
    y += 26.0;
    canvas2d::draw_text_styled(canvas, "One notification, changed and taken back.", gfx::Point { x, y }, 15.0, muted, style(450))?;
    y += 26.0;
    page.buttons.clear();
    for (i, (name, what)) in BUTTONS.iter().enumerate() {
        let b = Rect::new(x, y, col, 72.0);
        page.buttons.push(b);
        let on = page.pressed == Some(i);
        canvas2d::drop_shadow_round_rect(canvas, rect(b), radii(18.0), 12.0, gfx::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.06 })?;
        canvas2d::fill_round_rect(canvas, rect(b), radii(18.0), if on { rgb(0.90, 0.91, 0.99) } else { rgb(1.0, 1.0, 1.0) })?;
        canvas2d::draw_text_styled(canvas, name, gfx::Point { x: b.x + 18.0, y: b.y + 30.0 }, 18.0, if on { accent } else { ink }, style(700))?;
        canvas2d::draw_text_styled(canvas, what, gfx::Point { x: b.x + 18.0, y: b.y + 53.0 }, 13.5, muted, style(450))?;
        y += 84.0;
    }
    y += 10.0;
    canvas2d::draw_text_styled(canvas, &page.last, gfx::Point { x, y }, 15.0, ink, style(500))?;
    canvas2d::present(canvas)
}

struct Component;

impl Guest for Component {
    fn run() -> i32 {
        // One step per run, for watching it happen: the system keeps the
        // app's notification between runs, so a later run replaces or
        // takes back what an earlier one showed.
        for step in args::raw().split('\n') {
            if let Some(i) = ["show", "replace", "take-back"].iter().position(|s| *s == step) {
                say(&act(i));
                return 0;
            }
        }
        if args::raw().split('\n').any(|a| a == "quick") {
            let post = notifications::post(ID, "Uploading", "1 of 3 photos");
            say(&format!("post:{}", if post.is_ok() { "ok" } else { "refused" }));
            let replace = notifications::post(ID, "Uploading", "3 of 3 photos, done");
            say(&format!("replace:{}", if replace.is_ok() { "ok" } else { "refused" }));
            notifications::withdraw(ID);
            say("withdraw:done");
            let bad = notifications::post("../other", "x", "y");
            say(&format!("bad-id:{}", if bad.is_err() { "refused" } else { "ok" }));
            return 0;
        }
        let Ok(win) = window::create("Notice Board", types::WindowSize { width: 440, height: 520 }) else {
            return 1;
        };
        if tree::set_root(win, &node(1, None, types::WidgetKind::Stack)).is_err()
            || tree::upsert_node(win, &node(2, Some(1), types::WidgetKind::Canvas)).is_err()
        {
            return 1;
        }
        let Ok(canvas) = canvas2d::bind(win, 2) else {
            return 1;
        };
        let mut page = Page { buttons: Vec::new(), pressed: None, last: String::from("Nothing asked yet.") };
        loop {
            let screen = Screen::of(win);
            if draw(canvas, &screen, &mut page).is_err() {
                return 1;
            }
            let _ = window::request_redraw(win);
            let mut next = events::wait(None);
            while let Some(event) = next {
                match event {
                    Event::CloseRequested(_) => return 0,
                    Event::Pointer(p) if p.button.is_some() && p.pressed => {
                        page.pressed = page.buttons.iter().position(|b| b.contains(p.x, p.y));
                    }
                    Event::Pointer(p) if p.button.is_some() => {
                        if let Some(i) = page.pressed.take() {
                            if page.buttons[i].contains(p.x, p.y) {
                                page.last = act(i);
                                say(&page.last);
                            }
                        }
                    }
                    _ => {}
                }
                next = events::poll();
            }
        }
    }
}

krate::export!(Component);
