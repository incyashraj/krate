//! Krate contacts -- a real SQL database behind a modern contact list.
//!
//! The wall it tests: key-value storage is enough for a counter, but a real
//! app has structured data it queries -- rows, columns, ordering, a WHERE.
//! This app keeps contacts in an actual SQLite table through `store.sql`:
//! it creates the table, seeds a few people the first time it runs, then queries
//! them back sorted by name and shows them. If SQL is faked or the query path
//! is broken, the list comes back empty and the header says so.
//!
//! The presentation is drawn entirely on one `gfx.canvas2d`: a considered dark
//! ground, a bold title with a live count, one rounded card per contact -- a
//! colored avatar disc with the initial, the name in bright ink, the email in
//! a muted tone -- and a detail view for the one picked. It is laid out from
//! the canvas it really has, every frame: a desktop window or a tablet shows
//! the list and the detail side by side, a phone one at a time with a way
//! back. Text follows the person's text size, and each frame tells the screen
//! reader what it shows.
//!
//! This app must be `#![no_std]`, and that is the whole reason it is a probe.
//! `store.sql`'s `query` returns a nested `list<row<list<value>>>`, and the
//! generated glue that lifts it uses `Vec::with_capacity`. In a `std`-linked
//! guest that call reaches std's allocation-error handler, which routes through
//! std's panic runtime and drags the entire `wasi:*` import set into an
//! otherwise pure component -- so the app fails to instantiate against the Krate
//! linker. Building `#![no_std]` lets the SDK own the allocator and a trapping
//! panic handler, so the same allocation path traps instead of leaking.
#![no_std]
extern crate alloc;

// The SDK crate supplies the `no_std` runtime -- allocator, trapping panic
// handler, mem intrinsics -- and, with its `gui` feature, the window API.
use krate::bindings::krate::io::{args, stdio};
use krate::bindings::krate::store::sql;
use krate::gfx::{canvas2d, types as gfx};
use krate::layout::{Look, Rect, Said, Screen};
use krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

/// The window asked for on a desktop: wide enough for list and detail side
/// by side. Only a first size -- every frame lays out from the real canvas.
const WIDTH: f32 = 860.0;
const HEIGHT: f32 = 560.0;
/// Below this width the list and the detail cannot both be read, so they are
/// shown one at a time, the way a phone's own contacts app does it.
const SPLIT_MIN: f32 = 640.0;

/// Up to this many contacts held for drawing. The seed is four; a real table
/// could hold more, and the fixed pool keeps the loop panic-free.
const MAX_CONTACTS: usize = 64;
const NAME_CAP: usize = 48;
const EMAIL_CAP: usize = 64;

const QUICK_ROUNDS: u32 = 4;
const ROUND_MILLIS: u32 = 50;

/// The people seeded on first run, so a fresh open shows a real table.
const SEED: [(&str, &str); 4] = [
    ("Ada Lovelace", "ada@analytical.engine"),
    ("Alan Turing", "alan@bombe.uk"),
    ("Grace Hopper", "grace@cobol.navy"),
    ("Katherine Johnson", "katherine@nasa.gov"),
];

/// A palette of avatar tints, chosen by the first letter so each person keeps a
/// stable, distinct color. Warm-to-cool spread, all readable on the dark ground.
const AVATARS: [(f32, f32, f32); 8] = [
    (0.36, 0.72, 1.0),  // sky blue
    (0.98, 0.45, 0.62), // rose
    (0.55, 0.85, 0.55), // green
    (0.98, 0.72, 0.35), // amber
    (0.72, 0.55, 0.98), // violet
    (0.40, 0.85, 0.82), // teal
    (0.98, 0.58, 0.40), // coral
    (0.62, 0.70, 0.85), // slate
];

// The app is dark by design, in a light room as in a dark one.
const INK: gfx::Color = color(0.96, 0.97, 1.0, 1.0);
const MUTED: gfx::Color = color(0.55, 0.62, 0.80, 1.0);
const QUIET: gfx::Color = color(0.60, 0.68, 0.85, 1.0);
const ACCENT: gfx::Color = color(0.36, 0.72, 1.0, 1.0);
const CARD: gfx::Color = color(0.117, 0.133, 0.196, 1.0);
const CARD_PICKED: gfx::Color = color(0.15, 0.20, 0.32, 1.0);
const HAIRLINE: gfx::Color = color(1.0, 1.0, 1.0, 0.07);

/// One contact held for drawing: fixed-capacity byte buffers, no growing Vec.
#[derive(Clone, Copy)]
struct Contact {
    id: i64,
    name: [u8; NAME_CAP],
    name_len: usize,
    email: [u8; EMAIL_CAP],
    email_len: usize,
}

const ZERO_CONTACT: Contact = Contact {
    id: 0,
    name: [0u8; NAME_CAP],
    name_len: 0,
    email: [0u8; EMAIL_CAP],
    email_len: 0,
};

impl Contact {
    fn name(&self) -> &str {
        text_of(self.name.get(..self.name_len).unwrap_or(&[]))
    }

    fn email(&self) -> &str {
        text_of(self.email.get(..self.email_len).unwrap_or(&[]))
    }
}

/// What the person is looking at, and where the last frame drew what can be
/// tapped. Taps are tested against these rects, so the drawing and the hit
/// test can never disagree.
struct View {
    /// The contact picked. On a phone `Some` means the detail is showing.
    picked: Option<usize>,
    scroll: f32,
    max_scroll: f32,
    rows: [Option<Rect>; MAX_CONTACTS],
    back: Option<Rect>,
}

impl View {
    /// A tap (or a click) at `x`, `y`. Returns whether the screen changed.
    fn tap(&mut self, x: f32, y: f32) -> bool {
        if self.back.is_some_and(|r| r.contains(x, y)) {
            self.picked = None;
            return true;
        }
        let hit = self.rows.iter().position(|r| r.is_some_and(|r| r.contains(x, y)));
        if hit.is_some() && hit != self.picked {
            self.picked = hit;
            return true;
        }
        false
    }

    /// The wheel, or a finger dragging the list. Clamped to what was drawn.
    fn wheel(&mut self, dy: f32) -> bool {
        let next = (self.scroll + dy).clamp(0.0, self.max_scroll);
        let moved = next != self.scroll;
        self.scroll = next;
        moved
    }
}

struct Component;

impl krate::Guest for Component {
    fn run() -> i32 {
        let out = stdio::stdout();

        // Create the table if it is not there yet. execute returns the rows
        // affected; a schema statement affects none, which is fine.
        if sql::execute(
            "CREATE TABLE IF NOT EXISTS contacts (id INTEGER PRIMARY KEY, name TEXT NOT NULL, email TEXT NOT NULL)",
            &[],
        )
        .is_err()
        {
            let _ = out.write(b"sql:create-failed\n");
            return 40;
        }

        // Seed once: only insert when the table is empty, so reopening does not
        // pile up duplicates. A COUNT(*) query proves the read path too.
        let count = match sql::query("SELECT COUNT(*) FROM contacts", &[]) {
            Ok(result) => first_integer(&result),
            Err(_) => {
                let _ = out.write(b"sql:count-failed\n");
                return 41;
            }
        };
        if count == 0 {
            for &(name, email) in SEED.iter() {
                if sql::execute(
                    "INSERT INTO contacts (name, email) VALUES (?, ?)",
                    &[
                        sql::Value::Text(pure_string(name)),
                        sql::Value::Text(pure_string(email)),
                    ],
                )
                .is_err()
                {
                    let _ = out.write(b"sql:insert-failed\n");
                    return 42;
                }
            }
        }

        // Query them back, sorted. This is the read path the whole probe rides.
        // The id comes too: it is the row's key, which the detail view shows.
        let result = match sql::query("SELECT id, name, email FROM contacts ORDER BY name", &[]) {
            Ok(result) => result,
            Err(_) => {
                let _ = out.write(b"sql:query-failed\n");
                return 43;
            }
        };

        // Copy the queried rows into fixed byte buffers for drawing. Done once,
        // up front, so the render loop never touches the SQL result again.
        let mut contacts = [ZERO_CONTACT; MAX_CONTACTS];
        let mut n = 0usize;
        for (row, slot) in result.rows.iter().zip(contacts.iter_mut()) {
            if let Some(sql::Value::Integer(id)) = row.values.first() {
                slot.id = *id;
            }
            if let Some(sql::Value::Text(name)) = row.values.get(1) {
                slot.name_len = copy_into(&mut slot.name, name.as_bytes());
            }
            if let Some(sql::Value::Text(email)) = row.values.get(2) {
                slot.email_len = copy_into(&mut slot.email, email.as_bytes());
            }
            n += 1;
        }
        let row_count = result.rows.len();
        let people = contacts.get(..n).unwrap_or(&[]);

        // Report the count on stdout so a script can assert the round trip.
        let _ = out.write(b"contacts:");
        let _ = out.write(u64_slice(row_count as u64, &mut [0u8; 20]));
        let _ = out.write(b"\n");

        // ---- window + canvas ----
        let size = types::WindowSize { width: WIDTH as u32, height: HEIGHT as u32 };
        let Ok(win) = window::create("Contacts", size) else {
            return 30;
        };
        if window::show(win).is_err() {
            return 31;
        }
        if tree::set_root(win, &stack_root()).is_err()
            || tree::upsert_node(win, &canvas_node()).is_err()
        {
            let _ = window::close(win);
            return 32;
        }
        let canvas = match canvas2d::bind(win, CANVAS_ID) {
            Ok(c) => c,
            Err(_) => {
                let _ = window::close(win);
                return 33;
            }
        };

        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|byte| *byte == b'\n')
            .next()
            .is_some_and(|first| first == b"quick");

        let mut view = View {
            picked: None,
            scroll: 0.0,
            max_scroll: 0.0,
            rows: [None; MAX_CONTACTS],
            back: None,
        };
        let _ = draw(canvas, win, people, row_count, &mut view);

        // A real session ends when the person closes the window, never
        // on a round count: 600 rounds x 50 ms quietly shut the window
        // after thirty seconds of use (K-092). `quick` keeps its bound
        // so a headless check can never hang.
        let rounds = if quick { QUICK_ROUNDS } else { u32::MAX };
        'session: for _ in 0..rounds {
            // Take everything already queued, then draw once: a finger
            // dragging the list sends far more events than there are frames.
            let mut dirty = false;
            let mut next = events::wait(Some(ROUND_MILLIS));
            while let Some(event) = next {
                match event {
                    types::Event::CloseRequested(id) if id == win => break 'session,
                    // A new size is a new layout; a new theme, new text size.
                    types::Event::Resized(_)
                    | types::Event::ThemeChanged(_)
                    | types::Event::RedrawRequested(_) => dirty = true,
                    // On the release, so a finger that drags to scroll does
                    // not pick the row it started on.
                    types::Event::Pointer(p) if p.button.is_some() && !p.pressed => {
                        dirty |= view.tap(p.x, p.y);
                    }
                    types::Event::Wheel(w) => dirty |= view.wheel(w.dy),
                    _ => {}
                }
                next = events::poll();
            }
            if dirty {
                let _ = draw(canvas, win, people, row_count, &mut view);
            }
        }

        let _ = window::close(win);
        0
    }
}

// ------------------------------------------------------------------
// Rendering
// ------------------------------------------------------------------

/// One frame, laid out from the canvas the app really has: list and detail
/// side by side when wide, one at a time when narrow. Every font size goes
/// through the person's text size, and so does every row height.
fn draw(
    canvas: u64,
    win: u64,
    people: &[Contact],
    total: usize,
    view: &mut View,
) -> Result<(), gfx::GfxError> {
    let size = canvas2d::canvas_size(canvas).unwrap_or(gfx::Size {
        width: WIDTH,
        height: HEIGHT,
    });
    let (w, h) = (size.width.max(1.0), size.height.max(1.0));
    let look = Look::now();
    let mut said = Said::new();
    view.rows = [None; MAX_CONTACTS];
    view.back = None;

    let narrow = w < SPLIT_MIN;
    // A phone turned sideways has little height: a smaller title.
    let short = h < 500.0;
    let margin = if narrow { 16.0 } else { 28.0 };
    // Text and controls stay clear of a notch, the home bar and the close
    // button Krate draws on a phone; the ground still fills the whole canvas.
    let (top, right, bottom, left) = Screen::of(win).safe;
    let mut area = Rect::new(0.0, 0.0, w, h).inset(top, right, bottom, left).pad(margin);

    // Deep, considered ground: one native vertical gradient from a faint-blue
    // top to a darker floor, and a soft glow up top-left for depth.
    canvas2d::linear_gradient(
        canvas,
        gfx::Rect { x: 0.0, y: 0.0, width: w, height: h },
        color(0.078, 0.090, 0.145, 1.0),
        color(0.043, 0.051, 0.086, 1.0),
    )?;
    canvas2d::radial_gradient(
        canvas,
        gfx::Point { x: 90.0, y: 40.0 },
        260.0,
        color(0.30, 0.45, 0.90, 0.16),
        color(0.30, 0.45, 0.90, 0.0),
    )?;

    // On a phone with a contact picked, the detail takes the whole screen
    // and the header becomes the way back to the list.
    let picked = view.picked.and_then(|i| people.get(i));
    if let (true, Some(c)) = (narrow, picked) {
        let label_size = look.text(17.0);
        let bar = area.take_top((label_size * 2.4).max(44.0));
        let label = "‹ Contacts";
        let back = Rect::new(bar.x, bar.y, width_of(canvas, label, label_size) + 16.0, bar.h);
        draw_text(canvas, label, back.x, back.y + (back.h + label_size * 0.7) / 2.0, label_size, ACCENT)?;
        said.button(back, "Back to contacts");
        view.back = Some(back);
        area.take_top(8.0);
        draw_detail(canvas, area, c, &look, &mut said)?;
        canvas2d::present(canvas)?;
        said.send(win);
        return Ok(());
    }

    // ---- header ----
    // A phone turned sideways has the height for one line: the count goes
    // beside the title instead of under it.
    let title_size = look.text(if short { 24.0 } else if narrow { 28.0 } else { 34.0 });
    let sub_size = look.text(15.0);
    let head_h = if short { title_size * 1.2 + 16.0 } else { title_size + sub_size * 2.2 + 18.0 };
    let head = area.take_top(head_h);
    let title_y = head.y + title_size * 0.9;
    let title_w = width_of(canvas, "Contacts", title_size).min(head.w);
    fit(canvas, "Contacts", head.x, title_y, title_size, INK, head.w)?;
    said.heading(Rect::new(head.x, head.y, title_w, title_size * 1.1), "Contacts");

    // Subtitle with a live count and an accent dot.
    let mut buf = [0u8; 40];
    let sub = text_of(count_bytes(total as u64, &mut buf));
    let (sub_x, sub_y) = if short { (head.x + title_w + 20.0, title_y) } else { (head.x, title_y + sub_size * 1.7) };
    disc(canvas, sub_x + 6.0, sub_y - sub_size * 0.35, 3.0, ACCENT)?;
    let sub_room = head.right() - sub_x - 18.0;
    fit(canvas, sub, sub_x + 18.0, sub_y, sub_size, QUIET, sub_room)?;
    said.text(Rect::new(sub_x, sub_y - sub_size, sub_room + 18.0, sub_size * 1.4), sub);

    // A hairline divider under the header.
    fill(canvas, Rect::new(head.x, head.bottom() - 10.0, head.w, 1.0), HAIRLINE)?;

    if people.is_empty() {
        // Empty state: honest, styled, not a blank pane.
        let size = look.text(16.0);
        let msg = "No contacts came back from SQLite.";
        fit(canvas, msg, area.x, area.y + size * 1.5, size, color(0.75, 0.55, 0.55, 1.0), area.w)?;
        said.text(Rect::new(area.x, area.y, area.w, size * 2.0), msg);
    } else if narrow {
        draw_list(canvas, area, people, None, &look, view, &mut said)?;
    } else {
        // Side by side: the list keeps a readable width that grows with the
        // text, and the detail takes the rest. Wide, something is always
        // picked, so the right side is never an empty pane.
        let list_w = (300.0 * look.text_scale.clamp(1.0, 1.5)).min(area.w * 0.45);
        let list = area.take_left(list_w);
        area.take_left(20.0);
        let shown = view.picked.filter(|i| *i < people.len()).unwrap_or(0);
        draw_list(canvas, list, people, Some(shown), &look, view, &mut said)?;
        if let Some(c) = people.get(shown) {
            draw_detail(canvas, area, c, &look, &mut said)?;
        }
    }

    canvas2d::present(canvas)?;
    said.send(win);
    Ok(())
}

/// The list: one card per contact, scrolled by `view.scroll` and clipped to
/// `area`. Records each card's rect in `view.rows` for the next tap.
fn draw_list(
    canvas: u64,
    area: Rect,
    people: &[Contact],
    highlight: Option<usize>,
    look: &Look,
    view: &mut View,
    said: &mut Said,
) -> Result<(), gfx::GfxError> {
    let (name_size, email_size) = (look.text(18.0), look.text(14.0));
    // 68 points at the default size, taller as the text grows.
    let row_h = (name_size + email_size) * 1.25 + 28.0;
    let gap = 12.0;
    let content = people.len() as f32 * (row_h + gap) - gap;
    view.max_scroll = (content - area.h).max(0.0);
    view.scroll = view.scroll.clamp(0.0, view.max_scroll);

    canvas2d::set_clip(canvas, area.x, area.y, area.w, area.h)?;
    for (i, c) in people.iter().enumerate() {
        let y = area.y + i as f32 * (row_h + gap) - view.scroll;
        // Cull cards fully outside the list.
        if y + row_h < area.y || y > area.bottom() {
            continue;
        }
        let card = Rect::new(area.x, y, area.w, row_h);
        draw_card(canvas, c, card, highlight == Some(i), name_size, email_size, look)?;
        // Only the part on screen can be tapped.
        let top = y.max(area.y);
        let seen = Rect::new(card.x, top, card.w, (y + row_h).min(area.bottom()) - top);
        if let Some(slot) = view.rows.get_mut(i) {
            *slot = Some(seen);
        }
        said.button(seen, c.name());
    }
    canvas2d::clear_clip(canvas)
}

fn draw_card(
    canvas: u64,
    c: &Contact,
    r: Rect,
    picked: bool,
    name_size: f32,
    email_size: f32,
    look: &Look,
) -> Result<(), gfx::GfxError> {
    // The card: a rounded panel a touch lighter than the ground, with a very
    // soft shadow beneath so it lifts off the background. The picked one is
    // tinted and edged in the accent, so it reads as the one on the right.
    round_rect(canvas, Rect { y: r.y + 3.0, ..r }, 16.0, color(0.0, 0.0, 0.0, 0.22))?;
    round_rect(canvas, r, 16.0, if picked { CARD_PICKED } else { CARD })?;
    if picked {
        canvas2d::stroke_round_rect(canvas, gfx_rect(r), radii(16.0), 1.5, ACCENT)?;
    }

    // Avatar disc: grows with the text, but less, so the name keeps the room.
    let radius = 20.0 * look.text_scale.clamp(1.0, 1.6);
    let (ax, ay) = (r.x + 18.0 + radius, r.y + r.h * 0.5);
    avatar(canvas, c, ax, ay, radius)?;

    // Name in bright ink, email in a muted tone, centred as one block. Both
    // are cut with an ellipsis rather than running off the card.
    let text_x = ax + radius + 16.0;
    let room = r.right() - 16.0 - text_x;
    let block = name_size + email_size * 1.65;
    let name_y = r.y + (r.h - block) / 2.0 + name_size;
    fit(canvas, c.name(), text_x, name_y, name_size, INK, room)?;
    fit(canvas, c.email(), text_x, name_y + email_size * 1.4, email_size, MUTED, room)
}

/// The detail: a big avatar, the name and every field of the record. The
/// avatar sits beside the text in a wide pane and above it in a tall one.
fn draw_detail(canvas: u64, area: Rect, c: &Contact, look: &Look, said: &mut Said) -> Result<(), gfx::GfxError> {
    let pad = if area.w < 400.0 || area.h < 260.0 { 20.0 } else { 32.0 };
    let radius = 36.0 * look.text_scale.clamp(1.0, 1.6);
    let beside = area.w > area.h;
    let name_size = look.text(28.0);
    let (label_size, value_size) = (look.text(11.0), look.text(16.0));

    // The card hugs what it holds, so a tall pane is not one big empty
    // panel; it is never taller than the pane.
    let text_h = name_size * 1.2 + 14.0 + 2.0 * (label_size * 1.2 + value_size * 2.5);
    let content = if beside { text_h.max(2.0 * radius) } else { 2.0 * radius + 18.0 + text_h };
    let card = Rect { h: (content + 2.0 * pad).min(area.h), ..area };
    round_rect(canvas, card, 20.0, CARD)?;
    let inner = card.pad(pad);
    let (ax, ay) = (inner.x + radius, inner.y + radius);
    avatar(canvas, c, ax, ay, radius)?;

    let (x, mut y) = if beside {
        (ax + radius + 24.0, inner.y)
    } else {
        (inner.x, ay + radius + 18.0)
    };
    let room = inner.right() - x;
    fit(canvas, c.name(), x, y + name_size * 0.9, name_size, INK, room)?;
    said.heading(Rect::new(x, y, room, name_size * 1.2), c.name());
    y += name_size * 1.2 + 14.0;

    // The record's fields, one labelled line each. Only whole fields are
    // drawn: one cut in half at the bottom reads as a broken card.
    let mut key = [0u8; 48];
    let fields = [("EMAIL", c.email()), ("STORED AS", row_label(c.id, &mut key))];
    for (label, value) in fields {
        let value_y = y + label_size * 1.2 + value_size * 1.3;
        if value_y + value_size * 0.3 > inner.bottom() {
            break;
        }
        draw_text(canvas, label, x, y + label_size * 1.2, label_size, QUIET)?;
        fit(canvas, value, x, value_y, value_size, INK, room)?;
        said.text(Rect::new(x, y, room, value_y - y + value_size * 0.3), value);
        y = value_y + value_size * 1.2;
    }
    Ok(())
}

/// A colored disc with the person's initial centred on it, and a soft halo.
/// The color is chosen by the initial so each person keeps theirs.
fn avatar(canvas: u64, c: &Contact, x: f32, y: f32, radius: f32) -> Result<(), gfx::GfxError> {
    let initial = first_initial(c.name().as_bytes());
    let (ar, ag, ab) = avatar_color(initial);
    canvas2d::radial_gradient(
        canvas,
        gfx::Point { x, y },
        radius * 1.5,
        color(ar, ag, ab, 0.35),
        color(ar, ag, ab, 0.0),
    )?;
    disc(canvas, x, y, radius, color(ar, ag, ab, 1.0))?;
    let glyph = [initial];
    let s = text_of(&glyph);
    let gx = x - width_of(canvas, s, radius) / 2.0;
    draw_text(canvas, s, gx, y + radius * 0.36, radius, color(0.08, 0.09, 0.14, 1.0))
}

// ------------------------------------------------------------------
// Canvas helpers
// ------------------------------------------------------------------

/// Draw `s` at most `room` wide: the longest leading part that fits, then an
/// ellipsis. A name cut short reads as "there is more"; one running off the
/// card reads as a broken app.
fn fit(canvas: u64, s: &str, x: f32, y: f32, size: f32, c: gfx::Color, room: f32) -> Result<(), gfx::GfxError> {
    if room <= 0.0 {
        return Ok(());
    }
    if width_of(canvas, s, size) <= room {
        return draw_text(canvas, s, x, y, size, c);
    }
    let ell = width_of(canvas, "…", size);
    let (mut lo, mut hi) = (0usize, s.len());
    while lo < hi {
        // Halve on a character boundary: the text came from a database
        // and need not be ASCII.
        let mut mid = (lo + hi).div_ceil(2);
        while mid > lo && !s.is_char_boundary(mid) {
            mid -= 1;
        }
        if mid == lo {
            break;
        }
        if width_of(canvas, s.get(..mid).unwrap_or(""), size) + ell <= room {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let part = s.get(..lo).unwrap_or("").trim_end();
    draw_text(canvas, part, x, y, size, c)?;
    draw_text(canvas, "…", x + width_of(canvas, part, size), y, size, c)
}

fn width_of(canvas: u64, s: &str, size: f32) -> f32 {
    canvas2d::measure_text(canvas, s, size).map_or(0.0, |m| m.width)
}

/// A filled rounded rectangle, antialiased on the curve in one host call.
fn round_rect(canvas: u64, r: Rect, radius: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::fill_round_rect(canvas, gfx_rect(r), radii(radius), c)
}

fn fill(canvas: u64, r: Rect, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::fill_rect(canvas, gfx_rect(r), c)
}

fn gfx_rect(r: Rect) -> gfx::Rect {
    gfx::Rect { x: r.x, y: r.y, width: r.w, height: r.h }
}

fn radii(r: f32) -> gfx::CornerRadii {
    gfx::CornerRadii { top_left: r, top_right: r, bottom_right: r, bottom_left: r }
}

fn disc(canvas: u64, cx: f32, cy: f32, r: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::fill_circle(canvas, gfx::Point { x: cx, y: cy }, r, c)
}

fn draw_text(canvas: u64, text: &str, x: f32, y: f32, size: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::draw_text(canvas, text, gfx::Point { x, y }, size, c)
}

const fn color(r: f32, g: f32, b: f32, a: f32) -> gfx::Color {
    gfx::Color { r, g, b, a }
}

// ------------------------------------------------------------------
// Data helpers
// ------------------------------------------------------------------

/// The first column of the first row as an integer, for COUNT(*).
fn first_integer(result: &sql::QueryResult) -> i64 {
    match result.rows.first().and_then(|row| row.values.first()) {
        Some(sql::Value::Integer(n)) => *n,
        _ => 0,
    }
}

/// Bytes as text; a buffer cut mid-character shows as nothing rather than
/// panicking.
fn text_of(bytes: &[u8]) -> &str {
    core::str::from_utf8(bytes).unwrap_or("")
}

/// Copy `src` into `dst`, up to `dst.len()`, returning bytes written.
fn copy_into(dst: &mut [u8], src: &[u8]) -> usize {
    let mut i = 0usize;
    while i < src.len() && i < dst.len() {
        if let (Some(d), Some(s)) = (dst.get_mut(i), src.get(i)) {
            *d = *s;
        }
        i += 1;
    }
    i
}

/// The first ASCII letter of a name uppercased, or '?' if none.
fn first_initial(name: &[u8]) -> u8 {
    match name.first() {
        Some(&b) if b.is_ascii_lowercase() => b - 32,
        Some(&b) if b.is_ascii_uppercase() => b,
        Some(&b) if b.is_ascii_alphanumeric() => b,
        _ => b'?',
    }
}

/// A stable avatar tint chosen from the initial.
fn avatar_color(initial: u8) -> (f32, f32, f32) {
    let idx = (initial as usize) % AVATARS.len();
    AVATARS.get(idx).copied().unwrap_or((0.5, 0.6, 0.8))
}

/// "4 contacts in SQLite" / "1 contact in SQLite" from a count.
fn count_bytes(count: u64, buf: &mut [u8; 40]) -> &[u8] {
    let mut pos = 0usize;
    let mut num = [0u8; 20];
    for byte in u64_slice(count, &mut num) {
        push(buf, &mut pos, *byte);
    }
    let tail: &[u8] = if count == 1 {
        b" contact in SQLite"
    } else {
        b" contacts in SQLite"
    };
    for byte in tail {
        push(buf, &mut pos, *byte);
    }
    buf.get(..pos).unwrap_or(b"")
}

/// "Row 3 of contacts, in SQLite" from the row's primary key.
fn row_label(id: i64, buf: &mut [u8; 48]) -> &str {
    let mut pos = 0usize;
    let mut num = [0u8; 20];
    let parts: [&[u8]; 3] = [b"Row ", u64_slice(id.max(0) as u64, &mut num), b" of contacts, in SQLite"];
    for part in parts {
        for byte in part {
            push(buf, &mut pos, *byte);
        }
    }
    text_of(buf.get(..pos).unwrap_or(b""))
}

fn push(buf: &mut [u8], pos: &mut usize, byte: u8) {
    if let Some(slot) = buf.get_mut(*pos) {
        *slot = byte;
        *pos += 1;
    }
}

fn u64_slice(value: u64, buf: &mut [u8; 20]) -> &[u8] {
    if value == 0 {
        if let Some(slot) = buf.get_mut(0) {
            *slot = b'0';
        }
        return buf.get(..1).unwrap_or(b"0");
    }
    let mut scratch = [0u8; 20];
    let mut n = value;
    let mut count = 0usize;
    while n > 0 && count < scratch.len() {
        if let Some(slot) = scratch.get_mut(count) {
            *slot = b'0' + (n % 10) as u8;
        }
        n /= 10;
        count += 1;
    }
    let mut pos = 0usize;
    let mut i = count;
    while i > 0 {
        i -= 1;
        if let (Some(src), Some(dst)) = (scratch.get(i), buf.get_mut(pos)) {
            *dst = *src;
            pos += 1;
        }
    }
    buf.get(..pos).unwrap_or(b"0")
}

// ------------------------------------------------------------------
// Widget builders (root stack + one canvas filling it)
// ------------------------------------------------------------------

fn stack_root() -> types::WidgetNode {
    node(ROOT_ID, None, types::WidgetKind::Stack)
}

fn canvas_node() -> types::WidgetNode {
    node(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas)
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
            grow: 0.0,
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

// ------------------------------------------------------------------
// pure-alloc string helper (for the bound SQL text parameters)
// ------------------------------------------------------------------

use alloc::string::String;

fn pure_string(text: &str) -> String {
    let bytes = text.as_bytes();
    let len = bytes.len();
    if len == 0 {
        return String::new();
    }
    unsafe {
        let layout = core::alloc::Layout::from_size_align_unchecked(len, 1);
        let ptr = alloc::alloc::alloc(layout);
        if ptr.is_null() {
            #[cfg(target_arch = "wasm32")]
            core::arch::wasm32::unreachable();
            #[cfg(not(target_arch = "wasm32"))]
            unreachable!();
        }
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, len);
        String::from_raw_parts(ptr, len, len)
    }
}

krate::export!(Component);
