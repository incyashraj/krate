//! Krate Mdview -- a Markdown reader, rebuilt on a canvas.
//!
//! Renders a Markdown document into a readable page. The original rendered
//! through the host widget layer, and it showed: a flat light-grey ground, a
//! pile of blue pill buttons overlapping each other and the text, and body copy
//! with no hierarchy -- every line the same weight and size, headings marked
//! only by their literal `#`. It looked like a debug dump, not a document.
//!
//! This rebuild draws the whole page itself onto one `gfx.canvas2d`, so it can
//! give the text real typographic hierarchy: a large bold document title, H2/H3
//! headings that step down in size and colour, legible body at a comfortable
//! measure with generous line spacing and margins, indented bullet and numbered
//! lists with real markers, checked/unchecked task boxes, blockquotes with an
//! accent rule, and monospace fenced code blocks on their own panel. A slim top
//! bar names the file; a hairline separates it from the reading surface.
//!
//! It is laid out from the canvas it really has. The text column is as wide
//! as reads comfortably and no wider -- the whole screen on a phone, a
//! centred measure on a tablet or a desktop -- and lines are wrapped by the
//! width the host really draws them at. The page follows the person's light
//! or dark mode and their text size, scrolls with a finger, the wheel or the
//! keys, and each frame tells the screen reader what it shows. The wrapped
//! page is worked out once per width and text size and kept, so scrolling
//! only draws.
//!
//! The Markdown behaviour is preserved and real: `fs.list`/`fs.read` load a
//! document from the app's `docs/` folder when one is present, and the parser
//! understands headings, paragraphs, bold/italic/code/link inline text, ordered
//! and unordered and task lists, blockquotes, fenced code, and horizontal
//! rules. Under the automated screenshot, with no file in the sandbox, it renders
//! a built-in sample document that exercises every one of those.
//!
//! `#![no_std]` keeps this krate:*-only. The SDK owns the allocator and a
//! trapping panic handler, so no path pulls in the wasi:* set. Every access is
//! non-panicking, no `format!`, no `unwrap`, no slice indexing that can trap.

#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use krate::bindings::krate::fs::files;
use krate::bindings::krate::io::{args, stdio};
use krate::gfx::{canvas2d, types as gfx};
use krate::layout::{Look, Rect, Said, Screen};
use krate::math::F32Ext as _;
use krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

/// The window asked for on a desktop. Only a first size: every frame is laid
/// out from the canvas the app really has.
const WIDTH: f32 = 900.0;
const HEIGHT: f32 = 680.0;

/// The longest comfortable line, at the default text size: about 70
/// characters of body text. It grows with the text, so a larger size keeps
/// the same number of words to a line where there is room.
const MAX_MEASURE: f32 = 640.0;

struct Component;

// ------------------------------------------------------------------
// Parsed block model
// ------------------------------------------------------------------
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    H1,
    H2,
    H3,
    Para,
    Bullet,
    Numbered,
    TaskDone,
    TaskOpen,
    Quote,
    Code,
    Rule,
}

struct Block {
    kind: Kind,
    text: String,
    /// Nesting depth for list items (0 = top level).
    indent: u8,
    /// Ordinal for numbered items.
    ordinal: u32,
}

impl Block {
    fn new(kind: Kind, text: String) -> Self {
        Block {
            kind,
            text,
            indent: 0,
            ordinal: 0,
        }
    }
}

// ------------------------------------------------------------------
// Entry point
// ------------------------------------------------------------------
impl krate::Guest for Component {
    fn run() -> i32 {
        let size = types::WindowSize {
            width: WIDTH as u32,
            height: HEIGHT as u32,
        };
        let Ok(win) = window::create("Mdview", size) else {
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

        // Load a document from the docs folder if one is present; otherwise the
        // built-in sample. Returns (filename, source text).
        let (name, source) = load_document();
        let blocks = parse_markdown(&source);

        let mut page = Page::new();
        let _ = draw(canvas, win, &name, &blocks, &mut page);

        if quick {
            report(&name, blocks.len());
            let _ = window::close(win);
            return 0;
        }

        // Interactive: scroll with a finger, the wheel or the keys; close on
        // request. A real session ends only when the person closes it.
        'session: loop {
            // Take everything already queued, then draw once: a finger
            // dragging the page sends far more events than there are frames.
            let mut dirty = false;
            let mut next = events::wait(Some(200));
            while let Some(event) = next {
                match event {
                    types::Event::CloseRequested(id) if id == win => break 'session,
                    types::Event::Resized(_)
                    | types::Event::ThemeChanged(_)
                    | types::Event::RedrawRequested(_) => dirty = true,
                    types::Event::Wheel(w) => dirty |= page.scroll_by(w.dy),
                    types::Event::Key(k) if k.pressed => {
                        let line = page.line_step;
                        let screen = page.view_h * 0.85;
                        let by = match k.key.as_str() {
                            "ArrowDown" | "j" => line,
                            "ArrowUp" | "k" => -line,
                            "PageDown" | " " => screen,
                            "PageUp" => -screen,
                            "End" => page.max_scroll,
                            "Home" => -page.scroll - 1.0,
                            _ => 0.0,
                        };
                        if by != 0.0 {
                            dirty |= page.scroll_by(by);
                        }
                    }
                    _ => {}
                }
                next = events::poll();
            }
            if dirty {
                let _ = draw(canvas, win, &name, &blocks, &mut page);
            }
        }

        report(&name, blocks.len());
        let _ = window::close(win);
        0
    }
}

// ------------------------------------------------------------------
// Document loading
// ------------------------------------------------------------------

/// Load the first markdown file in `docs/`, or fall back to the built-in sample.
fn load_document() -> (String, String) {
    if let Ok(entries) = files::list("docs") {
        for e in entries {
            if ends_ci(e.as_bytes(), b".md") || ends_ci(e.as_bytes(), b".markdown") {
                let mut path = String::from("docs/");
                path.push_str(&e);
                if let Ok(file) =
                    files::open(&path, krate::bindings::krate::fs::files::OpenMode::Read)
                {
                    if let Some(text) = read_all(&file) {
                        return (e, text);
                    }
                }
            }
        }
    }
    (String::from("welcome.md"), String::from(SAMPLE))
}

fn read_all(file: &files::File) -> Option<String> {
    let mut out: Vec<u8> = Vec::new();
    loop {
        let chunk = file.read(4096).ok()?;
        if chunk.is_empty() {
            break;
        }
        out.extend_from_slice(&chunk);
        if out.len() > 1_000_000 {
            break;
        }
    }
    // Lossy: keep only valid UTF-8 up to the first bad byte, which is plenty for
    // a reader and stays panic-free.
    match core::str::from_utf8(&out) {
        Ok(s) => Some(String::from(s)),
        Err(e) => {
            let good = e.valid_up_to();
            out.get(..good)
                .and_then(|b| core::str::from_utf8(b).ok())
                .map(String::from)
        }
    }
}

fn ends_ci(b: &[u8], suffix: &[u8]) -> bool {
    let n = b.len();
    let m = suffix.len();
    if n < m {
        return false;
    }
    let start = n - m;
    let mut i = 0usize;
    while i < m {
        let c = match b.get(start + i) {
            Some(c) => to_lower(*c),
            None => return false,
        };
        let s = match suffix.get(i) {
            Some(s) => *s,
            None => return false,
        };
        if c != s {
            return false;
        }
        i += 1;
    }
    true
}

fn to_lower(c: u8) -> u8 {
    if c.is_ascii_uppercase() {
        c + 32
    } else {
        c
    }
}

// ------------------------------------------------------------------
// Markdown parsing -> blocks
// ------------------------------------------------------------------

fn parse_markdown(src: &str) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut in_code = false;
    let mut ordinal_counter: u32 = 0;

    for raw_line in src.split('\n') {
        // Count leading spaces for list indent (tabs treated as 2 spaces).
        let (indent_spaces, line) = strip_indent(raw_line);
        let trimmed = line;

        // Fenced code toggling.
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            blocks.push(Block::new(Kind::Code, String::from(raw_line)));
            continue;
        }

        if trimmed.is_empty() {
            ordinal_counter = 0;
            continue;
        }

        // Horizontal rule.
        if is_hr(trimmed) {
            blocks.push(Block::new(Kind::Rule, String::new()));
            continue;
        }

        // Headings.
        if let Some(rest) = trimmed.strip_prefix("### ") {
            blocks.push(Block::new(Kind::H3, strip_inline(rest)));
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("## ") {
            blocks.push(Block::new(Kind::H2, strip_inline(rest)));
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("# ") {
            blocks.push(Block::new(Kind::H1, strip_inline(rest)));
            continue;
        }

        // Blockquote.
        if let Some(rest) = trimmed.strip_prefix("> ") {
            blocks.push(Block::new(Kind::Quote, strip_inline(rest)));
            continue;
        }

        // Task list items.
        if let Some(rest) = task_item(trimmed) {
            let (done, body) = rest;
            let mut b = Block::new(
                if done { Kind::TaskDone } else { Kind::TaskOpen },
                strip_inline(body),
            );
            b.indent = indent_level(indent_spaces);
            blocks.push(b);
            continue;
        }

        // Unordered list.
        if let Some(rest) = bullet_item(trimmed) {
            let mut b = Block::new(Kind::Bullet, strip_inline(rest));
            b.indent = indent_level(indent_spaces);
            blocks.push(b);
            continue;
        }

        // Ordered list: "1. text".
        if let Some(rest) = numbered_item(trimmed) {
            ordinal_counter += 1;
            let mut b = Block::new(Kind::Numbered, strip_inline(rest));
            b.indent = indent_level(indent_spaces);
            b.ordinal = ordinal_counter;
            blocks.push(b);
            continue;
        }

        ordinal_counter = 0;
        // Paragraph (merge consecutive plain lines into one flowing paragraph).
        if let Some(last) = blocks.last_mut() {
            if last.kind == Kind::Para {
                last.text.push(' ');
                last.text.push_str(&strip_inline(trimmed));
                continue;
            }
        }
        blocks.push(Block::new(Kind::Para, strip_inline(trimmed)));
    }

    blocks
}

fn strip_indent(line: &str) -> (usize, &str) {
    let bytes = line.as_bytes();
    let mut i = 0usize;
    let mut spaces = 0usize;
    while i < bytes.len() {
        match bytes.get(i) {
            Some(b' ') => spaces += 1,
            Some(b'\t') => spaces += 2,
            _ => break,
        }
        i += 1;
    }
    (spaces, line.get(i..).unwrap_or(""))
}

fn indent_level(spaces: usize) -> u8 {
    (spaces / 2).min(4) as u8
}

fn is_hr(s: &str) -> bool {
    let t = s.trim();
    (t == "---" || t == "***" || t == "___") && t.len() >= 3
}

fn bullet_item(s: &str) -> Option<&str> {
    for p in ["- ", "* ", "+ "] {
        if let Some(rest) = s.strip_prefix(p) {
            return Some(rest);
        }
    }
    None
}

fn task_item(s: &str) -> Option<(bool, &str)> {
    // "- [x] text" / "- [ ] text" (also with * or +).
    let body = bullet_item(s)?;
    if let Some(rest) = body
        .strip_prefix("[x] ")
        .or_else(|| body.strip_prefix("[X] "))
    {
        return Some((true, rest));
    }
    if let Some(rest) = body.strip_prefix("[ ] ") {
        return Some((false, rest));
    }
    None
}

fn numbered_item(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes.get(i) {
            Some(c) if c.is_ascii_digit() => i += 1,
            _ => break,
        }
    }
    if i == 0 {
        return None;
    }
    // Expect ". " after the digits.
    if bytes.get(i) == Some(&b'.') && bytes.get(i + 1) == Some(&b' ') {
        return s.get(i + 2..);
    }
    None
}

/// Strip inline markdown markers, keeping the visible text. The host font is a
/// single weight, so we render one clean run rather than faking bold/italic; we
/// drop the `*`, `_`, backtick, and link syntax so the copy reads as prose.
fn strip_inline(s: &str) -> String {
    let mut out = String::new();
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let c = match bytes.get(i) {
            Some(c) => *c,
            None => break,
        };
        match c {
            b'*' | b'_' | b'`' => {
                // Skip a run of the same marker (handles ** and __).
                i += 1;
            }
            b'[' => {
                // Link: [text](url) -> keep text, drop the target.
                // Copy until ']'.
                i += 1;
                while i < bytes.len() {
                    match bytes.get(i) {
                        Some(&b']') => {
                            i += 1;
                            break;
                        }
                        Some(ch) => {
                            push_ascii(&mut out, *ch);
                            i += 1;
                        }
                        None => break,
                    }
                }
                // Skip "(...)" if present.
                if bytes.get(i) == Some(&b'(') {
                    while i < bytes.len() {
                        let done = bytes.get(i) == Some(&b')');
                        i += 1;
                        if done {
                            break;
                        }
                    }
                }
            }
            _ => {
                // Copy this UTF-8 codepoint whole.
                let cp_len = utf8_len(c);
                let end = (i + cp_len).min(bytes.len());
                if let Some(slice) = s.get(i..end) {
                    out.push_str(slice);
                }
                i = end;
            }
        }
    }
    out
}

fn push_ascii(out: &mut String, c: u8) {
    if c < 0x80 {
        out.push(c as char);
    }
}

fn utf8_len(first: u8) -> usize {
    if first < 0x80 {
        1
    } else if first >> 5 == 0b110 {
        2
    } else if first >> 4 == 0b1110 {
        3
    } else if first >> 3 == 0b11110 {
        4
    } else {
        1
    }
}

// ------------------------------------------------------------------
// Colours, for light and dark
// ------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Ink {
    H1,
    H2,
    H3,
    Body,
    Quote,
    Code,
    Mark,
}

struct Theme {
    bg_top: gfx::Color,
    bg_bot: gfx::Color,
    bar: gfx::Color,
    hair: gfx::Color,
    title: gfx::Color,
    h1: gfx::Color,
    h2: gfx::Color,
    h3: gfx::Color,
    body: gfx::Color,
    quote: gfx::Color,
    code: gfx::Color,
    code_bg: gfx::Color,
    mark: gfx::Color,
    accent: gfx::Color,
    pill: gfx::Color,
    pill_ink: gfx::Color,
    box_open: gfx::Color,
    box_edge: gfx::Color,
    box_done: gfx::Color,
    rule: gfx::Color,
}

impl Theme {
    fn ink(&self, ink: Ink) -> gfx::Color {
        match ink {
            Ink::H1 => self.h1,
            Ink::H2 => self.h2,
            Ink::H3 => self.h3,
            Ink::Body => self.body,
            Ink::Quote => self.quote,
            Ink::Code => self.code,
            Ink::Mark => self.mark,
        }
    }
}

fn theme(look: &Look) -> Theme {
    let contrast = look.high_contrast;
    if look.dark {
        // A dark, warm-neutral reading ground (a hair of blue), not flat black.
        Theme {
            bg_top: color(0.075, 0.078, 0.092, 1.0),
            bg_bot: color(0.055, 0.058, 0.070, 1.0),
            bar: color(0.055, 0.058, 0.070, 1.0),
            hair: color(1.0, 1.0, 1.0, 0.07),
            title: color(0.90, 0.93, 0.98, 1.0),
            h1: color(0.96, 0.97, 1.0, 1.0),
            h2: color(0.90, 0.93, 0.99, 1.0),
            h3: color(0.70, 0.80, 0.96, 1.0),
            body: if contrast {
                color(0.93, 0.94, 0.96, 1.0)
            } else {
                color(0.80, 0.83, 0.88, 1.0)
            },
            quote: if contrast {
                color(0.88, 0.90, 0.94, 1.0)
            } else {
                color(0.72, 0.76, 0.84, 1.0)
            },
            code: color(0.78, 0.86, 0.80, 1.0),
            code_bg: color(0.03, 0.05, 0.06, 1.0),
            mark: color(0.55, 0.70, 0.95, 1.0),
            accent: color(0.42, 0.72, 1.0, 1.0),
            pill: color(0.42, 0.72, 1.0, 0.14),
            pill_ink: color(0.62, 0.76, 0.98, 1.0),
            box_open: color(0.18, 0.20, 0.26, 1.0),
            box_edge: color(1.0, 1.0, 1.0, 0.18),
            box_done: color(0.30, 0.70, 0.45, 1.0),
            rule: color(1.0, 1.0, 1.0, 0.12),
        }
    } else {
        // Paper: a warm white page with near-black type.
        Theme {
            bg_top: color(0.988, 0.988, 0.984, 1.0),
            bg_bot: color(0.973, 0.973, 0.965, 1.0),
            bar: color(0.965, 0.965, 0.957, 1.0),
            hair: color(0.0, 0.0, 0.0, 0.08),
            title: color(0.10, 0.11, 0.13, 1.0),
            h1: color(0.07, 0.08, 0.10, 1.0),
            h2: color(0.10, 0.11, 0.14, 1.0),
            h3: color(0.16, 0.29, 0.52, 1.0),
            body: if contrast {
                color(0.10, 0.11, 0.13, 1.0)
            } else {
                color(0.22, 0.24, 0.28, 1.0)
            },
            quote: if contrast {
                color(0.16, 0.18, 0.21, 1.0)
            } else {
                color(0.33, 0.36, 0.41, 1.0)
            },
            code: color(0.13, 0.27, 0.18, 1.0),
            code_bg: color(0.937, 0.941, 0.937, 1.0),
            mark: color(0.20, 0.40, 0.78, 1.0),
            accent: color(0.18, 0.44, 0.92, 1.0),
            pill: color(0.18, 0.44, 0.92, 0.10),
            pill_ink: color(0.16, 0.38, 0.80, 1.0),
            box_open: color(1.0, 1.0, 1.0, 1.0),
            box_edge: color(0.0, 0.0, 0.0, 0.28),
            box_done: color(0.16, 0.60, 0.36, 1.0),
            rule: color(0.0, 0.0, 0.0, 0.12),
        }
    }
}

// ------------------------------------------------------------------
// Layout: the document wrapped into lines, kept between frames
// ------------------------------------------------------------------

/// The type for one kind of block, at the person's text size.
struct Style {
    size: f32,
    weight: u16,
    mono: bool,
    /// Space added ABOVE the block.
    space_before: f32,
    /// Distance from one wrapped line to the next.
    line_h: f32,
    ink: Ink,
    /// Extra left indent, for lists, quotes and code.
    indent_px: f32,
}

fn style_for(kind: Kind, indent: u8, look: &Look, narrow: bool) -> Style {
    let s = |pt: f32| look.text(pt);
    let ind = indent as f32 * s(22.0);
    let body = s(17.0);
    let (size, weight, space, lh, ink, ind_px, mono) = match kind {
        Kind::H1 => (
            s(if narrow { 30.0 } else { 34.0 }),
            700,
            s(24.0),
            1.22,
            Ink::H1,
            0.0,
            false,
        ),
        Kind::H2 => (s(24.0), 650, s(28.0), 1.3, Ink::H2, 0.0, false),
        Kind::H3 => (s(19.0), 600, s(22.0), 1.35, Ink::H3, 0.0, false),
        Kind::Para => (body, 400, s(14.0), 1.6, Ink::Body, 0.0, false),
        Kind::Bullet | Kind::Numbered | Kind::TaskDone | Kind::TaskOpen => {
            (body, 400, s(8.0), 1.55, Ink::Body, s(30.0) + ind, false)
        }
        Kind::Quote => (body, 400, s(14.0), 1.6, Ink::Quote, s(22.0), false),
        Kind::Code => (s(14.5), 400, s(16.0), 1.5, Ink::Code, s(16.0), true),
        Kind::Rule => (0.0, 400, s(22.0), 0.0, Ink::Body, 0.0, false),
    };
    Style {
        size,
        weight,
        mono,
        space_before: space,
        line_h: if kind == Kind::Rule {
            s(24.0)
        } else {
            size * lh
        },
        ink,
        indent_px: ind_px,
    }
}

/// One thing to draw, in page coordinates: x from the text column's left
/// edge, y from the top of the page.
enum Item {
    Text {
        x: f32,
        /// Baseline.
        y: f32,
        size: f32,
        weight: u16,
        mono: bool,
        ink: Ink,
        text: String,
    },
    /// A fenced code block's panel, behind its lines.
    Panel(Rect),
    /// A quote's accent bar.
    Bar(Rect),
    /// A horizontal rule.
    Rule(Rect),
    Dot {
        x: f32,
        y: f32,
        r: f32,
    },
    Task {
        r: Rect,
        done: bool,
    },
}

impl Item {
    /// The vertical band the item covers, for culling what is off screen.
    fn band(&self) -> (f32, f32) {
        match self {
            Item::Text { y, size, .. } => (y - size, y + size * 0.4),
            Item::Panel(r) | Item::Bar(r) | Item::Rule(r) | Item::Task { r, .. } => {
                (r.y, r.bottom())
            }
            Item::Dot { y, r, .. } => (y - r, y + r),
        }
    }
}

/// The page, wrapped for one text column width and text size, plus where it
/// is scrolled to. Rebuilt only when the width or the text size changes.
struct Page {
    key: (f32, f32, bool),
    items: Vec<Item>,
    /// Each block's band on the page, for the screen reader.
    spans: Vec<(usize, f32, f32)>,
    height: f32,
    scroll: f32,
    max_scroll: f32,
    view_h: f32,
    line_step: f32,
}

impl Page {
    fn new() -> Self {
        Page {
            key: (-1.0, -1.0, false),
            items: Vec::new(),
            spans: Vec::new(),
            height: 0.0,
            scroll: 0.0,
            max_scroll: 0.0,
            view_h: HEIGHT,
            line_step: 48.0,
        }
    }

    fn scroll_by(&mut self, dy: f32) -> bool {
        let next = (self.scroll + dy).clamp(0.0, self.max_scroll);
        let moved = next != self.scroll;
        self.scroll = next;
        moved
    }

    /// Wrap every block into lines `cw` wide. Text is measured by the host,
    /// with the face it is drawn in, so a line never runs past the column.
    fn build(&mut self, canvas: u64, blocks: &[Block], cw: f32, look: &Look, narrow: bool) {
        self.items.clear();
        self.spans.clear();
        let mut y = look.text(8.0);
        let mut code_run: Option<(usize, f32)> = None;
        let code_pad = look.text(10.0);
        for (bi, b) in blocks.iter().enumerate() {
            let st = style_for(b.kind, b.indent, look, narrow);
            let is_code = b.kind == Kind::Code;
            // A code run ends: its panel goes in behind the lines it holds.
            if !is_code {
                if let Some((at, top)) = code_run.take() {
                    let bottom = y + code_pad;
                    let at = at.min(self.items.len());
                    self.items
                        .insert(at, Item::Panel(Rect::new(0.0, top, cw, bottom - top)));
                    y = bottom;
                }
            }
            if !(is_code && code_run.is_some()) {
                y += st.space_before;
            }
            let top = y;
            if b.kind == Kind::Rule {
                self.items
                    .push(Item::Rule(Rect::new(0.0, y + st.line_h * 0.5, cw, 1.0)));
                y += st.line_h;
                self.spans.push((bi, top, y));
                continue;
            }
            if is_code && code_run.is_none() {
                code_run = Some((self.items.len(), y));
                y += code_pad;
            }

            let avail = cw - st.indent_px - if is_code { st.indent_px } else { 0.0 };
            let lines = wrap_lines(canvas, &b.text, &st, avail);
            let first_base = y + st.size * 0.95 + (st.line_h - st.size * 1.2) * 0.5;
            let tx = st.indent_px;

            // Markers, on the first line.
            match b.kind {
                Kind::Bullet => {
                    let r = (st.size * 0.17).max(2.5);
                    self.items.push(Item::Dot {
                        x: tx - st.size * 1.0,
                        y: first_base - st.size * 0.33,
                        r,
                    });
                }
                Kind::Numbered => {
                    let mut nb = [0u8; 16];
                    let ns = num_marker(b.ordinal, &mut nb);
                    let txt = String::from(core::str::from_utf8(ns).unwrap_or("1."));
                    let w = width_of(canvas, &txt, st.size, 500, false);
                    self.items.push(Item::Text {
                        x: tx - w - st.size * 0.45,
                        y: first_base,
                        size: st.size,
                        weight: 500,
                        mono: false,
                        ink: Ink::Mark,
                        text: txt,
                    });
                }
                Kind::TaskDone | Kind::TaskOpen => {
                    let side = st.size * 0.95;
                    let r = Rect::new(
                        tx - side - st.size * 0.5,
                        first_base - side * 0.86,
                        side,
                        side,
                    );
                    self.items.push(Item::Task {
                        r,
                        done: b.kind == Kind::TaskDone,
                    });
                }
                _ => {}
            }

            let mut line_y = first_base;
            for line in lines.into_iter() {
                self.items.push(Item::Text {
                    x: tx,
                    y: line_y,
                    size: st.size,
                    weight: st.weight,
                    mono: st.mono,
                    ink: st.ink,
                    text: line,
                });
                line_y += st.line_h;
                y += st.line_h;
            }
            if b.kind == Kind::Quote {
                let gap = look.text(4.0);
                self.items.push(Item::Bar(Rect::new(
                    look.text(3.0),
                    top - gap,
                    3.0,
                    y - top + gap,
                )));
            }
            self.spans.push((bi, top, y));
        }
        if let Some((at, top)) = code_run.take() {
            let bottom = y + code_pad;
            let at = at.min(self.items.len());
            self.items
                .insert(at, Item::Panel(Rect::new(0.0, top, cw, bottom - top)));
            y = bottom;
        }
        self.height = y + look.text(48.0);
        self.line_step = look.text(17.0) * 2.8;
    }
}

/// Wrap `text` into lines at most `avail` wide, measured in the face it is
/// drawn in. A code line keeps its indentation, and its continuation is
/// indented two spaces more, so wrapped code still reads as code.
fn wrap_lines(canvas: u64, text: &str, st: &Style, avail: f32) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let measure = |s: &str| width_of(canvas, s, st.size, st.weight, st.mono);
    if measure(text) <= avail || avail <= 0.0 {
        out.push(String::from(text));
        return out;
    }
    let lead_len = text.len() - text.trim_start().len();
    let lead = text.get(..lead_len).unwrap_or("");
    let mut current = String::from(lead);
    let mut has_word = false;
    for word in text.trim_start().split(' ') {
        let mut candidate = current.clone();
        if has_word {
            candidate.push(' ');
        }
        candidate.push_str(word);
        if has_word && measure(&candidate) > avail {
            out.push(core::mem::take(&mut current));
            if st.mono {
                current.push_str(lead);
                current.push_str("  ");
            }
            current.push_str(word);
        } else {
            current = candidate;
        }
        has_word = true;
    }
    if !current.is_empty() || out.is_empty() {
        out.push(current);
    }
    out
}

// ------------------------------------------------------------------
// Rendering
// ------------------------------------------------------------------

fn draw(
    canvas: u64,
    win: u64,
    name: &str,
    blocks: &[Block],
    page: &mut Page,
) -> Result<(), gfx::GfxError> {
    let size = canvas2d::canvas_size(canvas).unwrap_or(gfx::Size {
        width: WIDTH,
        height: HEIGHT,
    });
    let (w, h) = (size.width.max(1.0), size.height.max(1.0));
    let screen = Screen::of(win);
    let look = screen.look;
    let t = theme(&look);
    let mut said = Said::new();
    let narrow = w < 600.0;
    let (top, right, bottom, left) = screen.safe;
    let safe = Rect::new(0.0, 0.0, w, h).inset(top, right, bottom, left);

    canvas2d::linear_gradient(canvas, rect(0.0, 0.0, w, h), t.bg_top, t.bg_bot)?;

    // The text column: all of a phone, a centred comfortable measure on
    // anything wider.
    let margin = if narrow { 20.0 } else { 48.0 };
    let measure = MAX_MEASURE * look.text_scale.clamp(1.0, 1.4);
    let cw = (safe.w - margin * 2.0).min(measure).max(80.0);
    let x0 = safe.x + (safe.w - cw) * 0.5;

    let key = (cw, look.text_scale, narrow);
    if page.key != key {
        page.build(canvas, blocks, cw, &look, narrow);
        page.key = key;
    }

    // ---- the top bar: a filename and a tag, clear of the notch ----
    let title_size = look.text(17.0);
    let bar_h = (title_size * 2.6).max(screen.tap() + 8.0);
    let bar_bottom = top + bar_h;
    let view_top = bar_bottom + 1.0;
    let view_h = (h - bottom - view_top).max(1.0);
    page.view_h = view_h;
    page.max_scroll = (page.height - view_h).max(0.0);
    page.scroll = page.scroll.clamp(0.0, page.max_scroll);

    // ---- the page, under the bar, only what is on screen ----
    canvas2d::set_clip(canvas, 0.0, view_top, w, (h - view_top).max(0.0))?;
    let oy = view_top - page.scroll;
    for item in page.items.iter() {
        let (a, b) = item.band();
        if b + oy < view_top || a + oy > h {
            continue;
        }
        draw_item(canvas, item, x0, oy, &t)?;
    }
    canvas2d::clear_clip(canvas)?;

    // What the screen reader hears: each block on screen, whole.
    for (bi, a, b) in page.spans.iter() {
        let (ya, yb) = (a + oy, b + oy);
        if yb < view_top || ya > h - bottom {
            continue;
        }
        let Some(block) = blocks.get(*bi) else {
            continue;
        };
        let at = Rect::new(
            x0,
            ya.max(view_top),
            cw,
            (yb.min(h) - ya.max(view_top)).max(1.0),
        );
        match block.kind {
            Kind::H1 | Kind::H2 | Kind::H3 => {
                said.heading(at, &block.text);
            }
            Kind::Rule => {}
            Kind::TaskDone | Kind::TaskOpen => {
                let mut s = String::from(if block.kind == Kind::TaskDone {
                    "Done: "
                } else {
                    "To do: "
                });
                s.push_str(&block.text);
                said.text(at, &s);
            }
            _ if block.text.trim().is_empty() => {}
            _ => {
                said.text(at, &block.text);
            }
        }
    }

    // ---- top bar drawn last so content scrolls under it ----
    canvas2d::fill_rect(canvas, rect(0.0, 0.0, w, bar_bottom), t.bar)?;
    canvas2d::fill_rect(canvas, rect(0.0, bar_bottom, w, 1.0), t.hair)?;
    let bar = Rect::new(
        safe.x + margin.min(28.0),
        top,
        safe.w - 2.0 * margin.min(28.0),
        bar_h,
    );
    let mid = bar.y + bar.h * 0.5;
    // A little document glyph, then the filename.
    let glyph = title_size * 0.62;
    let doc = Rect::new(bar.x, mid - glyph * 0.62, glyph * 0.8, glyph * 1.24);
    canvas2d::fill_round_rect(canvas, gfx_rect(doc), radii(2.5), t.accent)?;
    canvas2d::fill_rect(
        canvas,
        rect(
            doc.x + doc.w * 0.22,
            doc.y + doc.h * 0.35,
            doc.w * 0.56,
            1.5,
        ),
        t.bar,
    )?;
    canvas2d::fill_rect(
        canvas,
        rect(doc.x + doc.w * 0.22, doc.y + doc.h * 0.6, doc.w * 0.4, 1.5),
        t.bar,
    )?;
    let name_x = doc.right() + 12.0;

    // Reading tag, right side, when there is room for it beside the name.
    let tag_size = look.text(11.5);
    let tag = "MARKDOWN";
    let tag_style = gfx::TextStyle {
        weight: 600,
        italic: false,
        letter_spacing: 0.8,
        family: gfx::FontFamily::Sans,
    };
    let tw =
        canvas2d::measure_text_styled(canvas, tag, tag_size, tag_style).map_or(0.0, |m| m.width);
    let pw = tw + tag_size * 2.0;
    let name_w = width_of(canvas, name, title_size, 600, false);
    let mut name_room = bar.right() - name_x;
    if name_x + name_w + 16.0 + pw <= bar.right() {
        let ph = tag_size * 2.1;
        let px = bar.right() - pw;
        canvas2d::fill_round_rect(
            canvas,
            rect(px, mid - ph * 0.5, pw, ph),
            radii(ph * 0.5),
            t.pill,
        )?;
        canvas2d::draw_text_styled(
            canvas,
            tag,
            pt(px + tag_size, mid + tag_size * 0.36),
            tag_size,
            t.pill_ink,
            tag_style,
        )?;
        name_room = px - 16.0 - name_x;
    }
    fit(
        canvas,
        name,
        name_x,
        mid + title_size * 0.36,
        title_size,
        600,
        t.title,
        name_room,
    )?;
    said.heading(Rect::new(name_x, bar.y, name_room.max(1.0), bar.h), name);

    // A thin scroll position mark at the right edge, while there is more.
    if page.max_scroll > 0.0 {
        let track_h = view_h - 16.0;
        let thumb_h = (track_h * view_h / page.height).clamp(28.0, track_h);
        let thumb_y = view_top + 8.0 + (track_h - thumb_h) * (page.scroll / page.max_scroll);
        let tx = safe.right() - 6.0;
        canvas2d::fill_round_rect(canvas, rect(tx, thumb_y, 3.0, thumb_h), radii(1.5), t.rule)?;
    }

    canvas2d::present(canvas)?;
    said.send(win);
    Ok(())
}

fn draw_item(canvas: u64, item: &Item, x0: f32, oy: f32, t: &Theme) -> Result<(), gfx::GfxError> {
    match item {
        Item::Text {
            x,
            y,
            size,
            weight,
            mono,
            ink,
            text,
        } => {
            if text.is_empty() {
                return Ok(());
            }
            canvas2d::draw_text_styled(
                canvas,
                text,
                pt(x0 + x, y + oy),
                *size,
                t.ink(*ink),
                style(*size, *weight, *mono),
            )
        }
        Item::Panel(r) => canvas2d::fill_round_rect(
            canvas,
            gfx_rect(Rect {
                x: r.x + x0,
                y: r.y + oy,
                ..*r
            }),
            radii(10.0),
            t.code_bg,
        ),
        Item::Bar(r) => canvas2d::fill_round_rect(
            canvas,
            gfx_rect(Rect {
                x: r.x + x0,
                y: r.y + oy,
                ..*r
            }),
            radii(1.5),
            t.accent,
        ),
        Item::Rule(r) => canvas2d::fill_rect(
            canvas,
            gfx_rect(Rect {
                x: r.x + x0,
                y: r.y + oy,
                ..*r
            }),
            t.rule,
        ),
        Item::Dot { x, y, r } => canvas2d::fill_circle(canvas, pt(x0 + x, y + oy), *r, t.mark),
        Item::Task { r, done } => {
            let at = Rect {
                x: r.x + x0,
                y: r.y + oy,
                ..*r
            };
            let rad = radii(r.w * 0.26);
            if *done {
                canvas2d::fill_round_rect(canvas, gfx_rect(at), rad, t.box_done)?;
                // A check mark: two strokes meeting at the foot.
                let (x, y, k) = (at.x, at.y, at.w);
                let white = color(1.0, 1.0, 1.0, 1.0);
                let thick = (k * 0.14).max(1.6);
                segment(
                    canvas,
                    x + k * 0.24,
                    y + k * 0.52,
                    x + k * 0.43,
                    y + k * 0.71,
                    thick,
                    white,
                )?;
                segment(
                    canvas,
                    x + k * 0.43,
                    y + k * 0.71,
                    x + k * 0.77,
                    y + k * 0.31,
                    thick,
                    white,
                )
            } else {
                canvas2d::fill_round_rect(canvas, gfx_rect(at), rad, t.box_open)?;
                canvas2d::stroke_round_rect(canvas, gfx_rect(at), rad, 1.2, t.box_edge)
            }
        }
    }
}

// ------------------------------------------------------------------
// Canvas helpers
// ------------------------------------------------------------------

fn style(size: f32, weight: u16, mono: bool) -> gfx::TextStyle {
    gfx::TextStyle {
        weight,
        italic: false,
        letter_spacing: if mono {
            0.0
        } else if size >= 26.0 {
            -0.6
        } else if size >= 20.0 {
            -0.3
        } else {
            0.0
        },
        family: if mono {
            gfx::FontFamily::Mono
        } else {
            gfx::FontFamily::Sans
        },
    }
}

/// Rendered width of a string, measured by the host with the same font
/// layout the page draws it with.
fn width_of(canvas: u64, s: &str, size: f32, weight: u16, mono: bool) -> f32 {
    canvas2d::measure_text_styled(canvas, s, size, style(size, weight, mono))
        .map_or(0.0, |m| m.width)
}

/// Draw `s` at most `room` wide: the longest leading part that fits, then an
/// ellipsis. A long filename cut short reads as "there is more".
#[allow(clippy::too_many_arguments)]
fn fit(
    canvas: u64,
    s: &str,
    x: f32,
    y: f32,
    size: f32,
    weight: u16,
    c: gfx::Color,
    room: f32,
) -> Result<(), gfx::GfxError> {
    if room <= 0.0 {
        return Ok(());
    }
    let st = style(size, weight, false);
    if width_of(canvas, s, size, weight, false) <= room {
        return canvas2d::draw_text_styled(canvas, s, pt(x, y), size, c, st);
    }
    let ell = width_of(canvas, "…", size, weight, false);
    let (mut lo, mut hi) = (0usize, s.len());
    while lo < hi {
        let mut mid = (lo + hi).div_ceil(2);
        while mid > lo && !s.is_char_boundary(mid) {
            mid -= 1;
        }
        if mid == lo {
            break;
        }
        if width_of(canvas, s.get(..mid).unwrap_or(""), size, weight, false) + ell <= room {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let part = s.get(..lo).unwrap_or("").trim_end();
    if part.is_empty() {
        return Ok(());
    }
    canvas2d::draw_text_styled(canvas, part, pt(x, y), size, c, st)?;
    canvas2d::draw_text_styled(
        canvas,
        "…",
        pt(x + width_of(canvas, part, size, weight, false), y),
        size,
        c,
        st,
    )
}

/// A straight stroke from one point to another: a tiny solid sprite,
/// stretched and turned, in one host call.
fn segment(
    canvas: u64,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    thick: f32,
    c: gfx::Color,
) -> Result<(), gfx::GfxError> {
    let (dx, dy) = (x1 - x0, y1 - y0);
    let len = dx.hypot(dy);
    if len <= 0.0 {
        return Ok(());
    }
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0) as u8;
    let px = [byte(c.r), byte(c.g), byte(c.b), byte(c.a)];
    let mut rgba = [0u8; 16];
    for (i, b) in rgba.iter_mut().enumerate() {
        *b = px.get(i % 4).copied().unwrap_or(0);
    }
    canvas2d::draw_sprite(
        canvas,
        pt((x0 + x1) * 0.5, (y0 + y1) * 0.5),
        gfx::Size {
            width: len + thick * 0.5,
            height: thick,
        },
        dy.atan2(dx),
        2,
        2,
        &rgba,
    )
}

fn gfx_rect(r: Rect) -> gfx::Rect {
    gfx::Rect {
        x: r.x,
        y: r.y,
        width: r.w,
        height: r.h,
    }
}
fn radii(r: f32) -> gfx::CornerRadii {
    gfx::CornerRadii {
        top_left: r,
        top_right: r,
        bottom_right: r,
        bottom_left: r,
    }
}
fn rect(x: f32, y: f32, width: f32, height: f32) -> gfx::Rect {
    gfx::Rect {
        x,
        y,
        width,
        height,
    }
}
fn pt(x: f32, y: f32) -> gfx::Point {
    gfx::Point { x, y }
}
fn color(r: f32, g: f32, b: f32, a: f32) -> gfx::Color {
    gfx::Color { r, g, b, a }
}

// ------------------------------------------------------------------
// Small formatting + reporting
// ------------------------------------------------------------------

fn num_marker(n: u32, buf: &mut [u8; 16]) -> &[u8] {
    let mut pos = 0usize;
    let mut scratch = [0u8; 12];
    let mut v = if n == 0 { 1 } else { n };
    let mut count = 0usize;
    while v > 0 && count < scratch.len() {
        if let Some(slot) = scratch.get_mut(count) {
            *slot = b'0' + (v % 10) as u8;
        }
        v /= 10;
        count += 1;
    }
    let mut i = count;
    while i > 0 {
        i -= 1;
        if let (Some(src), Some(dst)) = (scratch.get(i), buf.get_mut(pos)) {
            *dst = *src;
            pos += 1;
        }
    }
    if let Some(slot) = buf.get_mut(pos) {
        *slot = b'.';
        pos += 1;
    }
    buf.get(..pos).unwrap_or(b"1.")
}

fn report(name: &str, block_count: usize) {
    let out = stdio::stdout();
    let _ = out.write(b"mdview:ok\n");
    let _ = out.write(b"file:");
    let _ = out.write(name.as_bytes());
    let _ = out.write(b"\n");
    let _ = out.write(b"blocks:");
    let mut buf = [0u8; 20];
    let _ = out.write(usize_bytes(block_count, &mut buf));
    let _ = out.write(b"\n");
}

fn usize_bytes(value: usize, buf: &mut [u8; 20]) -> &[u8] {
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
// Built-in sample document (renders when no docs/ file is present)
// ------------------------------------------------------------------
const SAMPLE: &str = "\
# The Markdown Reader

A clean, comfortable surface for reading documents. Headings step down in \
size and weight, body copy sits at an easy measure, and lists, quotes, and \
code each get their own treatment.

## Typography that reads

This paragraph is set at a legible size with generous line spacing, so your \
eye can track from line to line without effort. Bold, *italic*, `inline code`, \
and [links](https://krate.tech) are all understood by the parser.

### Lists of every kind

- A simple bullet point
- Another item in the list
  - A nested item, indented one level
- Back to the top level

1. First, an ordered step
2. Then the second step
3. And finally the third

- [x] A task that is finished
- [ ] A task still waiting to be done

> A blockquote stands apart from the body with an accent rule, for a citation \
or an aside worth pulling out of the flow.

### Code, on its own panel

```
fn main() {
    println!(\"hello from a fenced block\");
}
```

---

That horizontal rule marks a section break. Everything above renders from real \
Markdown, drawn pixel by pixel on a canvas.";

// ----- widget builders -----

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

krate::export!(Component);
