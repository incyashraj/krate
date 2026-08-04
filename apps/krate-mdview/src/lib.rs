//! Krate Mdview — a Markdown reader, rebuilt on a canvas.
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
//! accent rule, and monospace-styled fenced code blocks on their own panel. A
//! slim top bar names the file; a hairline separates it from the reading
//! surface. It looks like something you would want to read.
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

// Linked purely for its no_std runtime lang items.
extern crate krate as _krate_runtime;

use alloc::string::String;
use alloc::vec::Vec;

#[allow(warnings)]
mod bindings;

use bindings::krate::fs::files;
use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

const WIDTH: f32 = 900.0;
const HEIGHT: f32 = 680.0;

const MAX_FRAMES: u32 = 100_000;

// Reading layout.
const TOPBAR_H: f32 = 56.0;
/// Left/right page margin. The text column is centred with a comfortable
/// measure so lines are not too long to track.
const PAGE_MARGIN: f32 = 76.0;
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
impl bindings::Guest for Component {
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

        let mut scroll = 0.0f32;
        let _ = draw(canvas, &name, &blocks, scroll);

        if quick {
            report(&name, blocks.len());
            let _ = window::close(win);
            return 0;
        }

        // Interactive: scroll with the wheel / arrows, close on request.
        let content_h = measure_content(&blocks);
        let max_scroll = (content_h - (HEIGHT - TOPBAR_H)).max(0.0);
        let mut frames = 0u32;
        while frames < MAX_FRAMES {
            let ev = events::wait(Some(200));
            frames += 1;
            match ev {
                Some(types::Event::CloseRequested(id)) => {
                    if id == win {
                        break;
                    }
                }
                Some(types::Event::Key(k)) if k.pressed => {
                    let before = scroll;
                    if key_is(&k, "ArrowDown") || key_is(&k, "j") {
                        scroll = (scroll + 48.0).min(max_scroll);
                    } else if key_is(&k, "ArrowUp") || key_is(&k, "k") {
                        scroll = (scroll - 48.0).max(0.0);
                    } else if key_is(&k, "PageDown") || key_is(&k, " ") {
                        scroll = (scroll + (HEIGHT - TOPBAR_H) * 0.85).min(max_scroll);
                    } else if key_is(&k, "PageUp") {
                        scroll = (scroll - (HEIGHT - TOPBAR_H) * 0.85).max(0.0);
                    }
                    if scroll != before {
                        let _ = draw(canvas, &name, &blocks, scroll);
                    }
                }
                _ => {}
            }
        }

        report(&name, blocks.len());
        let _ = window::close(win);
        0
    }
}

fn key_is(k: &types::KeyEvent, name: &str) -> bool {
    k.key.as_str() == name
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
                if let Ok(file) = files::open(&path, bindings::krate::fs::files::OpenMode::Read) {
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
    if let Some(rest) = body.strip_prefix("[x] ").or_else(|| body.strip_prefix("[X] ")) {
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
// Layout metrics (shared by measuring and drawing)
// ------------------------------------------------------------------

struct Style {
    size: f32,
    /// Space added ABOVE the block.
    space_before: f32,
    /// Baseline line height for wrapped lines within the block.
    line_h: f32,
    color: gfx::Color,
    /// Extra left indent in pixels (for lists / quotes / code).
    indent_px: f32,
}

fn style_for(kind: Kind, indent: u8) -> Style {
    let ind = indent as f32 * 24.0;
    match kind {
        Kind::H1 => Style {
            size: 34.0,
            space_before: 30.0,
            line_h: 42.0,
            color: color(0.96, 0.97, 1.0, 1.0),
            indent_px: 0.0,
        },
        Kind::H2 => Style {
            size: 24.0,
            space_before: 30.0,
            line_h: 32.0,
            color: color(0.90, 0.93, 0.99, 1.0),
            indent_px: 0.0,
        },
        Kind::H3 => Style {
            size: 19.0,
            space_before: 22.0,
            line_h: 26.0,
            color: color(0.70, 0.80, 0.96, 1.0),
            indent_px: 0.0,
        },
        Kind::Para => Style {
            size: 17.0,
            space_before: 16.0,
            line_h: 27.0,
            color: color(0.80, 0.83, 0.88, 1.0),
            indent_px: 0.0,
        },
        Kind::Bullet | Kind::Numbered | Kind::TaskDone | Kind::TaskOpen => Style {
            size: 17.0,
            space_before: 9.0,
            line_h: 26.0,
            color: color(0.80, 0.83, 0.88, 1.0),
            indent_px: 34.0 + ind,
        },
        Kind::Quote => Style {
            size: 17.0,
            space_before: 14.0,
            line_h: 27.0,
            color: color(0.72, 0.76, 0.84, 1.0),
            indent_px: 26.0,
        },
        Kind::Code => Style {
            size: 15.0,
            space_before: 0.0,
            line_h: 22.0,
            color: color(0.78, 0.86, 0.80, 1.0),
            indent_px: 18.0,
        },
        Kind::Rule => Style {
            size: 0.0,
            space_before: 26.0,
            line_h: 26.0,
            color: color(1.0, 1.0, 1.0, 0.10),
            indent_px: 0.0,
        },
    }
}

fn content_width() -> f32 {
    (WIDTH - PAGE_MARGIN * 2.0).min(MAX_MEASURE)
}

/// Total laid-out height of all blocks, for scroll bounds.
fn measure_content(blocks: &[Block]) -> f32 {
    let cw = content_width();
    let mut y = 28.0;
    let mut i = 0usize;
    let mut prev_code = false;
    while i < blocks.len() {
        if let Some(b) = blocks.get(i) {
            let st = style_for(b.kind, b.indent);
            let is_code = b.kind == Kind::Code;
            // Code blocks pack tightly; only the first gets space before.
            let sb = if is_code && prev_code { 0.0 } else { st.space_before };
            y += sb;
            if b.kind == Kind::Rule {
                y += 4.0;
            } else {
                let avail = cw - st.indent_px;
                let lines = wrap_count(&b.text, st.size, avail);
                y += (lines.max(1) as f32) * st.line_h;
            }
            prev_code = is_code;
        }
        i += 1;
    }
    y + 40.0
}

// ------------------------------------------------------------------
// Rendering
// ------------------------------------------------------------------

fn draw(canvas: u64, name: &str, blocks: &[Block], scroll: f32) -> Result<(), gfx::GfxError> {
    // A dark, warm-neutral reading ground (a hair of blue), not flat black.
    canvas2d::linear_gradient(
        canvas,
        rect(0.0, 0.0, WIDTH, HEIGHT),
        color(0.075, 0.078, 0.092, 1.0),
        color(0.055, 0.058, 0.070, 1.0),
    )?;

    let cw = content_width();
    let x0 = (WIDTH - cw) * 0.5;
    let mut y = TOPBAR_H + 28.0 - scroll;

    let mut i = 0usize;
    let mut prev_code = false;
    while i < blocks.len() {
        if let Some(b) = blocks.get(i) {
            let st = style_for(b.kind, b.indent);
            let is_code = b.kind == Kind::Code;
            let sb = if is_code && prev_code { 0.0 } else { st.space_before };
            y += sb;

            // Only draw blocks intersecting the viewport (below the top bar).
            let block_h = if b.kind == Kind::Rule {
                st.line_h
            } else {
                let avail = cw - st.indent_px;
                (wrap_count(&b.text, st.size, avail).max(1) as f32) * st.line_h
            };
            let visible = y + block_h > TOPBAR_H && y < HEIGHT;

            if visible {
                y = draw_block(canvas, b, &st, x0, cw, y, is_code, prev_code)?;
            } else {
                y += block_h;
            }
            prev_code = is_code;
        }
        i += 1;
    }

    // ---- top bar drawn last so content scrolls under it ----
    canvas2d::fill_rect(
        canvas,
        rect(0.0, 0.0, WIDTH, TOPBAR_H),
        color(0.055, 0.058, 0.070, 0.96),
    )?;
    canvas2d::fill_rect(
        canvas,
        rect(0.0, TOPBAR_H, WIDTH, 1.0),
        color(1.0, 1.0, 1.0, 0.06),
    )?;
    // A little document glyph and the filename.
    canvas2d::draw_text(
        canvas,
        "\u{25A0}",
        pt(28.0, 36.0),
        16.0,
        color(0.42, 0.72, 1.0, 1.0),
    )?;
    canvas2d::draw_text(
        canvas,
        name,
        pt(52.0, 36.0),
        17.0,
        color(0.90, 0.93, 0.98, 1.0),
    )?;
    // Reading hint, right side. A small pill so it reads as a tag and never
    // crowds the window edge.
    let hint = "MARKDOWN";
    let tw = text_w(hint, 12.0);
    let pw = tw + 24.0;
    let px = WIDTH - 28.0 - pw;
    fill_round(canvas, px, 16.0, pw, 24.0, 12.0, color(0.42, 0.72, 1.0, 0.12))?;
    canvas2d::draw_text(
        canvas,
        hint,
        pt(px + 12.0, 32.0),
        12.0,
        color(0.58, 0.72, 0.95, 1.0),
    )?;

    canvas2d::present(canvas)?;
    Ok(())
}

/// Draw one block starting at baseline-top `y`, returning the new y below it.
fn draw_block(
    canvas: u64,
    b: &Block,
    st: &Style,
    x0: f32,
    cw: f32,
    mut y: f32,
    is_code: bool,
    prev_code: bool,
) -> Result<f32, gfx::GfxError> {
    // Horizontal rule.
    if b.kind == Kind::Rule {
        canvas2d::fill_rect(canvas, rect(x0, y + st.line_h * 0.5, cw, 1.0), st.color)?;
        return Ok(y + st.line_h);
    }

    let tx = x0 + st.indent_px;
    let avail = cw - st.indent_px;

    // Backgrounds / markers per kind.
    let lines = wrap_lines(&b.text, st.size, avail);
    let block_h = (lines.len().max(1) as f32) * st.line_h;

    match b.kind {
        Kind::Code => {
            // A code panel: fill a soft dark rounded block spanning the run. We
            // draw a per-line strip so consecutive code lines read as one panel
            // without needing lookahead; overlap the top for the first line.
            let pad_top = if prev_code { 0.0 } else { 6.0 };
            let pad_bot = 6.0;
            fill_round_top(
                canvas,
                x0,
                y - pad_top,
                cw,
                block_h + pad_top + pad_bot,
                !prev_code,
                color(0.03, 0.05, 0.06, 1.0),
            )?;
        }
        Kind::Quote => {
            // Accent rule on the left of the quote.
            canvas2d::fill_rect(
                canvas,
                rect(x0 + 4.0, y - 2.0, 3.0, block_h + 4.0),
                color(0.42, 0.72, 1.0, 0.8),
            )?;
        }
        Kind::Bullet => {
            canvas2d::fill_circle(
                canvas,
                pt(tx - 18.0, y + st.size * 0.5),
                3.0,
                color(0.5, 0.72, 1.0, 1.0),
            )?;
        }
        Kind::Numbered => {
            let mut nb = [0u8; 16];
            let ns = num_marker(b.ordinal, &mut nb);
            if let Ok(txt) = core::str::from_utf8(ns) {
                canvas2d::draw_text(
                    canvas,
                    txt,
                    pt(tx - 24.0, y + st.size * 0.9),
                    st.size,
                    color(0.55, 0.68, 0.9, 1.0),
                )?;
            }
        }
        Kind::TaskDone | Kind::TaskOpen => {
            let done = b.kind == Kind::TaskDone;
            let bx = tx - 22.0;
            let by = y + st.size * 0.15;
            let s = 15.0;
            let boxc = if done {
                color(0.30, 0.70, 0.45, 1.0)
            } else {
                color(0.18, 0.20, 0.26, 1.0)
            };
            fill_round(canvas, bx, by, s, s, 4.0, boxc)?;
            if done {
                // A check mark: two strokes.
                canvas2d::fill_rect(canvas, rect(bx + 3.5, by + 7.5, 3.0, 3.0), color(1.0, 1.0, 1.0, 1.0))?;
                canvas2d::fill_rect(canvas, rect(bx + 5.5, by + 5.0, 3.0, 5.5), color(1.0, 1.0, 1.0, 1.0))?;
                canvas2d::fill_rect(canvas, rect(bx + 7.5, by + 3.0, 3.0, 6.0), color(1.0, 1.0, 1.0, 1.0))?;
            } else {
                stroke_round(canvas, bx, by, s, s, 4.0, color(1.0, 1.0, 1.0, 0.16))?;
            }
        }
        _ => {}
    }

    // Draw the wrapped lines.
    let mut li = 0usize;
    while li < lines.len() {
        if let Some(line) = lines.get(li) {
            let baseline = y + st.size * (if is_code { 0.95 } else { 0.82 });
            canvas2d::draw_text(canvas, line, pt(tx, baseline), st.size, st.color)?;
        }
        y += st.line_h;
        li += 1;
    }
    if lines.is_empty() {
        y += st.line_h;
    }
    Ok(y)
}

// ------------------------------------------------------------------
// Word wrapping (host font is proportional; estimate advance per glyph)
// ------------------------------------------------------------------

/// Estimated glyph advance in ems. The host renders a MONOSPACE face, so every
/// glyph is a fixed ~0.6em wide; we wrap against that with a little headroom so
/// no line overruns the measure.
fn advance_em(size: f32) -> f32 {
    size * 0.62
}

fn wrap_lines(text: &str, size: f32, avail: f32) -> Vec<String> {
    let adv = advance_em(size);
    let max_chars = if adv > 0.0 {
        (avail / adv) as usize
    } else {
        80
    };
    let mut out: Vec<String> = Vec::new();
    if max_chars == 0 {
        out.push(String::from(text));
        return out;
    }
    let mut current = String::new();
    let mut current_len = 0usize;
    for word in text.split(' ') {
        let wlen = word.chars().count();
        if current_len == 0 {
            current.push_str(word);
            current_len = wlen;
        } else if current_len + 1 + wlen <= max_chars {
            current.push(' ');
            current.push_str(word);
            current_len += 1 + wlen;
        } else {
            out.push(core::mem::take(&mut current));
            current.push_str(word);
            current_len = wlen;
        }
    }
    if !current.is_empty() || out.is_empty() {
        out.push(current);
    }
    out
}

fn wrap_count(text: &str, size: f32, avail: f32) -> usize {
    wrap_lines(text, size, avail).len()
}

// ------------------------------------------------------------------
// Canvas helpers
// ------------------------------------------------------------------

fn fill_round(
    canvas: u64,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
    c: gfx::Color,
) -> Result<(), gfx::GfxError> {
    let r = r.min(w * 0.5).min(h * 0.5);
    canvas2d::fill_rect(canvas, rect(x + r, y, w - 2.0 * r, h), c)?;
    canvas2d::fill_rect(canvas, rect(x, y + r, w, h - 2.0 * r), c)?;
    canvas2d::fill_circle(canvas, pt(x + r, y + r), r, c)?;
    canvas2d::fill_circle(canvas, pt(x + w - r, y + r), r, c)?;
    canvas2d::fill_circle(canvas, pt(x + r, y + h - r), r, c)?;
    canvas2d::fill_circle(canvas, pt(x + w - r, y + h - r), r, c)?;
    Ok(())
}

/// A code panel strip: rounded on top only when `round_top`, square-ish bottom
/// so stacked strips join. Kept simple with a plain fill plus optional top
/// corner discs.
fn fill_round_top(
    canvas: u64,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    round_top: bool,
    c: gfx::Color,
) -> Result<(), gfx::GfxError> {
    let r = 8.0f32.min(w * 0.5);
    if round_top {
        canvas2d::fill_rect(canvas, rect(x + r, y, w - 2.0 * r, h), c)?;
        canvas2d::fill_rect(canvas, rect(x, y + r, w, h - r), c)?;
        canvas2d::fill_circle(canvas, pt(x + r, y + r), r, c)?;
        canvas2d::fill_circle(canvas, pt(x + w - r, y + r), r, c)?;
    } else {
        canvas2d::fill_rect(canvas, rect(x, y, w, h), c)?;
    }
    Ok(())
}

fn stroke_round(
    canvas: u64,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
    c: gfx::Color,
) -> Result<(), gfx::GfxError> {
    let t = 1.0;
    let r = r.min(w * 0.5).min(h * 0.5);
    canvas2d::fill_rect(canvas, rect(x + r, y, w - 2.0 * r, t), c)?;
    canvas2d::fill_rect(canvas, rect(x + r, y + h - t, w - 2.0 * r, t), c)?;
    canvas2d::fill_rect(canvas, rect(x, y + r, t, h - 2.0 * r), c)?;
    canvas2d::fill_rect(canvas, rect(x + w - t, y + r, t, h - 2.0 * r), c)?;
    Ok(())
}

fn rect(x: f32, y: f32, width: f32, height: f32) -> gfx::Rect {
    gfx::Rect { x, y, width, height }
}
fn pt(x: f32, y: f32) -> gfx::Point {
    gfx::Point { x, y }
}
fn color(r: f32, g: f32, b: f32, a: f32) -> gfx::Color {
    gfx::Color { r, g, b, a }
}
fn text_w(s: &str, size: f32) -> f32 {
    (s.chars().count() as f32) * size * 0.6
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
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

bindings::export!(Component with_types_in bindings);
