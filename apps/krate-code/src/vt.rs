//! A terminal emulator: bytes from a pseudo-terminal in, a grid of coloured
//! cells out.
//!
//! What it understands is what `ls --color`, `cargo build`, a shell's line
//! editor, `less`, `top` and `vim` send an `xterm-256color` terminal: cursor
//! movement, erasing, inserting and deleting characters and lines, colours
//! (the 16, the 256 and 24-bit), bold, italic, underline and reverse, a
//! scroll region, the alternate screen, line wrap, tab stops, the DEC line
//! drawing set, the title, and the reports a program asks for (cursor
//! position, device attributes, colours). Lines that scroll off the top of
//! the main screen are kept as scrollback.
//!
//! Keys go the other way: [`key_bytes`] turns a key and its modifiers into
//! what a terminal sends for it, in the cursor-key mode the program chose.
//!
//! Plain `alloc`: the whole of it is proved by `cargo test`.

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Default,
    /// 0-15 are the theme's sixteen; 16-255 the xterm cube and greys.
    Idx(u8),
    Rgb(u8, u8, u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attr {
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
    pub hidden: bool,
    pub strike: bool,
}

impl Attr {
    pub const PLAIN: Attr = Attr {
        fg: Color::Default,
        bg: Color::Default,
        bold: false,
        dim: false,
        italic: false,
        underline: false,
        inverse: false,
        hidden: false,
        strike: false,
    };
}

impl Default for Attr {
    fn default() -> Self {
        Attr::PLAIN
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub c: char,
    pub a: Attr,
    /// 1 for an ordinary character, 2 for the first half of a wide one,
    /// 0 for the second half (drawn by the first).
    pub w: u8,
}

impl Cell {
    pub const BLANK: Cell = Cell {
        c: ' ',
        a: Attr::PLAIN,
        w: 1,
    };

    fn blank(bg: Color) -> Cell {
        Cell {
            c: ' ',
            a: Attr { bg, ..Attr::PLAIN },
            w: 1,
        }
    }
}

type Line = Vec<Cell>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Saved {
    x: usize,
    y: usize,
    attr: Attr,
    origin: bool,
    graphics: bool,
    pending: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum St {
    Ground,
    Esc,
    /// ESC followed by an intermediate byte (`(`, `)`, `#`, ...).
    EscInter(u8),
    Csi,
    Osc,
    OscEsc,
    /// DCS, SOS, PM, APC: skipped until the string terminator.
    Skip,
    SkipEsc,
}

/// How the program wants the mouse reported (it is only ever the wheel).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseMode {
    Off,
    On,
}

pub struct Vt {
    pub cols: usize,
    pub rows: usize,
    screen: Vec<Line>,
    /// The other screen: the main one while the alternate is shown.
    other: Vec<Line>,
    pub alt: bool,
    pub scrollback: VecDeque<Line>,
    pub max_scrollback: usize,
    pub x: usize,
    pub y: usize,
    /// A character was written in the last column: the next one wraps.
    pending_wrap: bool,
    attr: Attr,
    top: usize,
    bottom: usize,
    saved: Saved,
    saved_alt: Saved,
    tabs: Vec<bool>,
    // Modes.
    pub autowrap: bool,
    pub origin: bool,
    pub insert: bool,
    pub newline_mode: bool,
    pub cursor_visible: bool,
    pub app_cursor: bool,
    pub app_keypad: bool,
    pub bracketed_paste: bool,
    pub mouse: MouseMode,
    pub mouse_sgr: bool,
    graphics: bool,
    // Parser.
    st: St,
    params: Vec<Vec<Option<u32>>>,
    cur_param: Vec<Option<u32>>,
    cur_num: Option<u32>,
    private: Option<u8>,
    inter: Vec<u8>,
    osc: Vec<u8>,
    utf8: Vec<u8>,
    last_char: char,
    /// What the terminal answers a program (reports), to write back to it.
    pub reply: Vec<u8>,
    pub title: String,
    pub bells: usize,
    /// Lines scrolled back from the bottom while looking at history.
    pub view: usize,
    /// The theme behind the text, for a program that asks (vim does).
    pub dark: bool,
    /// Bumped on every change, so the view knows to draw again.
    pub version: u64,
}

impl Vt {
    pub fn new(cols: usize, rows: usize) -> Self {
        let cols = cols.max(2);
        let rows = rows.max(1);
        let mut v = Vt {
            cols,
            rows,
            screen: blank_grid(cols, rows),
            other: blank_grid(cols, rows),
            alt: false,
            scrollback: VecDeque::new(),
            max_scrollback: 5000,
            x: 0,
            y: 0,
            pending_wrap: false,
            attr: Attr::PLAIN,
            top: 0,
            bottom: rows - 1,
            saved: Saved::default(),
            saved_alt: Saved::default(),
            tabs: Vec::new(),
            autowrap: true,
            origin: false,
            insert: false,
            newline_mode: false,
            cursor_visible: true,
            app_cursor: false,
            app_keypad: false,
            bracketed_paste: false,
            mouse: MouseMode::Off,
            mouse_sgr: false,
            graphics: false,
            st: St::Ground,
            params: Vec::new(),
            cur_param: Vec::new(),
            cur_num: None,
            private: None,
            inter: Vec::new(),
            osc: Vec::new(),
            utf8: Vec::new(),
            last_char: ' ',
            reply: Vec::new(),
            title: String::new(),
            bells: 0,
            view: 0,
            dark: true,
            version: 0,
        };
        v.reset_tabs();
        v
    }

    fn reset_tabs(&mut self) {
        self.tabs = (0..self.cols).map(|i| i % 8 == 0 && i > 0).collect();
    }

    /// The visible row `i` (0 at the top), counting the scrollback view.
    pub fn view_row(&self, i: usize) -> &[Cell] {
        let view = self.view.min(self.scrollback.len());
        if i < view {
            let k = self.scrollback.len() - view + i;
            return &self.scrollback[k];
        }
        let r = i - view;
        self.screen.get(r).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Row `r` of the screen itself, ignoring the view.
    pub fn row(&self, r: usize) -> &[Cell] {
        self.screen.get(r).map(Vec::as_slice).unwrap_or(&[])
    }

    /// The screen's text, one string per row, trailing spaces trimmed.
    pub fn screen_text(&self) -> Vec<String> {
        self.screen.iter().map(|l| line_text(l)).collect()
    }

    /// Every line, scrollback first, as text.
    pub fn all_text(&self) -> Vec<String> {
        self.scrollback
            .iter()
            .chain(self.screen.iter())
            .map(|l| line_text(l))
            .collect()
    }

    /// How many lines can be looked at, scrollback and screen.
    pub fn total_rows(&self) -> usize {
        self.scrollback.len() + self.rows
    }

    /// Text between two places, in rows counted from the first line of
    /// scrollback (`total_rows` of them) and columns.
    pub fn text_between(&self, a: (usize, usize), b: (usize, usize)) -> String {
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        let mut out = String::new();
        for r in a.0..=b.0 {
            let line: &[Cell] = if r < self.scrollback.len() {
                &self.scrollback[r]
            } else {
                self.screen
                    .get(r - self.scrollback.len())
                    .map(Vec::as_slice)
                    .unwrap_or(&[])
            };
            let from = if r == a.0 { a.1 } else { 0 };
            let to = if r == b.0 {
                (b.1 + 1).min(line.len())
            } else {
                line.len()
            };
            let mut s = String::new();
            for c in line.iter().take(to).skip(from) {
                if c.w != 0 {
                    s.push(c.c);
                }
            }
            out.push_str(s.trim_end());
            if r != b.0 {
                out.push('\n');
            }
        }
        out
    }

    /// Scroll the view through history (positive is back in time).
    pub fn scroll_view(&mut self, lines: isize) {
        let max = self.scrollback.len() as isize;
        self.view = (self.view as isize + lines).clamp(0, max) as usize;
        self.version += 1;
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.max(2);
        let rows = rows.max(1);
        if cols == self.cols && rows == self.rows {
            return;
        }
        for grid in [&mut self.screen, &mut self.other] {
            for l in grid.iter_mut() {
                l.resize(cols, Cell::BLANK);
                if let Some(last) = l.last_mut() {
                    if last.w == 2 {
                        *last = Cell::BLANK;
                    }
                }
            }
        }
        // Fewer rows: lines above the cursor go to scrollback so the cursor
        // keeps its line.
        if rows < self.rows {
            let drop = (self.y + 1).saturating_sub(rows);
            for _ in 0..drop {
                let l = self.screen.remove(0);
                if !self.alt {
                    self.push_scrollback(l);
                }
            }
            self.y -= drop;
            self.screen.truncate(rows);
            self.other.truncate(rows);
        }
        while self.screen.len() < rows {
            self.screen.push(alloc::vec![Cell::BLANK; cols]);
        }
        while self.other.len() < rows {
            self.other.push(alloc::vec![Cell::BLANK; cols]);
        }
        for s in self.scrollback.iter_mut() {
            if s.len() > cols {
                s.truncate(cols);
            }
        }
        self.cols = cols;
        self.rows = rows;
        self.top = 0;
        self.bottom = rows - 1;
        self.x = self.x.min(cols - 1);
        self.y = self.y.min(rows - 1);
        self.pending_wrap = false;
        self.reset_tabs();
        self.version += 1;
    }

    /// Text for a log, not a program: `\n` starts a new line.
    pub fn write_text(&mut self, s: &str) {
        for part in s.split_inclusive('\n') {
            if let Some(body) = part.strip_suffix('\n') {
                self.feed(body.as_bytes());
                self.feed(b"\r\n");
            } else {
                self.feed(part.as_bytes());
            }
        }
    }

    /// Bytes from the program.
    pub fn feed(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.version += 1;
        for &b in bytes {
            self.byte(b);
        }
    }

    fn byte(&mut self, b: u8) {
        match self.st {
            St::Ground => self.ground(b),
            St::Esc => self.esc(b),
            St::EscInter(i) => {
                self.st = St::Ground;
                match (i, b) {
                    (b'(', b'0') => self.graphics = true,
                    (b'(', _) => self.graphics = false,
                    (b'#', b'8') => {
                        // DECALN: fill the screen with E.
                        for l in self.screen.iter_mut() {
                            for c in l.iter_mut() {
                                *c = Cell {
                                    c: 'E',
                                    ..Cell::BLANK
                                };
                            }
                        }
                    }
                    _ => {}
                }
            }
            St::Csi => self.csi_byte(b),
            St::Osc => match b {
                0x07 => self.osc_done(),
                0x1b => self.st = St::OscEsc,
                _ => {
                    if self.osc.len() < 4096 {
                        self.osc.push(b)
                    }
                }
            },
            St::OscEsc => {
                if b == b'\\' {
                    self.osc_done();
                } else {
                    self.st = St::Ground;
                    self.osc.clear();
                }
            }
            St::Skip => {
                if b == 0x1b {
                    self.st = St::SkipEsc;
                } else if b == 0x07 {
                    self.st = St::Ground;
                }
            }
            St::SkipEsc => {
                self.st = if b == b'\\' { St::Ground } else { St::Skip };
            }
        }
    }

    fn ground(&mut self, b: u8) {
        if !self.utf8.is_empty() {
            if b & 0xC0 == 0x80 {
                self.utf8.push(b);
                let need = utf8_len(self.utf8[0]);
                if self.utf8.len() == need {
                    let c = core::str::from_utf8(&self.utf8)
                        .ok()
                        .and_then(|s| s.chars().next())
                        .unwrap_or('\u{FFFD}');
                    self.utf8.clear();
                    self.print(c);
                }
                return;
            }
            self.utf8.clear();
            self.print('\u{FFFD}');
        }
        match b {
            0x1b => {
                self.st = St::Esc;
            }
            0x07 => self.bells += 1,
            0x08 => {
                self.pending_wrap = false;
                self.x = self.x.saturating_sub(1);
            }
            0x09 => self.tab(1),
            0x0a..=0x0c => {
                self.index();
                if self.newline_mode {
                    self.x = 0;
                }
            }
            0x0d => {
                self.x = 0;
                self.pending_wrap = false;
            }
            0x0e | 0x0f => {}
            0x00..=0x1f | 0x7f => {}
            0x20..=0x7e => self.print(b as char),
            0xC0..=0xF7 => self.utf8.push(b),
            _ => self.print('\u{FFFD}'),
        }
    }

    fn esc(&mut self, b: u8) {
        self.st = St::Ground;
        match b {
            b'[' => {
                self.st = St::Csi;
                self.params.clear();
                self.cur_param.clear();
                self.cur_num = None;
                self.private = None;
                self.inter.clear();
            }
            b']' => {
                self.st = St::Osc;
                self.osc.clear();
            }
            b'P' | b'X' | b'^' | b'_' => self.st = St::Skip,
            b'(' | b')' | b'*' | b'+' | b'#' | b'%' => self.st = St::EscInter(b),
            b'7' => self.save_cursor(),
            b'8' => self.restore_cursor(),
            b'D' => self.index(),
            b'E' => {
                self.index();
                self.x = 0;
            }
            b'M' => self.reverse_index(),
            b'H' => {
                if let Some(t) = self.tabs.get_mut(self.x) {
                    *t = true;
                }
            }
            b'c' => self.full_reset(),
            b'=' => self.app_keypad = true,
            b'>' => self.app_keypad = false,
            _ => {}
        }
    }

    fn csi_byte(&mut self, b: u8) {
        match b {
            b'0'..=b'9' => {
                let d = (b - b'0') as u32;
                self.cur_num = Some(
                    self.cur_num
                        .unwrap_or(0)
                        .saturating_mul(10)
                        .saturating_add(d),
                );
            }
            b':' => {
                self.cur_param.push(self.cur_num.take());
            }
            b';' => {
                self.cur_param.push(self.cur_num.take());
                let p = core::mem::take(&mut self.cur_param);
                self.params.push(p);
            }
            b'<'..=b'?' => {
                if self.params.is_empty() && self.cur_param.is_empty() && self.cur_num.is_none() {
                    self.private = Some(b);
                }
            }
            0x20..=0x2f => self.inter.push(b),
            0x40..=0x7e => {
                self.cur_param.push(self.cur_num.take());
                let p = core::mem::take(&mut self.cur_param);
                self.params.push(p);
                self.st = St::Ground;
                self.csi_dispatch(b);
            }
            0x1b => self.st = St::Esc,
            _ => {}
        }
    }

    /// Parameter `i`, `def` when absent or zero-defaulted.
    fn p(&self, i: usize, def: u32) -> u32 {
        match self
            .params
            .get(i)
            .and_then(|g| g.first().copied().flatten())
        {
            Some(0) | None => def,
            Some(n) => n,
        }
    }

    /// Parameter `i` where 0 is meaningful.
    fn p0(&self, i: usize) -> u32 {
        self.params
            .get(i)
            .and_then(|g| g.first().copied().flatten())
            .unwrap_or(0)
    }

    fn csi_dispatch(&mut self, f: u8) {
        if !self.inter.is_empty() {
            // `CSI ! p` (soft reset); cursor shapes (`CSI 2 q`) and the rest
            // change nothing a grid shows.
            if self.inter == b"!" && f == b'p' {
                self.soft_reset();
            }
            return;
        }
        match self.private {
            Some(b'?') => {
                let on = match f {
                    b'h' => true,
                    b'l' => false,
                    _ => return,
                };
                let modes: Vec<u32> = (0..self.params.len()).map(|i| self.p0(i)).collect();
                for m in modes {
                    self.dec_mode(m, on);
                }
                return;
            }
            Some(b'>') => {
                if f == b'c' {
                    self.reply.extend_from_slice(b"\x1b[>0;276;0c");
                }
                return;
            }
            Some(_) => return,
            None => {}
        }
        let n = self.p(0, 1) as usize;
        match f {
            b'@' => self.insert_chars(n),
            b'A' => self.move_to(self.x, self.y.saturating_sub(n).max(self.top_if_inside())),
            b'B' | b'e' => {
                let lim = self.bottom_if_inside();
                self.move_to(self.x, (self.y + n).min(lim));
            }
            b'C' | b'a' => self.move_to((self.x + n).min(self.cols - 1), self.y),
            b'D' => self.move_to(self.x.saturating_sub(n), self.y),
            b'E' => {
                let lim = self.bottom_if_inside();
                self.move_to(0, (self.y + n).min(lim));
            }
            b'F' => self.move_to(0, self.y.saturating_sub(n).max(self.top_if_inside())),
            b'G' | b'`' => self.move_to((n - 1).min(self.cols - 1), self.y),
            b'H' | b'f' => {
                let row = self.p(0, 1) as usize - 1;
                let col = self.p(1, 1) as usize - 1;
                let row = if self.origin {
                    (row + self.top).min(self.bottom)
                } else {
                    row.min(self.rows - 1)
                };
                self.move_to(col.min(self.cols - 1), row);
            }
            b'I' => self.tab(n),
            b'J' => self.erase_display(self.p0(0)),
            b'K' => self.erase_line(self.p0(0)),
            b'L' => self.insert_lines(n),
            b'M' => self.delete_lines(n),
            b'P' => self.delete_chars(n),
            b'S' => self.scroll_up(n),
            b'T' => self.scroll_down(n),
            b'X' => {
                let bg = self.attr.bg;
                let y = self.y;
                let end = (self.x + n).min(self.cols);
                for c in self.screen[y][self.x..end].iter_mut() {
                    *c = Cell::blank(bg);
                }
                self.pending_wrap = false;
            }
            b'Z' => {
                for _ in 0..n {
                    let mut x = self.x;
                    while x > 0 {
                        x -= 1;
                        if self.tabs.get(x).copied().unwrap_or(false) || x == 0 {
                            break;
                        }
                    }
                    self.x = x;
                }
                self.pending_wrap = false;
            }
            b'b' => {
                let c = self.last_char;
                for _ in 0..n.min(65_535) {
                    self.print(c);
                }
            }
            b'c' => self.reply.extend_from_slice(b"\x1b[?62;22c"),
            b'd' => {
                let row = n - 1;
                let row = if self.origin {
                    (row + self.top).min(self.bottom)
                } else {
                    row.min(self.rows - 1)
                };
                self.move_to(self.x, row);
            }
            b'g' => match self.p0(0) {
                0 => {
                    if let Some(t) = self.tabs.get_mut(self.x) {
                        *t = false;
                    }
                }
                3 => self.tabs.iter_mut().for_each(|t| *t = false),
                _ => {}
            },
            b'h' | b'l' => {
                let on = f == b'h';
                for i in 0..self.params.len() {
                    match self.p0(i) {
                        4 => self.insert = on,
                        20 => self.newline_mode = on,
                        _ => {}
                    }
                }
            }
            b'm' => self.sgr(),
            b'n' => match self.p0(0) {
                5 => self.reply.extend_from_slice(b"\x1b[0n"),
                6 => {
                    let row = if self.origin {
                        self.y - self.top
                    } else {
                        self.y
                    };
                    self.reply.extend_from_slice(b"\x1b[");
                    push_num(&mut self.reply, row + 1);
                    self.reply.push(b';');
                    push_num(&mut self.reply, self.x + 1);
                    self.reply.push(b'R');
                }
                _ => {}
            },
            b'r' => {
                let top = self.p(0, 1) as usize - 1;
                let bottom = self.p(1, self.rows as u32) as usize - 1;
                let bottom = bottom.min(self.rows - 1);
                if top < bottom {
                    self.top = top;
                    self.bottom = bottom;
                    let home = if self.origin { self.top } else { 0 };
                    self.move_to(0, home);
                }
            }
            b's' => self.save_cursor(),
            b'u' => self.restore_cursor(),
            _ => {}
        }
    }

    fn top_if_inside(&self) -> usize {
        if self.y >= self.top && self.y <= self.bottom {
            self.top
        } else {
            0
        }
    }

    fn bottom_if_inside(&self) -> usize {
        if self.y >= self.top && self.y <= self.bottom {
            self.bottom
        } else {
            self.rows - 1
        }
    }

    fn dec_mode(&mut self, m: u32, on: bool) {
        match m {
            1 => self.app_cursor = on,
            6 => {
                self.origin = on;
                let home = if on { self.top } else { 0 };
                self.move_to(0, home);
            }
            7 => self.autowrap = on,
            25 => self.cursor_visible = on,
            47 | 1047 => self.set_alt(on, false),
            1048 => {
                if on {
                    self.save_cursor()
                } else {
                    self.restore_cursor()
                }
            }
            1049 => {
                if on {
                    self.save_cursor();
                    self.set_alt(true, true);
                } else {
                    self.set_alt(false, false);
                    self.restore_cursor();
                }
            }
            1000 | 1002 | 1003 => self.mouse = if on { MouseMode::On } else { MouseMode::Off },
            1006 => self.mouse_sgr = on,
            2004 => self.bracketed_paste = on,
            _ => {}
        }
    }

    fn set_alt(&mut self, on: bool, clear: bool) {
        if on == self.alt {
            if on && clear {
                self.screen = blank_grid(self.cols, self.rows);
            }
            return;
        }
        core::mem::swap(&mut self.screen, &mut self.other);
        core::mem::swap(&mut self.saved, &mut self.saved_alt);
        self.alt = on;
        if on && clear {
            self.screen = blank_grid(self.cols, self.rows);
        }
        self.top = 0;
        self.bottom = self.rows - 1;
        self.view = 0;
    }

    fn sgr(&mut self) {
        let params = core::mem::take(&mut self.params);
        let mut i = 0;
        if params.is_empty() {
            self.attr = Attr::PLAIN;
        }
        while i < params.len() {
            let g = &params[i];
            let n = g.first().copied().flatten().unwrap_or(0);
            match n {
                0 => self.attr = Attr::PLAIN,
                1 => self.attr.bold = true,
                2 => self.attr.dim = true,
                3 => self.attr.italic = true,
                4 => self.attr.underline = !matches!(g.get(1), Some(Some(0))),
                7 => self.attr.inverse = true,
                8 => self.attr.hidden = true,
                9 => self.attr.strike = true,
                21 => self.attr.underline = true,
                22 => {
                    self.attr.bold = false;
                    self.attr.dim = false;
                }
                23 => self.attr.italic = false,
                24 => self.attr.underline = false,
                27 => self.attr.inverse = false,
                28 => self.attr.hidden = false,
                29 => self.attr.strike = false,
                30..=37 => self.attr.fg = Color::Idx((n - 30) as u8),
                39 => self.attr.fg = Color::Default,
                40..=47 => self.attr.bg = Color::Idx((n - 40) as u8),
                49 => self.attr.bg = Color::Default,
                90..=97 => self.attr.fg = Color::Idx((n - 90 + 8) as u8),
                100..=107 => self.attr.bg = Color::Idx((n - 100 + 8) as u8),
                38 | 48 | 58 => {
                    // Either `38;5;n` / `38;2;r;g;b` across parameters, or
                    // the colon form inside one (`38:2::r:g:b`).
                    let (color, used) = if g.len() > 1 {
                        (color_from(&g[1..]), 0)
                    } else {
                        let rest: Vec<Option<u32>> = params[i + 1..]
                            .iter()
                            .map(|p| p.first().copied().flatten())
                            .collect();
                        let kind = rest.first().copied().flatten();
                        let used = match kind {
                            Some(5) => 2,
                            Some(2) => 4,
                            _ => 0,
                        };
                        (color_from(&rest), used)
                    };
                    i += used;
                    if let Some(c) = color {
                        match n {
                            38 => self.attr.fg = c,
                            48 => self.attr.bg = c,
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }

    fn osc_done(&mut self) {
        self.st = St::Ground;
        let text = String::from_utf8_lossy(&self.osc).into_owned();
        self.osc.clear();
        let (code, rest) = text.split_once(';').unwrap_or((text.as_str(), ""));
        match code {
            "0" | "2" => {
                self.title = rest.chars().take(200).collect();
            }
            "10" | "11" if rest == "?" => {
                // vim asks for the background to pick its colours.
                let (fg, bg) = if self.dark {
                    ("d8d8/d8d8/d8d8", "1c1c/1c1c/1e1e")
                } else {
                    ("2020/2020/2020", "fbfb/fbfb/fafa")
                };
                self.reply.extend_from_slice(b"\x1b]");
                self.reply.extend_from_slice(code.as_bytes());
                self.reply.extend_from_slice(b";rgb:");
                self.reply
                    .extend_from_slice(if code == "10" { fg } else { bg }.as_bytes());
                self.reply.extend_from_slice(b"\x1b\\");
            }
            _ => {}
        }
    }

    fn print(&mut self, c: char) {
        let c = if self.graphics { dec_graphics(c) } else { c };
        let w = char_width(c);
        if w == 0 {
            return;
        }
        self.last_char = c;
        if self.pending_wrap && self.autowrap {
            self.x = 0;
            self.index();
        }
        self.pending_wrap = false;
        if w == 2 && self.x + 1 >= self.cols {
            if self.autowrap {
                let (x, y) = (self.x, self.y);
                self.screen[y][x] = Cell::blank(self.attr.bg);
                self.x = 0;
                self.index();
            } else {
                return;
            }
        }
        let y = self.y;
        if self.insert {
            let line = &mut self.screen[y];
            for _ in 0..w {
                line.insert(self.x, Cell::blank(self.attr.bg));
                line.pop();
            }
        }
        self.fix_wide_at(self.x);
        let a = self.attr;
        self.screen[y][self.x] = Cell { c, a, w: w as u8 };
        if w == 2 {
            self.fix_wide_at(self.x + 1);
            self.screen[y][self.x + 1] = Cell { c: ' ', a, w: 0 };
        }
        self.x += w;
        if self.x >= self.cols {
            self.x = self.cols - 1;
            self.pending_wrap = self.autowrap;
        }
    }

    /// Before overwriting column `x`, break a wide character it was part of.
    fn fix_wide_at(&mut self, x: usize) {
        let y = self.y;
        let line = &mut self.screen[y];
        match line.get(x).map(|c| c.w) {
            Some(0) if x > 0 => line[x - 1] = Cell::blank(line[x - 1].a.bg),
            Some(2) if x + 1 < line.len() => line[x + 1] = Cell::blank(line[x + 1].a.bg),
            _ => {}
        }
    }

    fn move_to(&mut self, x: usize, y: usize) {
        self.x = x.min(self.cols - 1);
        self.y = y.min(self.rows - 1);
        self.pending_wrap = false;
    }

    fn tab(&mut self, n: usize) {
        for _ in 0..n {
            let mut x = self.x + 1;
            while x < self.cols - 1 && !self.tabs.get(x).copied().unwrap_or(false) {
                x += 1;
            }
            self.x = x.min(self.cols - 1);
        }
        self.pending_wrap = false;
    }

    fn index(&mut self) {
        self.pending_wrap = false;
        if self.y == self.bottom {
            self.scroll_up(1);
        } else if self.y + 1 < self.rows {
            self.y += 1;
        }
    }

    fn reverse_index(&mut self) {
        self.pending_wrap = false;
        if self.y == self.top {
            self.scroll_down(1);
        } else if self.y > 0 {
            self.y -= 1;
        }
    }

    fn push_scrollback(&mut self, l: Line) {
        self.scrollback.push_back(l);
        while self.scrollback.len() > self.max_scrollback {
            self.scrollback.pop_front();
        }
        if self.view > 0 {
            // Keep the history a person is reading where it is.
            self.view = (self.view + 1).min(self.scrollback.len());
        }
    }

    fn scroll_up(&mut self, n: usize) {
        let n = n.min(self.bottom - self.top + 1);
        let bg = self.attr.bg;
        for _ in 0..n {
            let l = self.screen.remove(self.top);
            self.screen
                .insert(self.bottom, alloc::vec![Cell::blank(bg); self.cols]);
            if self.top == 0 && !self.alt {
                self.push_scrollback(l);
            }
        }
    }

    fn scroll_down(&mut self, n: usize) {
        let n = n.min(self.bottom - self.top + 1);
        let bg = self.attr.bg;
        for _ in 0..n {
            self.screen.remove(self.bottom);
            self.screen
                .insert(self.top, alloc::vec![Cell::blank(bg); self.cols]);
        }
    }

    fn insert_lines(&mut self, n: usize) {
        if self.y < self.top || self.y > self.bottom {
            return;
        }
        let n = n.min(self.bottom - self.y + 1);
        let bg = self.attr.bg;
        for _ in 0..n {
            self.screen.remove(self.bottom);
            self.screen
                .insert(self.y, alloc::vec![Cell::blank(bg); self.cols]);
        }
        self.x = 0;
        self.pending_wrap = false;
    }

    fn delete_lines(&mut self, n: usize) {
        if self.y < self.top || self.y > self.bottom {
            return;
        }
        let n = n.min(self.bottom - self.y + 1);
        let bg = self.attr.bg;
        for _ in 0..n {
            self.screen.remove(self.y);
            self.screen
                .insert(self.bottom, alloc::vec![Cell::blank(bg); self.cols]);
        }
        self.x = 0;
        self.pending_wrap = false;
    }

    fn insert_chars(&mut self, n: usize) {
        let (x, y, bg) = (self.x, self.y, self.attr.bg);
        let line = &mut self.screen[y];
        for _ in 0..n.min(self.cols - x) {
            line.insert(x, Cell::blank(bg));
            line.pop();
        }
        self.pending_wrap = false;
    }

    fn delete_chars(&mut self, n: usize) {
        let (x, y, bg) = (self.x, self.y, self.attr.bg);
        let line = &mut self.screen[y];
        for _ in 0..n.min(self.cols - x) {
            line.remove(x);
            line.push(Cell::blank(bg));
        }
        self.pending_wrap = false;
    }

    fn erase_line(&mut self, how: u32) {
        let (x, y, bg) = (self.x, self.y, self.attr.bg);
        let cols = self.cols;
        let range = match how {
            0 => x..cols,
            1 => 0..(x + 1).min(cols),
            _ => 0..cols,
        };
        for c in self.screen[y][range].iter_mut() {
            *c = Cell::blank(bg);
        }
        self.pending_wrap = false;
    }

    fn erase_display(&mut self, how: u32) {
        let bg = self.attr.bg;
        let cols = self.cols;
        match how {
            0 => {
                self.erase_line(0);
                for l in self.screen.iter_mut().skip(self.y + 1) {
                    *l = alloc::vec![Cell::blank(bg); cols];
                }
            }
            1 => {
                self.erase_line(1);
                for l in self.screen.iter_mut().take(self.y) {
                    *l = alloc::vec![Cell::blank(bg); cols];
                }
            }
            2 => {
                for l in self.screen.iter_mut() {
                    *l = alloc::vec![Cell::blank(bg); cols];
                }
            }
            3 => {
                self.scrollback.clear();
                self.view = 0;
            }
            _ => {}
        }
    }

    fn save_cursor(&mut self) {
        self.saved = Saved {
            x: self.x,
            y: self.y,
            attr: self.attr,
            origin: self.origin,
            graphics: self.graphics,
            pending: self.pending_wrap,
        };
    }

    fn restore_cursor(&mut self) {
        let s = self.saved;
        self.x = s.x.min(self.cols - 1);
        self.y = s.y.min(self.rows - 1);
        self.attr = s.attr;
        self.origin = s.origin;
        self.graphics = s.graphics;
        self.pending_wrap = s.pending;
    }

    fn soft_reset(&mut self) {
        self.attr = Attr::PLAIN;
        self.top = 0;
        self.bottom = self.rows - 1;
        self.origin = false;
        self.autowrap = true;
        self.insert = false;
        self.app_cursor = false;
        self.cursor_visible = true;
        self.graphics = false;
        self.pending_wrap = false;
    }

    fn full_reset(&mut self) {
        let (cols, rows, max, dark) = (self.cols, self.rows, self.max_scrollback, self.dark);
        let sb = core::mem::take(&mut self.scrollback);
        *self = Vt::new(cols, rows);
        self.scrollback = sb;
        self.max_scrollback = max;
        self.dark = dark;
    }

    /// What the mouse wheel sends: a report when the program asked for the
    /// mouse, arrow keys on the alternate screen (so `less` and `vim`
    /// scroll), or nothing -- then the view scrolls through history.
    pub fn wheel_bytes(&self, up: bool, col: usize, row: usize) -> Option<Vec<u8>> {
        if self.mouse == MouseMode::On {
            let button = if up { 64 } else { 65 };
            let mut v = Vec::new();
            if self.mouse_sgr {
                v.extend_from_slice(b"\x1b[<");
                push_num(&mut v, button);
                v.push(b';');
                push_num(&mut v, col + 1);
                v.push(b';');
                push_num(&mut v, row + 1);
                v.push(b'M');
            } else {
                v.extend_from_slice(b"\x1b[M");
                v.push((32 + button) as u8);
                v.push((33 + col.min(222)) as u8);
                v.push((33 + row.min(222)) as u8);
            }
            return Some(v);
        }
        if self.alt {
            let k: &[u8] = match (up, self.app_cursor) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            let mut v = Vec::new();
            for _ in 0..3 {
                v.extend_from_slice(k);
            }
            return Some(v);
        }
        None
    }

    /// Text pasted into the program, bracketed when it asked for that.
    pub fn paste_bytes(&self, text: &str) -> Vec<u8> {
        let mut v = Vec::new();
        let clean = text.replace("\r\n", "\r").replace('\n', "\r");
        if self.bracketed_paste {
            v.extend_from_slice(b"\x1b[200~");
            v.extend_from_slice(clean.replace('\u{1b}', "").as_bytes());
            v.extend_from_slice(b"\x1b[201~");
        } else {
            v.extend_from_slice(clean.as_bytes());
        }
        v
    }
}

fn blank_grid(cols: usize, rows: usize) -> Vec<Line> {
    (0..rows).map(|_| alloc::vec![Cell::BLANK; cols]).collect()
}

fn line_text(l: &[Cell]) -> String {
    let mut s = String::new();
    for c in l {
        if c.w != 0 {
            s.push(c.c);
        }
    }
    String::from(s.trim_end())
}

fn push_num(v: &mut Vec<u8>, n: usize) {
    v.extend_from_slice(crate::text::num(n).as_bytes());
}

fn utf8_len(first: u8) -> usize {
    match first {
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

fn color_from(p: &[Option<u32>]) -> Option<Color> {
    match p.first().copied().flatten()? {
        5 => Some(Color::Idx(p.get(1).copied().flatten()?.min(255) as u8)),
        2 => {
            // The colon form may carry a colour-space id: `2::r:g:b` has an
            // empty slot first, `2:cs:r:g:b` a number. Take the last three.
            let rest: Vec<u32> = p[1..].iter().map(|x| x.unwrap_or(0)).collect();
            if rest.len() < 3 {
                return None;
            }
            let k = rest.len() - 3;
            let c = |i: usize| rest[k + i].min(255) as u8;
            Some(Color::Rgb(c(0), c(1), c(2)))
        }
        _ => None,
    }
}

/// The DEC special graphics set: what `tmux`, `htop` and friends draw boxes
/// with after `ESC ( 0`.
fn dec_graphics(c: char) -> char {
    match c {
        '`' => '◆',
        'a' => '▒',
        'f' => '°',
        'g' => '±',
        'j' => '┘',
        'k' => '┐',
        'l' => '┌',
        'm' => '└',
        'n' => '┼',
        'o' => '⎺',
        'p' => '⎻',
        'q' => '─',
        'r' => '⎼',
        's' => '⎽',
        't' => '├',
        'u' => '┤',
        'v' => '┴',
        'w' => '┬',
        'x' => '│',
        'y' => '≤',
        'z' => '≥',
        '{' => 'π',
        '|' => '≠',
        '}' => '£',
        '~' => '·',
        c => c,
    }
}

/// How many cells a character takes: 0 for combining marks, 2 for wide
/// East Asian characters and most emoji, 1 otherwise.
pub fn char_width(c: char) -> usize {
    let u = c as u32;
    if u < 0x300 {
        return 1;
    }
    let zero = [
        (0x0300, 0x036F),
        (0x0483, 0x0489),
        (0x0591, 0x05BD),
        (0x0610, 0x061A),
        (0x064B, 0x065F),
        (0x0E31, 0x0E31),
        (0x0E34, 0x0E3A),
        (0x1AB0, 0x1AFF),
        (0x1DC0, 0x1DFF),
        (0x200B, 0x200F),
        (0x20D0, 0x20FF),
        (0xFE00, 0xFE0F),
        (0xFE20, 0xFE2F),
        (0xE0100, 0xE01EF),
    ];
    if zero.iter().any(|(a, b)| (*a..=*b).contains(&u)) {
        return 0;
    }
    let wide = [
        (0x1100, 0x115F),
        (0x231A, 0x231B),
        (0x2329, 0x232A),
        (0x23E9, 0x23EC),
        (0x25FD, 0x25FE),
        (0x2614, 0x2615),
        (0x2648, 0x2653),
        (0x26A1, 0x26A1),
        (0x26AA, 0x26AB),
        (0x26BD, 0x26BE),
        (0x26C4, 0x26C5),
        (0x26D4, 0x26D4),
        (0x26EA, 0x26EA),
        (0x26F5, 0x26F5),
        (0x26FA, 0x26FA),
        (0x26FD, 0x26FD),
        (0x2705, 0x2705),
        (0x270A, 0x270B),
        (0x2728, 0x2728),
        (0x274C, 0x274C),
        (0x2753, 0x2755),
        (0x2795, 0x2797),
        (0x2B1B, 0x2B1C),
        (0x2B50, 0x2B50),
        (0x2E80, 0x303E),
        (0x3041, 0x33FF),
        (0x3400, 0x4DBF),
        (0x4E00, 0x9FFF),
        (0xA000, 0xA4CF),
        (0xAC00, 0xD7A3),
        (0xF900, 0xFAFF),
        (0xFE30, 0xFE4F),
        (0xFF00, 0xFF60),
        (0xFFE0, 0xFFE6),
        (0x1F300, 0x1F64F),
        (0x1F680, 0x1F6FF),
        (0x1F900, 0x1F9FF),
        (0x1FA70, 0x1FAFF),
        (0x20000, 0x3FFFD),
    ];
    if wide.iter().any(|(a, b)| (*a..=*b).contains(&u)) {
        2
    } else {
        1
    }
}

/// The colour of palette entry `i` (16-255; 0-15 are the theme's).
pub fn xterm_rgb(i: u8) -> (u8, u8, u8) {
    if i >= 232 {
        let v = 8 + (i - 232) * 10;
        return (v, v, v);
    }
    let i = i.saturating_sub(16);
    let step = |n: u8| if n == 0 { 0 } else { 55 + n * 40 };
    (step(i / 36), step((i / 6) % 6), step(i % 6))
}

/// What a terminal sends for a named key (`"ArrowUp"`, `"Enter"`, `"F5"`)
/// or a single character with Ctrl or Alt held. `None` for a plain
/// character: that arrives as text.
pub fn key_bytes(
    name: &str,
    shift: bool,
    ctrl: bool,
    alt: bool,
    app_cursor: bool,
) -> Option<Vec<u8>> {
    let m = 1 + shift as u8 + 2 * alt as u8 + 4 * ctrl as u8;
    let csi_mod = |fin: u8| -> Vec<u8> {
        let mut v = alloc::vec![0x1b, b'[', b'1', b';'];
        v.extend_from_slice(crate::text::num(m as usize).as_bytes());
        v.push(fin);
        v
    };
    let tilde = |n: &str| -> Vec<u8> {
        let mut v = alloc::vec![0x1b, b'['];
        v.extend_from_slice(n.as_bytes());
        if m > 1 {
            v.push(b';');
            v.extend_from_slice(crate::text::num(m as usize).as_bytes());
        }
        v.push(b'~');
        v
    };
    let cursor = |fin: u8| -> Vec<u8> {
        if m > 1 {
            csi_mod(fin)
        } else if app_cursor {
            alloc::vec![0x1b, b'O', fin]
        } else {
            alloc::vec![0x1b, b'[', fin]
        }
    };
    let esc_if_alt = |mut v: Vec<u8>| -> Vec<u8> {
        if alt {
            v.insert(0, 0x1b);
        }
        v
    };
    let v = match name {
        "Enter" => esc_if_alt(alloc::vec![b'\r']),
        "Backspace" => esc_if_alt(alloc::vec![if ctrl { 0x08 } else { 0x7f }]),
        "Tab" => {
            if shift {
                b"\x1b[Z".to_vec()
            } else {
                alloc::vec![b'\t']
            }
        }
        "Escape" => alloc::vec![0x1b],
        "ArrowUp" => cursor(b'A'),
        "ArrowDown" => cursor(b'B'),
        "ArrowRight" => cursor(b'C'),
        "ArrowLeft" => cursor(b'D'),
        "Home" => cursor(b'H'),
        "End" => cursor(b'F'),
        "Insert" => tilde("2"),
        "Delete" => tilde("3"),
        "PageUp" => tilde("5"),
        "PageDown" => tilde("6"),
        "F1" => ss3_or_mod(b'P', m),
        "F2" => ss3_or_mod(b'Q', m),
        "F3" => ss3_or_mod(b'R', m),
        "F4" => ss3_or_mod(b'S', m),
        "F5" => tilde("15"),
        "F6" => tilde("17"),
        "F7" => tilde("18"),
        "F8" => tilde("19"),
        "F9" => tilde("20"),
        "F10" => tilde("21"),
        "F11" => tilde("23"),
        "F12" => tilde("24"),
        "Space" if ctrl => alloc::vec![0],
        "Space" if alt => alloc::vec![0x1b, b' '],
        _ => {
            let mut cs = name.chars();
            let (Some(c), None) = (cs.next(), cs.next()) else {
                return None;
            };
            if ctrl {
                let b = match c.to_ascii_lowercase() {
                    c @ 'a'..='z' => (c as u8) & 0x1f,
                    '@' | ' ' | '2' => 0,
                    '[' | '3' => 0x1b,
                    '\\' | '4' => 0x1c,
                    ']' | '5' => 0x1d,
                    '^' | '6' => 0x1e,
                    '_' | '-' | '/' | '7' => 0x1f,
                    '?' | '8' => 0x7f,
                    _ => return None,
                };
                esc_if_alt(alloc::vec![b])
            } else if alt {
                let mut v = alloc::vec![0x1b];
                let mut buf = [0u8; 4];
                v.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                v
            } else {
                return None;
            }
        }
    };
    Some(v)
}

fn ss3_or_mod(fin: u8, m: u8) -> Vec<u8> {
    if m > 1 {
        let mut v = alloc::vec![0x1b, b'[', b'1', b';'];
        v.extend_from_slice(crate::text::num(m as usize).as_bytes());
        v.push(fin);
        v
    } else {
        alloc::vec![0x1b, b'O', fin]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vt(cols: usize, rows: usize, bytes: &str) -> Vt {
        let mut v = Vt::new(cols, rows);
        v.feed(bytes.as_bytes());
        v
    }

    #[test]
    fn text_wraps_and_scrolls_into_history() {
        let v = vt(5, 2, "helloworld\r\nnext");
        assert_eq!(v.screen_text(), ["world", "next"]);
        assert_eq!(v.scrollback.len(), 1);
        assert_eq!(v.all_text(), ["hello", "world", "next"]);
        // Writing in the last column waits to wrap until the next character.
        let v = vt(5, 2, "abcde");
        assert_eq!((v.x, v.y), (4, 0));
        assert_eq!(v.screen_text(), ["abcde", ""]);
    }

    #[test]
    fn cursor_movement_and_erasing() {
        let v = vt(
            10,
            3,
            "abcdefghij\x1b[2;3HXY\x1b[1;5H\x1b[K\x1b[3;1Hzz\x1b[1D\x1b[1P",
        );
        assert_eq!(v.screen_text(), ["abcd", "  XY", "z"]);
        let v = vt(6, 3, "aaaaaa\r\nbbbbbb\r\ncccccc\x1b[2;3H\x1b[J");
        assert_eq!(v.screen_text(), ["aaaaaa", "bb", ""]);
        let v = vt(6, 3, "aaaaaa\r\nbbbbbb\r\ncccccc\x1b[2;3H\x1b[1J");
        assert_eq!(v.screen_text(), ["", "   bbb", "cccccc"]);
        let v = vt(6, 2, "abcdef\x1b[1;3H\x1b[2@\x1b[1;1H\x1b[2X");
        assert_eq!(v.screen_text(), ["    cd", ""]);
    }

    #[test]
    fn colours_16_256_and_truecolor() {
        let v = vt(20, 1, "\x1b[1;31mR\x1b[0m\x1b[38;5;208mO\x1b[38;2;10;20;30mT\x1b[48:2::1:2:3mU\x1b[94;7mB\x1b[m.");
        let r = v.row(0);
        assert_eq!(r[0].a.fg, Color::Idx(1));
        assert!(r[0].a.bold);
        assert_eq!(r[1].a.fg, Color::Idx(208));
        assert!(!r[1].a.bold);
        assert_eq!(r[2].a.fg, Color::Rgb(10, 20, 30));
        assert_eq!(r[3].a.bg, Color::Rgb(1, 2, 3));
        assert_eq!(r[4].a.fg, Color::Idx(12));
        assert!(r[4].a.inverse);
        assert_eq!(r[5].a, Attr::PLAIN);
        assert_eq!(xterm_rgb(196), (255, 0, 0));
        assert_eq!(xterm_rgb(232), (8, 8, 8));
    }

    /// What `ls --color` prints: colour on, name, colour off, padding.
    #[test]
    fn ls_color_output() {
        let v = vt(
            40,
            2,
            "\x1b[0m\x1b[01;34msrc\x1b[0m  Cargo.toml  \x1b[01;32mrun.sh\x1b[0m\r\n",
        );
        assert_eq!(v.screen_text()[0], "src  Cargo.toml  run.sh");
        let r = v.row(0);
        assert_eq!(r[0].a.fg, Color::Idx(4));
        assert!(r[0].a.bold);
        assert_eq!(r[5].a.fg, Color::Default);
        assert_eq!(r[17].a.fg, Color::Idx(2));
        assert_eq!((v.x, v.y), (0, 1));
    }

    /// `cargo build` in a terminal redraws its progress bar in place.
    #[test]
    fn carriage_return_redraw_like_cargo() {
        let v = vt(
            40,
            2,
            "    Building [==>  ] 3/10\r\x1b[K   \x1b[32mCompiling\x1b[0m p v0.1.0\r\n",
        );
        assert_eq!(v.screen_text()[0], "   Compiling p v0.1.0");
    }

    #[test]
    fn scroll_region_like_less_and_vim() {
        // A region of rows 2..3 scrolls alone; row 1 and 4 stay.
        let mut v = vt(4, 4, "top\r\nr1\r\nr2\r\nbot");
        v.feed(b"\x1b[2;3r\x1b[3;1H\n");
        assert_eq!(v.screen_text(), ["top", "r2", "", "bot"]);
        // Reverse index at the top of the region scrolls it down.
        v.feed(b"\x1b[2;1H\x1bM");
        assert_eq!(v.screen_text(), ["top", "", "r2", "bot"]);
        // Inserting and deleting lines stay inside it too.
        v.feed(b"\x1b[2;1Hx\x1b[1M");
        assert_eq!(v.screen_text(), ["top", "r2", "", "bot"]);
        assert!(v.scrollback.is_empty());
    }

    #[test]
    fn alternate_screen_keeps_the_shell_underneath() {
        let mut v = vt(10, 3, "$ vim\r\n");
        v.feed(b"\x1b[?1049h\x1b[H\x1b[2J~\r\n~ VIM");
        assert!(v.alt);
        assert_eq!(v.screen_text(), ["~", "~ VIM", ""]);
        v.feed(b"\x1b[?1049l");
        assert!(!v.alt);
        assert_eq!(v.screen_text(), ["$ vim", "", ""]);
        assert_eq!((v.x, v.y), (0, 1));
        // Nothing the full-screen program drew went to history.
        assert!(v.scrollback.is_empty());
    }

    #[test]
    fn reports_a_program_asks_for() {
        let mut v = vt(10, 5, "\x1b[3;4H\x1b[6n\x1b[c\x1b[5n");
        assert_eq!(v.reply, b"\x1b[3;4R\x1b[?62;22c\x1b[0n");
        v.reply.clear();
        v.feed(b"\x1b]11;?\x07");
        assert!(String::from_utf8_lossy(&v.reply).starts_with("\x1b]11;rgb:"));
        v.feed(b"\x1b]0;my title\x07\x1b]2;other\x1b\\");
        assert_eq!(v.title, "other");
    }

    #[test]
    fn utf8_wide_characters_and_line_drawing() {
        let v = vt(6, 2, "é中x\x1b(0lqk\x1b(B!");
        let r = v.row(0);
        assert_eq!(r[0].c, 'é');
        assert_eq!((r[1].c, r[1].w, r[2].w), ('中', 2, 0));
        assert_eq!(r[3].c, 'x');
        assert_eq!(r[4].c, '┌');
        assert_eq!(r[5].c, '─');
        assert_eq!(v.screen_text()[1], "┐!");
        // Split across two reads still decodes.
        let mut v = Vt::new(4, 1);
        let bytes = "中".as_bytes();
        v.feed(&bytes[..1]);
        v.feed(&bytes[1..]);
        assert_eq!(v.row(0)[0].c, '中');
    }

    #[test]
    fn modes_programs_set() {
        let v = vt(10, 3, "\x1b[?1h\x1b[?25l\x1b[?2004h\x1b[?1000h\x1b[?1006h");
        assert!(v.app_cursor && !v.cursor_visible && v.bracketed_paste);
        assert_eq!(v.wheel_bytes(true, 2, 1).unwrap(), b"\x1b[<64;3;2M");
        assert_eq!(v.paste_bytes("a\nb"), b"\x1b[200~a\rb\x1b[201~");
        let v = vt(10, 3, "\x1b[?1049h");
        assert_eq!(v.wheel_bytes(false, 0, 0).unwrap(), b"\x1b[B\x1b[B\x1b[B");
        assert!(vt(10, 3, "").wheel_bytes(true, 0, 0).is_none());
    }

    #[test]
    fn tabs_insert_mode_and_repeat() {
        let v = vt(20, 1, "a\tb\x1b[1;1H\x1b[4hZ\x1b[4l-\x1b[3b");
        assert_eq!(v.screen_text()[0], "Z----    b");
    }

    #[test]
    fn resize_keeps_the_cursor_line() {
        let mut v = vt(10, 4, "1\r\n2\r\n3\r\n4");
        v.resize(5, 2);
        assert_eq!(v.screen_text(), ["3", "4"]);
        assert_eq!(v.y, 1);
        assert_eq!(v.scrollback.len(), 2);
        v.resize(8, 3);
        assert_eq!(v.screen_text(), ["3", "4", ""]);
        assert_eq!(v.cols, 8);
    }

    #[test]
    fn history_view_and_copy() {
        let mut v = vt(10, 2, "one\r\ntwo\r\nthree\r\nfour");
        assert_eq!(v.total_rows(), 4);
        v.scroll_view(5);
        assert_eq!(v.view, 2);
        assert_eq!(line_text(v.view_row(0)), "one");
        assert_eq!(v.text_between((0, 0), (1, 1)), "one\ntw");
        v.scroll_view(-10);
        assert_eq!(line_text(v.view_row(0)), "three");
        // A log line written as text starts a new line.
        let mut log = Vt::new(20, 3);
        log.write_text("$ cargo build\nok\n");
        assert_eq!(log.screen_text(), ["$ cargo build", "ok", ""]);
    }

    #[test]
    fn key_encoding() {
        assert_eq!(
            key_bytes("ArrowUp", false, false, false, false).unwrap(),
            b"\x1b[A"
        );
        assert_eq!(
            key_bytes("ArrowUp", false, false, false, true).unwrap(),
            b"\x1bOA"
        );
        assert_eq!(
            key_bytes("ArrowLeft", false, true, false, false).unwrap(),
            b"\x1b[1;5D"
        );
        assert_eq!(key_bytes("c", false, true, false, false).unwrap(), [3]);
        assert_eq!(key_bytes("[", false, true, false, false).unwrap(), [0x1b]);
        assert_eq!(
            key_bytes("Enter", false, false, false, false).unwrap(),
            b"\r"
        );
        assert_eq!(
            key_bytes("Backspace", false, false, false, false).unwrap(),
            [0x7f]
        );
        assert_eq!(
            key_bytes("Tab", true, false, false, false).unwrap(),
            b"\x1b[Z"
        );
        assert_eq!(
            key_bytes("F5", false, false, false, false).unwrap(),
            b"\x1b[15~"
        );
        assert_eq!(
            key_bytes("PageDown", false, false, false, false).unwrap(),
            b"\x1b[6~"
        );
        assert_eq!(key_bytes("b", false, false, true, false).unwrap(), b"\x1bb");
        assert!(key_bytes("a", false, false, false, false).is_none());
    }

    #[test]
    fn garbage_never_panics() {
        let mut v = Vt::new(3, 2);
        let mut seed = 7u32;
        let mut bytes = Vec::new();
        for _ in 0..20_000 {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            bytes.push((seed >> 16) as u8);
        }
        v.feed(&bytes);
        v.feed(b"\x1b[999;999H\x1b[999P\x1b[999@\x1b[999L\x1b[999M\x1b[0;0r\x1b[5;2r\x1b[99999999999999b");
        v.resize(1, 1);
        v.feed(b"\xe4\xb8\xad\xe4\xb8\xad");
        assert_eq!(v.rows, 1);
    }
}
