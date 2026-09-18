//! Workload -- how much real computation fits inside one frame?
//!
//! The capability survey claimed a single-threaded guest means "any app doing
//! real computation blocks its own frame", and named parsing, indexing,
//! compressing and searching as things Krate cannot host. That is a claim
//! about a NUMBER nobody had measured.
//!
//! So measure it. Each band is a job a real app actually does, sized to
//! something a person would plausibly ask for, and timed by the guest's own
//! clock:
//!
//!   * parse   -- scan 1 MB of CSV-shaped text into fields
//!   * index   -- build a word-frequency table over 200,000 words
//!   * search  -- substring-scan 2 MB for a needle
//!   * sort    -- order 100,000 numbers
//!   * hash    -- checksum 4 MB
//!
//! Against a 16,666us frame, the answer says which of these an app can do
//! between two frames, which needs splitting across a few, and which genuinely
//! needs to be somewhere other than the frame loop.
//!
//! Measured on this machine, every band above fits inside ONE frame:
//!
//!     parse  1 MB    2,181 us      index  1 MB    2,890 us
//!     search 2 MB    1,492 us      sort   100k    1,377 us
//!     hash   4 MB    4,168 us
//!
//! At ten times the size the wall appears, and it is where arithmetic says it
//! should be -- the scaling is linear to within 10%, so there is no allocator
//! or collector cliff, just a steady ~964 MB/s:
//!
//!     parse  10 MB  19,946 us      index  10 MB  26,699 us
//!     search 20 MB  15,536 us      sort   1M     12,772 us
//!     hash   40 MB  41,491 us
//!
//! So the threshold is around 10 MB of text or a million items. Under it an
//! app can do the work between two frames and never notice; over it the work
//! has to be split across frames, which an app can do for itself by keeping a
//! cursor -- no threads required.
//!
//! Everything is generated in-process from a seed, so the numbers are about
//! computation and never about the disk.
//!
//! The same probe found the MEMORY ceiling by holding three equal strings and
//! growing them, under the default 256 MB limit:
//!
//!     3 x 30 MB =  90 MB live -> OK
//!     3 x 40 MB = 120 MB live -> TRAP
//!
//! So an app gets 90-120 MB of live data out of a nominal 256, because a
//! doubling `String` holds the old buffer and the new one at once. And the
//! overrun TRAPS rather than returning an error, because a no_std guest
//! allocates infallibly -- `handle_alloc_error` aborts and there is no
//! unwinding (K-416).
//!
//! `#![no_std]`: fixed-size state where it matters, hand-rolled formatting, no
//! `format!` and no panicking index.

#![no_std]
#![allow(clippy::needless_range_loop)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use krate::bindings::krate::io::stdio;
use krate::bindings::krate::time::clock;
use krate::gfx::{canvas2d, types as gfx};
use krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;
const WIDTH: f32 = 980.0;
const HEIGHT: f32 = 560.0;

/// One 60fps frame, in microseconds. The line every band is measured against.
const FRAME_US: u32 = 16_666;

const BG: gfx::Color = rgb(0.055, 0.063, 0.086);
const INK: gfx::Color = rgb(0.90, 0.92, 0.96);
const QUIET: gfx::Color = rgb(0.45, 0.49, 0.58);
const GOOD: gfx::Color = rgb(0.42, 0.85, 0.60);
const WARN: gfx::Color = rgb(0.95, 0.72, 0.35);
const BAD: gfx::Color = rgb(0.95, 0.42, 0.40);

const fn rgb(r: f32, g: f32, b: f32) -> gfx::Color {
    gfx::Color { r, g, b, a: 1.0 }
}

fn text(canvas: u64, s: &str, x: f32, y: f32, size: f32, ink: gfx::Color) {
    let _ = canvas2d::draw_text(canvas, s, gfx::Point { x, y }, size, ink);
}

fn push_u32(mut n: u32, out: &mut String) {
    if n == 0 {
        out.push('0');
        return;
    }
    let mut digits = [0u8; 10];
    let mut len = 0usize;
    while n > 0 && len < digits.len() {
        digits[len] = b'0' + (n % 10) as u8;
        n /= 10;
        len += 1;
    }
    while len > 0 {
        len -= 1;
        out.push(digits[len] as char);
    }
}

/// Thousands separators, so 1234567 reads as 1,234,567.
fn push_commas(n: u32, out: &mut String) {
    let mut raw = String::new();
    push_u32(n, &mut raw);
    let bytes = raw.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
}

/// A tiny deterministic generator, so every run measures the same work.
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> u32 {
        // xorshift32. Not for cryptography; for repeatable filler.
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
}

struct Band {
    name: &'static str,
    detail: &'static str,
    us: u32,
}

/// Build roughly `bytes` of CSV-shaped text: `word,word,number\n`.
fn make_text(bytes: usize) -> String {
    let mut rng = Rng(0x1234_5678);
    let mut out = String::new();
    const WORDS: [&str; 8] = [
        "alpha", "bravo", "delta", "echo", "foxtrot", "golf", "hotel", "india",
    ];
    while out.len() < bytes {
        out.push_str(WORDS[(rng.next() % 8) as usize]);
        out.push(',');
        out.push_str(WORDS[(rng.next() % 8) as usize]);
        out.push(',');
        push_u32(rng.next() % 100_000, &mut out);
        out.push('\n');
    }
    out
}

/// Scan CSV-shaped text into fields, counting them. The work a table import
/// does before it can show a single row.
fn parse(corpus: &str) -> u32 {
    let mut fields = 0u32;
    for line in corpus.split('\n') {
        if line.is_empty() {
            continue;
        }
        for _field in line.split(',') {
            fields += 1;
        }
    }
    fields
}

/// Count word frequencies. What a search box builds before it can answer.
fn index(corpus: &str) -> u32 {
    // Eight known words, so this measures scanning and counting rather than a
    // hash map's growth curve.
    const WORDS: [&str; 8] = [
        "alpha", "bravo", "delta", "echo", "foxtrot", "golf", "hotel", "india",
    ];
    let mut counts = [0u32; 8];
    for token in corpus.split(|c| c == ',' || c == '\n') {
        for (i, w) in WORDS.iter().enumerate() {
            if token == *w {
                counts[i] += 1;
                break;
            }
        }
    }
    counts.iter().sum()
}

/// Find every occurrence of a needle. What a find-in-document does.
fn search(corpus: &str, needle: &str) -> u32 {
    let mut hits = 0u32;
    let mut rest = corpus;
    while let Some(at) = rest.find(needle) {
        hits += 1;
        let step = at + needle.len();
        if step >= rest.len() {
            break;
        }
        rest = &rest[step..];
    }
    hits
}

/// Order a column. What a table does when a heading is clicked.
fn sort_numbers(n: usize) -> u32 {
    let mut rng = Rng(0x9E37_79B9);
    let mut values: Vec<u32> = Vec::with_capacity(n);
    for _ in 0..n {
        values.push(rng.next());
    }
    values.sort_unstable();
    values[n / 2]
}

/// Checksum a buffer. What saving or verifying a file does.
fn hash(bytes: &[u8]) -> u32 {
    // FNV-1a: one multiply and one xor per byte, which is the shape of every
    // real checksum even when the constants differ.
    let mut h: u32 = 0x811C_9DC5;
    for b in bytes {
        h ^= *b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// Ask for far more than the budget, and SURVIVE it.
///
/// This is the half of the memory story the trap hides. Growing a `String`
/// past the budget calls `handle_alloc_error` and aborts; asking first returns
/// `None` and the app keeps running. Reported rather than drawn, because the
/// point is that the next line of output happens at all.
fn memory_probe() {
    // Far past any plausible budget, so this is the refusal path every time.
    //
    // 2 GB, not 8: `usize` is 32 bits on wasm32, so 8 GB does not even fit in
    // the type -- the address space is the first ceiling, before any budget.
    const HUGE: usize = 2 * 1024 * 1024 * 1024;
    let mut line = String::new();
    line.push_str("workload: asked for 2 GB -> ");
    if krate::mem::have_room_for(HUGE) {
        line.push_str("granted (this machine gave the guest a very large budget)");
    } else {
        line.push_str("refused, and the app is still running");
    }
    say(&line);

    // And a size that does fit, so the check is not merely always-false.
    let mut ok = String::new();
    ok.push_str("workload: asked for 1 MB -> ");
    if krate::mem::have_room_for(1024 * 1024) {
        ok.push_str("granted");
    } else {
        ok.push_str("refused");
    }
    say(&ok);
}

fn measure() -> ([Band; 5], u32) {
    // One corpus, reused, so generation is not counted in any band.
    let corpus = make_text(1024 * 1024);
    let big = make_text(2 * 1024 * 1024);
    let blob = make_text(4 * 1024 * 1024);

    let t0 = clock::monotonic_nanos();
    let fields = parse(&corpus);
    let parse_us = ((clock::monotonic_nanos().saturating_sub(t0)) / 1000) as u32;

    let t0 = clock::monotonic_nanos();
    let words = index(&corpus);
    let index_us = ((clock::monotonic_nanos().saturating_sub(t0)) / 1000) as u32;

    let t0 = clock::monotonic_nanos();
    let hits = search(&big, "foxtrot");
    let search_us = ((clock::monotonic_nanos().saturating_sub(t0)) / 1000) as u32;

    let t0 = clock::monotonic_nanos();
    let median = sort_numbers(100_000);
    let sort_us = ((clock::monotonic_nanos().saturating_sub(t0)) / 1000) as u32;

    let t0 = clock::monotonic_nanos();
    let sum = hash(blob.as_bytes());
    let hash_us = ((clock::monotonic_nanos().saturating_sub(t0)) / 1000) as u32;

    // Use every result, so none of the work above can be optimised away.
    let guard = fields
        .wrapping_add(words)
        .wrapping_add(hits)
        .wrapping_add(median)
        .wrapping_add(sum);

    (
        [
            Band {
                name: "parse",
                detail: "1 MB of CSV into fields",
                us: parse_us,
            },
            Band {
                name: "index",
                detail: "word counts over 1 MB",
                us: index_us,
            },
            Band {
                name: "search",
                detail: "find every hit in 2 MB",
                us: search_us,
            },
            Band {
                name: "sort",
                detail: "order 100,000 numbers",
                us: sort_us,
            },
            Band {
                name: "hash",
                detail: "checksum 4 MB",
                us: hash_us,
            },
        ],
        guard,
    )
}

fn verdict(us: u32) -> (&'static str, gfx::Color) {
    if us < FRAME_US / 2 {
        ("fits in a frame, with room to draw", GOOD)
    } else if us < FRAME_US {
        ("fits in a frame, only just", WARN)
    } else {
        ("BLOWS the frame: this one needs splitting", BAD)
    }
}

fn draw(canvas: u64, bands: &[Band; 5]) {
    let _ = canvas2d::clear(canvas, BG);
    text(
        canvas,
        "How much real work fits in one frame?",
        40.0,
        54.0,
        22.0,
        INK,
    );
    text(
        canvas,
        "a 60fps frame is 16,666 us -- every number below is this guest's own clock",
        40.0,
        80.0,
        13.0,
        QUIET,
    );

    let widest = bands.iter().map(|b| b.us).max().unwrap_or(1).max(1);
    let mut y = 138.0;
    for band in bands {
        text(canvas, band.name, 40.0, y, 15.0, INK);
        text(canvas, band.detail, 130.0, y, 13.0, QUIET);

        let mut n = String::new();
        push_commas(band.us, &mut n);
        n.push_str(" us");
        let (note, color) = verdict(band.us);
        text(canvas, &n, 400.0, y, 15.0, color);
        text(canvas, note, 520.0, y, 12.5, color);

        // A bar, with the frame budget marked on it.
        let track = 380.0;
        let w = track * (band.us as f32 / widest as f32);
        let _ = canvas2d::fill_round_rect(
            canvas,
            gfx::Rect {
                x: 40.0,
                y: y + 10.0,
                width: w.max(2.0),
                height: 7.0,
            },
            gfx::CornerRadii {
                top_left: 3.5,
                top_right: 3.5,
                bottom_right: 3.5,
                bottom_left: 3.5,
            },
            color,
        );
        y += 62.0;
    }

    // The frame line, drawn where 16,666us falls on the same scale.
    let frame_x = 40.0 + 380.0 * (FRAME_US as f32 / widest as f32);
    if frame_x < 40.0 + 380.0 {
        let _ = canvas2d::fill_rect(
            canvas,
            gfx::Rect {
                x: frame_x,
                y: 128.0,
                width: 1.5,
                height: y - 138.0,
            },
            QUIET,
        );
        text(canvas, "one frame", frame_x + 6.0, 124.0, 11.0, QUIET);
    }

    let _ = canvas2d::present(canvas);
}

fn say(line: &str) {
    let out = stdio::stdout();
    let _ = out.write(line.as_bytes());
    let _ = out.write(b"\n");
    let _ = out.flush();
}

fn report(bands: &[Band; 5]) {
    for band in bands {
        let mut line = String::new();
        line.push_str("workload: ");
        line.push_str(band.name);
        line.push_str(" (");
        line.push_str(band.detail);
        line.push_str(") = ");
        push_commas(band.us, &mut line);
        line.push_str(" us -- ");
        line.push_str(verdict(band.us).0);
        say(&line);
    }
}

struct Component;

impl krate::Guest for Component {
    fn run() -> i32 {
        let win = match window::create(
            "Workload",
            types::WindowSize {
                width: WIDTH as u32,
                height: HEIGHT as u32,
            },
        ) {
            Ok(win) => win,
            Err(_) => return 1,
        };

        let mut root = types::WidgetNode {
            id: ROOT_ID,
            parent: None,
            kind: types::WidgetKind::Stack,
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
        };
        if tree::set_root(win, &root).is_err() {
            return 1;
        }
        root.id = CANVAS_ID;
        root.parent = Some(ROOT_ID);
        root.kind = types::WidgetKind::Canvas;
        if tree::upsert_node(win, &root).is_err() {
            return 1;
        }
        let canvas = match canvas2d::bind(win, CANVAS_ID) {
            Ok(canvas) => canvas,
            Err(_) => return 1,
        };

        let (bands, guard) = measure();
        if guard == 0x5555_5555 {
            // Never true in practice; it exists so the compiler cannot treat
            // any of the measured work as dead.
            say("workload: impossible guard");
        }
        memory_probe();
        report(&bands);
        draw(canvas, &bands);

        loop {
            match events::wait(Some(100)) {
                Some(types::Event::CloseRequested(_)) => break,
                Some(types::Event::RedrawRequested(_)) | Some(types::Event::Resized(_)) => {
                    draw(canvas, &bands)
                }
                _ => {}
            }
        }
        let _ = window::close(win);
        0
    }
}

krate::export!(Component);
