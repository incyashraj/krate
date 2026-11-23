// Tray Probe: a counter in the menu bar (krate:ui/tray, IC-1003). One icon
// carries a menu -- Show Window, a Paused check, Add One, Quit -- and its
// count as the title beside it; a second icon has no menu and reports left
// and right clicks. Close the window and the app keeps running in the tray;
// Show Window brings it back; Quit ends it.
//
//   krate run tray-probe.krate -- quick   show both icons, print what each
//                                         call answered, take them away, exit
#![no_std]
extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use krate::gfx::{canvas2d, types as gfx};
use krate::layout::{Look, Rect, Said, Screen};
use krate::ui::menus::{Item, ItemKind, Role};
use krate::ui::tray;
use krate::{
    io::{args, stdio},
    ui::{
        events, tree,
        types::{self, Event},
        window,
    },
    Guest,
};

const SHOW: u64 = 1;
const PAUSED: u64 = 2;
const ADD: u64 = 3;
const PING_LEFT: u64 = 20;
const PING_RIGHT: u64 = 21;

fn say(line: &str) {
    let out = stdio::stdout();
    let _ = out.write(line.as_bytes());
    let _ = out.write(b"\n");
}

/// A ring (or a dot), black on clear: a template icon the Mac tints.
fn icon(dot: bool) -> tray::Icon {
    let size = 36u32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    let c = (size as f32 - 1.0) / 2.0;
    for y in 0..size {
        for x in 0..size {
            let d = libm_sqrt((x as f32 - c) * (x as f32 - c) + (y as f32 - c) * (y as f32 - c));
            let on = if dot { d <= 9.0 } else { (11.0..=16.0).contains(&d) || (d <= 4.0) };
            rgba.extend_from_slice(&[0, 0, 0, if on { 255 } else { 0 }]);
        }
    }
    tray::Icon { width: size, height: size, rgba, template: true }
}

/// Square root without std: a few Newton steps are plenty for a picture.
fn libm_sqrt(v: f32) -> f32 {
    if v <= 0.0 {
        return 0.0;
    }
    let mut x = v;
    for _ in 0..12 {
        x = 0.5 * (x + v / x);
    }
    x
}

fn item(id: u64, kind: ItemKind, label: &str) -> Item {
    Item {
        id,
        parent: 0,
        kind,
        label: String::from(label),
        enabled: true,
        checked: false,
        shortcut: String::new(),
        role: Role::None,
    }
}

fn menu() -> Vec<Item> {
    alloc::vec![
        item(SHOW, ItemKind::Normal, "Show Window"),
        item(0, ItemKind::Separator, ""),
        item(PAUSED, ItemKind::Check, "Paused"),
        item(ADD, ItemKind::Normal, "Add One"),
        item(0, ItemKind::Separator, ""),
        Item { role: Role::Quit, ..item(0, ItemKind::Normal, "") },
    ]
}

fn node(id: u64, parent: Option<u64>, kind: types::WidgetKind) -> types::WidgetNode {
    types::WidgetNode {
        id,
        parent,
        kind,
        label: None,
        role: None,
        style: types::Style { width: None, height: None, grow: 1.0, padding: 0.0, text: None, place: None, box_: None },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

fn style(weight: u16) -> gfx::TextStyle {
    gfx::TextStyle { weight, italic: false, letter_spacing: 0.0, family: gfx::FontFamily::Sans }
}

fn open() -> Option<(u64, u64)> {
    let win = window::create("Tray Probe", types::WindowSize { width: 480, height: 300 }).ok()?;
    tree::set_root(win, &node(1, None, types::WidgetKind::Stack)).ok()?;
    tree::upsert_node(win, &node(2, Some(1), types::WidgetKind::Canvas)).ok()?;
    let canvas = canvas2d::bind(win, 2).ok()?;
    Some((win, canvas))
}

fn draw(win: u64, canvas: u64, count: u32, paused: bool, last: &str) -> Result<(), gfx::GfxError> {
    let screen = Screen::of(win);
    let look: Look = screen.look;
    let colors = look.palette();
    let area = screen.safe_area().inset(20.0, 24.0, 20.0, 24.0);
    canvas2d::clear(canvas, colors.background)?;
    let line = |t: &str, y: f32, size: f32, w: u16, color| {
        canvas2d::draw_text_styled(canvas, t, gfx::Point { x: area.x, y: area.y + y }, look.text(size), color, style(w))
    };
    line("Tray Probe", 30.0, 26.0, 800, colors.ink)?;
    line("The count is in the menu bar. Close this window: it keeps counting there.", 60.0, 13.0, 500, colors.muted)?;
    line(&format!("Count: {count}{}", if paused { "  (paused)" } else { "" }), 110.0, 20.0, 700, colors.ink)?;
    line(&format!("Last: {last}"), 145.0, 15.0, 500, colors.ink)?;
    let strip = gfx::Rect { x: area.x, y: area.y + area.h - 2.0, width: area.w, height: 2.0 };
    canvas2d::fill_rect(canvas, strip, colors.accent)?;
    canvas2d::present(canvas)?;
    let mut said = Said::new();
    said.heading(Rect::new(area.x, area.y, area.w, 40.0), "Tray Probe");
    said.text(Rect::new(area.x, area.y + 90.0, area.w, 24.0), &format!("Count: {count}"));
    said.send(win);
    Ok(())
}

struct Component;

impl Guest for Component {
    fn run() -> i32 {
        let quick = args::raw().split('\n').any(|a| a == "quick");
        say(&format!("tray:available={}", tray::available()));
        let main = match tray::show(&icon(false), "Tray Probe: a counter") {
            Ok(id) => id,
            Err(e) => {
                say(&format!("tray:show=failed {e:?}"));
                return 1;
            }
        };
        say("tray:show=ok");
        say(&match tray::set_menu(main, &menu()) {
            Ok(()) => String::from("tray:menu=ok"),
            Err(e) => format!("tray:menu=failed {e:?}"),
        });
        let _ = tray::set_title(main, "0");
        let ping = tray::show(&icon(true), "Click me, left or right").ok();
        if let Some(p) = ping {
            let _ = tray::on_click(p, PING_LEFT, PING_RIGHT);
        }
        if quick {
            let mut bad = icon(false);
            bad.rgba.truncate(10);
            say(&match tray::show(&bad, "broken") {
                Err(e) => format!("tray:bad-icon=refused {e:?}"),
                Ok(_) => String::from("tray:bad-icon=allowed"),
            });
            if let Some(p) = ping {
                tray::remove(p);
            }
            tray::remove(main);
            return 0;
        }
        let mut shown = open();
        let mut count = 0u32;
        let mut paused = false;
        let mut last = String::from("(nothing yet)");
        let mut ticks = 0u32;
        loop {
            if let Some((win, canvas)) = shown {
                let _ = draw(win, canvas, count, paused, &last);
            }
            let mut next = events::wait(Some(500));
            if next.is_none() {
                ticks += 1;
                if ticks % 2 == 0 && !paused {
                    count += 1;
                    let _ = tray::set_title(main, &format!("{count}"));
                }
            }
            while let Some(event) = next {
                match event {
                    Event::CloseRequested(w) => {
                        // A tray app: the window goes, the app stays.
                        let _ = window::close(w);
                        shown = None;
                        last = String::from("window closed, still running");
                        say("window:closed");
                    }
                    Event::Action(id) => {
                        last = match id {
                            SHOW => String::from("Show Window"),
                            PAUSED => String::from("Paused"),
                            ADD => String::from("Add One"),
                            PING_LEFT => String::from("left click on the dot"),
                            PING_RIGHT => String::from("right click on the dot"),
                            _ => format!("id {id}"),
                        };
                        say(&format!("chosen:{id} {last}"));
                        match id {
                            SHOW if shown.is_none() => shown = open(),
                            PAUSED => paused = krate::ui::menus::checked(PAUSED),
                            ADD => {
                                count += 1;
                                let _ = tray::set_title(main, &format!("{count}"));
                            }
                            _ => {}
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
