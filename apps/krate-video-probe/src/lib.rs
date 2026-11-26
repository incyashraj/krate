// Clip: a short video played in the app's own layout, from krate:gfx/video
// (IC-922), with the controls a person expects: play and pause, a scrubber
// to drag or tap, loop and sound.
//
//   krate run video-probe.krate -- quick [https://...]
//                                          plays three seconds headless,
//                                          then one line and exit:
//                                          `video:ready=yes duration=8000
//                                          size=960x540 moved=yes frames=N
//                                          volume=ok rate=ok float=ok`
//                                          or `video:problem=<why>`
#![no_std]
extern crate alloc;

use alloc::format;
use alloc::string::String;
use krate::bindings::krate::time::clock;
use krate::gfx::{canvas2d, types as gfx, video};
use krate::layout::{Rect, Said, Screen};
use krate::{
    io::{args, stdio},
    ui::{
        events, pointer, tree,
        types::{self, Event},
        window,
    },
    Guest,
};

const CLIP: &str = "clips/sample.mp4";

fn rgb(r: f32, g: f32, b: f32) -> gfx::Color {
    gfx::Color { r, g, b, a: 1.0 }
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

fn r(rect: Rect) -> gfx::Rect {
    gfx::Rect { x: rect.x, y: rect.y, width: rect.w, height: rect.h }
}

fn clock_text(millis: u64) -> String {
    let s = millis / 1000;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Where everything sits, worked out once per frame from the screen.
struct Layout {
    picture: Rect,
    bar: Rect,
    play: Rect,
    looping: Rect,
    sound: Rect,
}

fn layout(screen: &Screen, state: &video::VideoState) -> Layout {
    let area = screen.safe_area().inset(16.0, 16.0, 16.0, 16.0);
    let top = area.y + 52.0;
    // The picture keeps its own shape: as wide as the screen allows, no
    // taller than what is left above the controls.
    let (w, h) = if state.width > 0 && state.height > 0 {
        (state.width as f32, state.height as f32)
    } else {
        (16.0, 9.0)
    };
    let room_h = (area.bottom() - 150.0 - top).max(80.0);
    let mut pw = area.w;
    let mut ph = pw * h / w;
    if ph > room_h {
        ph = room_h;
        pw = ph * w / h;
    }
    let picture = Rect::new(area.x + (area.w - pw) / 2.0, top, pw, ph);
    let bar = Rect::new(area.x, picture.bottom() + 28.0, area.w, 28.0);
    let row = bar.bottom() + 22.0;
    let play = Rect::new(area.x + area.w / 2.0 - 34.0, row, 68.0, 68.0);
    let looping = Rect::new(area.x, row + 14.0, 96.0, 40.0);
    let sound = Rect::new(area.right() - 96.0, row + 14.0, 96.0, 40.0);
    Layout { picture, bar, play, looping, sound }
}

struct Page {
    video: Option<u64>,
    problem: String,
    looping: bool,
    muted: bool,
    /// The finger on the scrubber, as a share of the bar, while dragging.
    dragging: Option<f32>,
    /// The video's state as the last frame drew it, for the touches that
    /// arrive between frames.
    last: Option<video::VideoState>,
}

fn pill(canvas: u64, at: Rect, label: &str, on: bool) -> Result<(), gfx::GfxError> {
    let radii = gfx::CornerRadii { top_left: 20.0, top_right: 20.0, bottom_right: 20.0, bottom_left: 20.0 };
    let (fill, ink) = if on { (rgb(0.07, 0.07, 0.09), rgb(1.0, 1.0, 1.0)) } else { (rgb(0.90, 0.90, 0.92), rgb(0.07, 0.07, 0.09)) };
    canvas2d::fill_round_rect(canvas, r(at), radii, fill)?;
    let m = canvas2d::measure_text_styled(canvas, label, 15.0, style(600))?;
    canvas2d::draw_text_styled(canvas, label, gfx::Point { x: at.x + (at.w - m.width) / 2.0, y: at.y + 26.0 }, 15.0, ink, style(600))
}

fn draw(win: u64, canvas: u64, screen: &Screen, page: &Page) -> Result<(), gfx::GfxError> {
    let ink = rgb(0.07, 0.07, 0.09);
    let muted = rgb(0.43, 0.44, 0.48);
    canvas2d::clear(canvas, rgb(0.96, 0.96, 0.97))?;
    let area = screen.safe_area().inset(16.0, 16.0, 16.0, 16.0);
    canvas2d::draw_text_styled(canvas, "Clip", gfx::Point { x: area.x, y: area.y + 32.0 }, 30.0, ink, style(800))?;
    let Some(v) = page.video else {
        canvas2d::draw_text_styled(canvas, &page.problem, gfx::Point { x: area.x, y: area.y + 64.0 }, 15.0, muted, style(450))?;
        return canvas2d::present(canvas);
    };
    let state = video::state(v)?;
    let at = layout(screen, &state);
    let radii = gfx::CornerRadii { top_left: 14.0, top_right: 14.0, bottom_right: 14.0, bottom_left: 14.0 };
    canvas2d::fill_round_rect(canvas, r(at.picture), radii, rgb(0.0, 0.0, 0.0))?;
    video::draw_round(canvas, v, r(at.picture), radii)?;
    if !state.problem.is_empty() {
        canvas2d::draw_text_styled(canvas, &state.problem, gfx::Point { x: at.picture.x + 16.0, y: at.picture.y + 30.0 }, 15.0, rgb(1.0, 0.6, 0.6), style(500))?;
    } else if !state.ready {
        canvas2d::draw_text_styled(canvas, "Loading", gfx::Point { x: at.picture.x + 16.0, y: at.picture.y + 30.0 }, 15.0, rgb(1.0, 1.0, 1.0), style(500))?;
    }

    // The scrubber: a track, how far it has played, and a knob.
    let share = match page.dragging {
        Some(share) => share,
        None if state.duration_millis > 0 => state.position_millis as f32 / state.duration_millis as f32,
        None => 0.0,
    }
    .clamp(0.0, 1.0);
    let track = Rect::new(at.bar.x, at.bar.y + 11.0, at.bar.w, 6.0);
    let round = gfx::CornerRadii { top_left: 3.0, top_right: 3.0, bottom_right: 3.0, bottom_left: 3.0 };
    canvas2d::fill_round_rect(canvas, r(track), round, rgb(0.85, 0.86, 0.89))?;
    canvas2d::fill_round_rect(canvas, r(Rect::new(track.x, track.y, track.w * share, track.h)), round, ink)?;
    canvas2d::fill_circle(canvas, gfx::Point { x: track.x + track.w * share, y: track.y + 3.0 }, 10.0, ink)?;
    let shown = if page.dragging.is_some() { (share * state.duration_millis as f32) as u64 } else { state.position_millis };
    canvas2d::draw_text_styled(canvas, &clock_text(shown), gfx::Point { x: at.bar.x, y: at.bar.bottom() + 6.0 }, 13.0, muted, style(500))?;
    let total = clock_text(state.duration_millis);
    let tm = canvas2d::measure_text_styled(canvas, &total, 13.0, style(500))?;
    canvas2d::draw_text_styled(canvas, &total, gfx::Point { x: at.bar.right() - tm.width, y: at.bar.bottom() + 6.0 }, 13.0, muted, style(500))?;

    // Play or pause, drawn as the familiar shapes.
    let c = gfx::Point { x: at.play.x + at.play.w / 2.0, y: at.play.y + at.play.h / 2.0 };
    canvas2d::fill_circle(canvas, c, at.play.w / 2.0, ink)?;
    let white = rgb(1.0, 1.0, 1.0);
    if state.playing {
        canvas2d::fill_rect(canvas, gfx::Rect { x: c.x - 10.0, y: c.y - 12.0, width: 7.0, height: 24.0 }, white)?;
        canvas2d::fill_rect(canvas, gfx::Rect { x: c.x + 3.0, y: c.y - 12.0, width: 7.0, height: 24.0 }, white)?;
    } else {
        // A triangle from thin strips: the canvas has no paths.
        for i in 0..22 {
            let x = c.x - 8.0 + i as f32;
            let half = 13.0 * (1.0 - i as f32 / 22.0);
            canvas2d::fill_rect(canvas, gfx::Rect { x, y: c.y - half, width: 1.2, height: half * 2.0 }, white)?;
        }
    }
    pill(canvas, at.looping, "Loop", page.looping)?;
    pill(canvas, at.sound, if page.muted { "Muted" } else { "Sound" }, !page.muted)?;
    canvas2d::present(canvas)?;
    // What a screen reader says, and what a finger may drag on a phone: the
    // scrubber is adjustable, so moving a finger on it seeks, not scrolls.
    let mut said = Said::new();
    said.heading(Rect::new(area.x, area.y, area.w, 40.0), "Clip");
    said.image(at.picture, "The video");
    said.adjustable(at.bar, "Position", &format!("{} of {}", clock_text(shown), total));
    said.button(at.play, if state.playing { "Pause" } else { "Play" });
    said.toggle(at.looping, "Loop", page.looping);
    said.toggle(at.sound, "Sound", !page.muted);
    said.send(win);
    Ok(())
}

fn quick(canvas: u64, source: &str) -> i32 {
    let v = match video::open(source) {
        Ok(v) => v,
        Err(e) => {
            say(&format!("video:problem={e:?}"));
            return 0;
        }
    };
    let _ = video::set_muted(v, true);
    // K-1351: a volume is taken, and one that is not a number is refused.
    let volume = video::set_volume(v, 0.5).is_ok() && video::set_volume(v, f32::NAN).is_err();
    // K-1358: it floats in a window of its own while it plays.
    let floated = video::start_floating(v);
    let _ = video::play(v);
    let mut frames = 0u32;
    let mut first = None;
    // (position, clock) pairs: the rate is judged against real time, not
    // the loop's turns, which run long on a busy machine.
    let mut fast: Option<(u64, u64)> = None;
    let mut rate_ok = false;
    let mut fast_last: Option<(u64, u64)> = None;
    let mut last = video::state(v);
    // About three seconds, drawing as an app would. Nothing is presented
    // until the video plays, so `--shoot` captures a real frame of it.
    for _ in 0..90 {
        let playing = matches!(&last, Ok(s) if s.ready && s.position_millis > 300);
        let _ = canvas2d::clear(canvas, rgb(0.0, 0.0, 0.0));
        if video::draw(canvas, v, gfx::Rect { x: 0.0, y: 0.0, width: 420.0, height: 236.0 }).is_ok() {
            frames += 1;
        }
        if playing {
            let _ = canvas2d::present(canvas);
        }
        last = video::state(v);
        if let Ok(s) = &last {
            if s.ready && first.is_none() {
                first = Some(s.position_millis);
            }
            // K-1357: from the first second on, twice as fast.
            if s.position_millis >= 1000 && fast.is_none() {
                rate_ok = video::set_rate(v, 2.0).is_ok() && video::set_rate(v, f32::NAN).is_err();
                fast = Some((s.position_millis, clock::monotonic_nanos()));
            }
            // The last moment it was still playing: a short clip ends at
            // 2x, and the clock stands still after that.
            if fast.is_some() && s.playing && !s.ended {
                fast_last = Some((s.position_millis, clock::monotonic_nanos()));
            }
        }
        let _ = events::wait(Some(33));
    }
    // ... and comes back.
    let float = match floated {
        Ok(()) if video::floating(v) => {
            video::stop_floating(v);
            if video::floating(v) { "stuck" } else { "ok" }
        }
        Ok(()) => "no",
        Err(gfx::GfxError::Unsupported(_)) => "unsupported",
        Err(_) => "error",
    };
    match last {
        Ok(s) if !s.problem.is_empty() => say(&format!("video:problem={}", s.problem)),
        Ok(s) => say(&format!(
            "video:ready={} duration={} size={}x{} moved={} frames={frames} volume={} rate={} float={float}",
            if s.ready { "yes" } else { "no" },
            s.duration_millis,
            s.width,
            s.height,
            if first.is_some_and(|f| s.position_millis > f + 1000) { "yes" } else { "no" },
            if volume { "ok" } else { "no" },
            // At 2x the video's clock moves about twice as far as real
            // time; anything over 1.5x is the rate working.
            match (fast, fast_last) {
                (Some((at, from)), Some((to, until))) if rate_ok && until > from + 150_000_000 => {
                    let real = (until - from) / 1_000_000;
                    let moved = to.saturating_sub(at);
                    if moved * 2 > real * 3 { "ok" } else { "slow" }
                }
                _ => "no",
            },
        )),
        Err(e) => say(&format!("video:problem={e:?}")),
    }
    video::close(v);
    0
}

struct Component;

impl Guest for Component {
    fn run() -> i32 {
        let Ok(win) = window::create("Clip", types::WindowSize { width: 420, height: 720 }) else {
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
        let raw = args::raw();
        if raw.split('\n').any(|a| a == "quick") {
            // `quick https://...` plays a stream instead of the bundled clip.
            let source = raw.split('\n').find(|a| a.starts_with("https://")).unwrap_or(CLIP);
            return quick(canvas, source);
        }
        let mut page = Page { video: None, problem: String::new(), looping: false, muted: false, dragging: None, last: None };
        match video::open(CLIP) {
            Ok(v) => {
                let _ = video::play(v);
                page.video = Some(v);
            }
            Err(e) => page.problem = format!("This video cannot play here: {e:?}"),
        }
        let mut frames = 0u32;
        loop {
            if let Some(v) = page.video {
                page.last = video::state(v).ok();
            }
            // While a finger holds the scrubber, follow it between events,
            // and every sixth frame (about a tenth of a second) show the
            // picture there too.
            if page.dragging.is_some() {
                if let (Some(state), Some((x, _))) = (page.last.as_ref(), pointer::position(win)) {
                    let at = layout(&Screen::of(win), state);
                    let share = ((x - at.bar.x) / at.bar.w).clamp(0.0, 1.0);
                    page.dragging = Some(share);
                    frames += 1;
                    if frames % 6 == 0 {
                        if let Some(v) = page.video {
                            let _ = video::seek(v, (share * state.duration_millis as f32) as u64);
                        }
                    }
                }
            }
            if draw(win, canvas, &Screen::of(win), &page).is_err() {
                return 1;
            }
            let mut next = events::wait(Some(16));
            while let Some(event) = next {
                match event {
                    Event::CloseRequested(_) => return 0,
                    Event::Pointer(p) => {
                        if let (Some(v), Some(state)) = (page.video, page.last.as_ref()) {
                            let state = state.clone();
                            let at = layout(&Screen::of(win), &state);
                            let share = ((p.x - at.bar.x) / at.bar.w).clamp(0.0, 1.0);
                            let on_bar = at.bar.inset(-14.0, 0.0, -14.0, 0.0).contains(p.x, p.y);
                            if p.button.is_some() && p.pressed && on_bar {
                                page.dragging = Some(share);
                            } else if page.dragging.is_some() && p.button.is_some() && !p.pressed {
                                page.dragging = None;
                                let _ = video::seek(v, (share * state.duration_millis as f32) as u64);
                            } else if p.button.is_some() && !p.pressed {
                                if at.play.contains(p.x, p.y) {
                                    let _ = if state.playing { video::pause(v) } else { video::play(v) };
                                } else if at.looping.contains(p.x, p.y) {
                                    page.looping = !page.looping;
                                    let _ = video::set_looping(v, page.looping);
                                } else if at.sound.contains(p.x, p.y) {
                                    page.muted = !page.muted;
                                    let _ = video::set_muted(v, page.muted);
                                } else if at.picture.contains(p.x, p.y) {
                                    let _ = if state.playing { video::pause(v) } else { video::play(v) };
                                }
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
