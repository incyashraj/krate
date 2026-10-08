// The smallest app that asks about the mouse (krate:ui/pointer, IC-909), so
// the interface is proved through the binary a person runs.
//
//   krate run pointer-probe.krate              one report, then exit
//   krate run pointer-probe.krate -- watch     ten seconds: click to hold the
//                                              pointer, move the mouse, press
//                                              Escape; one line per second
//
// A report line: where the pointer is, which buttons are down, the motion
// since the last report, whether it is held, and what `capture` answered.
#![no_std]
extern crate alloc;

use alloc::format;
use alloc::string::String;
use krate::{
    io::{
        args, stdio,
        streams::OutputStream,
    },
    time::clock,
    ui::{
        events,
        pointer::{self, PointerButton},
        types::{self, Event},
        window,
    },
    Guest,
};

struct Component;

fn report(win: u64, capture: &str) -> String {
    let at = match pointer::position(win) {
        Some((x, y)) => format!("{x:.0},{y:.0}"),
        None => String::from("none"),
    };
    let held = |b| if pointer::button_held(b) { "down" } else { "up" };
    let (dx, dy) = pointer::take_motion(win);
    format!(
        "position={at} primary={} secondary={} middle={} motion={dx:.0},{dy:.0} captured={} capture={capture}",
        held(PointerButton::Primary),
        held(PointerButton::Secondary),
        held(PointerButton::Middle),
        pointer::captured(win),
    )
}

fn answer(result: Result<(), types::UiError>) -> String {
    match result {
        Ok(()) => String::from("ok"),
        Err(types::UiError::Unsupported(why)) => format!("refused: {why}"),
        Err(other) => format!("error: {other:?}"),
    }
}

impl Guest for Component {
    fn run() -> i32 {
        let out = stdio::stdout();
        let Ok(win) = window::create(
            "Pointer probe",
            types::WindowSize {
                width: 480,
                height: 320,
            },
        ) else {
            let _ = write_line(&out, "window=refused");
            return 1;
        };
        let watch = args::raw().split('\n').any(|w| w == "watch");
        if !watch {
            // A window is not focused the instant it is made; give the system
            // a moment, as a game would before its first click.
            let start = clock::monotonic_nanos();
            let mut focused = false;
            while !focused && clock::monotonic_nanos() - start < 1_500_000_000 {
                if let Some(Event::FocusChanged(_)) | Some(Event::Resized(_)) = events::wait(Some(50)) {}
                focused = pointer::capture(win).is_ok();
            }
            let capture = if focused {
                String::from("ok")
            } else {
                answer(pointer::capture(win))
            };
            let _ = write_line(&out, &report(win, &capture));
            pointer::release(win);
            let _ = write_line(&out, &format!("released captured={}", pointer::captured(win)));
            let _ = window::close(win);
            return 0;
        }
        let start = clock::monotonic_nanos();
        let mut last = start;
        let mut capture = String::from("-");
        while clock::monotonic_nanos() - start < 10_000_000_000 {
            match events::wait(Some(16)) {
                Some(Event::CloseRequested(_)) => break,
                Some(Event::Pointer(p)) if p.pressed => capture = answer(pointer::capture(win)),
                _ => {}
            }
            if clock::monotonic_nanos() - last >= 1_000_000_000 {
                last = clock::monotonic_nanos();
                let _ = write_line(&out, &report(win, &capture));
            }
        }
        let _ = window::close(win);
        0
    }
}

fn write_line(out: &OutputStream, line: &str) -> bool {
    out.write(format!("{line}\n").as_bytes()).is_ok()
}

krate::export!(Component);
