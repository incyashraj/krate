//! Laying a screen out for the device it is on (IC-912).
//!
//! An app is made once and opened on a phone held upright, a phone turned
//! sideways, a tablet and a desktop window. Rectangles computed from one
//! device's numbers -- a 260-point sidebar, four cards in a row -- cut the
//! screen off on the others. This kit computes them from the screen in
//! front of the app instead:
//!
//! ```ignore
//! use krate::layout::{Rect, Screen};
//!
//! let screen = Screen::of(window);           // ask again on every `resized`
//! let mut area = screen.safe_area().pad(16.0);
//! let header = area.take_top(screen.tap() + 8.0);
//! let nav = screen.nav(area, 240.0);         // a sidebar, or tabs on a phone
//! if let Some(sidebar) = nav.sidebar { /* draw the list of places */ }
//! if let Some(tabs) = nav.tabs { /* draw the same places as tabs */ }
//! for card in nav.content.grid(items.len(), 160.0, 200.0, 12.0) {
//!     // as many columns as fit, wrapping into rows
//! }
//! ```
//!
//! Every call is plain arithmetic on the numbers given, so a layout can be
//! worked out and tested away from a screen. `Screen` is only the place the
//! numbers come from.

use alloc::vec::Vec;

/// A rectangle in points, from its top-left corner.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// The right edge.
    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    /// The bottom edge.
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    /// Whether a point (a tap, a click) lands inside.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    /// Smaller by `top`, `right`, `bottom` and `left` points; never
    /// negative.
    pub fn inset(&self, top: f32, right: f32, bottom: f32, left: f32) -> Rect {
        Rect {
            x: self.x + left,
            y: self.y + top,
            w: (self.w - left - right).max(0.0),
            h: (self.h - top - bottom).max(0.0),
        }
    }

    /// Smaller by the same margin on every side.
    pub fn pad(&self, all: f32) -> Rect {
        self.inset(all, all, all, all)
    }

    /// Cut a band `h` points tall off the top and return it; `self` keeps
    /// what is left. A band never takes more than there is.
    pub fn take_top(&mut self, h: f32) -> Rect {
        let h = h.min(self.h).max(0.0);
        let band = Rect { h, ..*self };
        self.y += h;
        self.h -= h;
        band
    }

    /// Cut a band `h` points tall off the bottom and return it.
    pub fn take_bottom(&mut self, h: f32) -> Rect {
        let h = h.min(self.h).max(0.0);
        self.h -= h;
        Rect {
            y: self.y + self.h,
            h,
            ..*self
        }
    }

    /// Cut a band `w` points wide off the left and return it.
    pub fn take_left(&mut self, w: f32) -> Rect {
        let w = w.min(self.w).max(0.0);
        let band = Rect { w, ..*self };
        self.x += w;
        self.w -= w;
        band
    }

    /// Cut a band `w` points wide off the right and return it.
    pub fn take_right(&mut self, w: f32) -> Rect {
        let w = w.min(self.w).max(0.0);
        self.w -= w;
        Rect {
            x: self.x + self.w,
            w,
            ..*self
        }
    }

    /// `n` equal columns side by side, `gap` points apart.
    pub fn columns(&self, n: usize, gap: f32) -> Vec<Rect> {
        let n = n.max(1);
        let w = ((self.w - gap * (n - 1) as f32) / n as f32).max(0.0);
        (0..n)
            .map(|i| Rect {
                x: self.x + i as f32 * (w + gap),
                w,
                ..*self
            })
            .collect()
    }

    /// `n` equal rows one above the other, `gap` points apart.
    pub fn rows(&self, n: usize, gap: f32) -> Vec<Rect> {
        let n = n.max(1);
        let h = ((self.h - gap * (n - 1) as f32) / n as f32).max(0.0);
        (0..n)
            .map(|i| Rect {
                y: self.y + i as f32 * (h + gap),
                h,
                ..*self
            })
            .collect()
    }

    /// Cells for `count` items: as many columns as fit at `min_w` points
    /// each (at least one), sharing the width, wrapping into rows of
    /// `cell_h` points. Rows run past the bottom when there are more items
    /// than fit -- that is what scrolling is for; add the scroll offset to
    /// `y`.
    pub fn grid(&self, count: usize, min_w: f32, cell_h: f32, gap: f32) -> Vec<Rect> {
        let per_row = (((self.w + gap) / (min_w + gap)) as usize).max(1);
        let w = ((self.w - gap * (per_row - 1) as f32) / per_row as f32).max(0.0);
        (0..count)
            .map(|i| {
                let (col, row) = (i % per_row, i / per_row);
                Rect::new(
                    self.x + col as f32 * (w + gap),
                    self.y + row as f32 * (cell_h + gap),
                    w,
                    cell_h,
                )
            })
            .collect()
    }

    /// A `w` by `h` rectangle centred inside this one (shrunk to fit).
    pub fn center(&self, w: f32, h: f32) -> Rect {
        let (w, h) = (w.min(self.w), h.min(self.h));
        Rect::new(
            self.x + (self.w - w) / 2.0,
            self.y + (self.h - h) / 2.0,
            w,
            h,
        )
    }
}

/// How much room there is along one side: a phone, a tablet, a desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    /// A phone: under 600 points wide, or under 500 tall.
    Compact,
    /// A tablet, or a small desktop window.
    Medium,
    /// A desktop: 1000 points wide or more, or 900 tall or more.
    Large,
}

/// Where the places of an app go: a sidebar on a wide screen, tabs along
/// the bottom on a phone (`Screen::nav`). Exactly one of `sidebar` and
/// `tabs` is set.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Nav {
    /// The sidebar, on a screen wide enough for one.
    pub sidebar: Option<Rect>,
    /// The tab bar along the bottom, on a phone. Draw the same places as
    /// the sidebar would list, one tab each (five at most).
    pub tabs: Option<Rect>,
    /// Everything else.
    pub content: Rect,
}

/// What a screen reader is told about a frame, built as the frame is drawn
/// (IC-926): every button, heading and line of text, where it is and what
/// it says. Send it once the frame is drawn; a person using VoiceOver or
/// TalkBack then hears the screen and double-taps a button to press it,
/// which reaches the app as an ordinary tap at its centre.
///
/// ```ignore
/// let mut said = Said::new();
/// said.heading(title_rect, "Inbox");
/// said.button(compose_rect, "New message");
/// said.text(row_rect, "Anna: lunch tomorrow?");
/// said.send(win);
/// ```
#[cfg(feature = "gui")]
#[derive(Default)]
pub struct Said {
    elements: Vec<crate::ui::accessibility::Element>,
}

#[cfg(feature = "gui")]
impl Said {
    pub fn new() -> Self {
        Self::default()
    }

    fn push(
        &mut self,
        at: Rect,
        role: crate::ui::accessibility::Role,
        label: &str,
        value: &str,
        checked: Option<bool>,
    ) -> &mut Self {
        self.elements.push(crate::ui::accessibility::Element {
            x: at.x,
            y: at.y,
            width: at.w,
            height: at.h,
            role,
            label: alloc::string::String::from(label),
            value: alloc::string::String::from(value),
            checked,
            enabled: true,
        });
        self
    }

    /// Something to press.
    pub fn button(&mut self, at: Rect, label: &str) -> &mut Self {
        self.push(at, crate::ui::accessibility::Role::Button, label, "", None)
    }

    /// A link to somewhere else.
    pub fn link(&mut self, at: Rect, label: &str) -> &mut Self {
        self.push(at, crate::ui::accessibility::Role::Link, label, "", None)
    }

    /// A title over a part of the screen.
    pub fn heading(&mut self, at: Rect, label: &str) -> &mut Self {
        self.push(at, crate::ui::accessibility::Role::Heading, label, "", None)
    }

    /// Text to read.
    pub fn text(&mut self, at: Rect, text: &str) -> &mut Self {
        self.push(at, crate::ui::accessibility::Role::Text, text, "", None)
    }

    /// A picture, said as what it shows.
    pub fn image(&mut self, at: Rect, says: &str) -> &mut Self {
        self.push(at, crate::ui::accessibility::Role::Image, says, "", None)
    }

    /// An on/off switch.
    pub fn toggle(&mut self, at: Rect, label: &str, on: bool) -> &mut Self {
        self.push(
            at,
            crate::ui::accessibility::Role::Toggle,
            label,
            "",
            Some(on),
        )
    }

    /// A value that goes up and down, said with its value ("Volume", "60%").
    pub fn adjustable(&mut self, at: Rect, label: &str, value: &str) -> &mut Self {
        self.push(
            at,
            crate::ui::accessibility::Role::Adjustable,
            label,
            value,
            None,
        )
    }

    /// A field to type into, with what it holds.
    pub fn field(&mut self, at: Rect, label: &str, value: &str) -> &mut Self {
        self.push(
            at,
            crate::ui::accessibility::Role::Field,
            label,
            value,
            None,
        )
    }

    /// The last element, unusable right now (a greyed-out button).
    pub fn disabled(&mut self) -> &mut Self {
        if let Some(last) = self.elements.last_mut() {
            last.enabled = false;
        }
        self
    }

    /// Tell the screen reader. Call it once per frame, after drawing.
    pub fn send(&self, window: u64) {
        let _ = crate::ui::accessibility::describe(window, &self.elements);
    }
}

/// How the person asked things to look (IC-925): dark mode, more contrast,
/// larger text. Read it every frame with `Look::now()` (or through
/// `Screen::of`, which carries it) and draw from it:
///
/// ```ignore
/// let look = screen.look;
/// let colors = look.palette();                 // dark in a dark room
/// canvas2d::clear(canvas, colors.background)?;
/// canvas2d::draw_text_styled(canvas, "Inbox", at, look.text(28.0), colors.ink, bold)?;
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    /// The system is in dark mode.
    pub dark: bool,
    /// The person asked for more contrast.
    pub high_contrast: bool,
    /// The person's text size against the default; 1.0 by default.
    pub text_scale: f32,
}

impl Default for Look {
    fn default() -> Self {
        Self {
            dark: false,
            high_contrast: false,
            text_scale: 1.0,
        }
    }
}

impl Look {
    /// How things should look right now.
    #[cfg(feature = "gui")]
    pub fn now() -> Self {
        let look = crate::ui::appearance::current();
        Self {
            dark: look.dark,
            high_contrast: look.high_contrast,
            text_scale: look.text_scale,
        }
    }

    /// A font size for this person: `points` at their text size.
    pub fn text(&self, points: f32) -> f32 {
        points * self.text_scale
    }

    /// Colours that follow dark mode and contrast.
    #[cfg(feature = "gui")]
    pub fn palette(&self) -> Palette {
        let c = |r: f32, g: f32, b: f32| crate::gfx::types::Color { r, g, b, a: 1.0 };
        match (self.dark, self.high_contrast) {
            (false, contrast) => Palette {
                background: c(0.96, 0.96, 0.97),
                surface: c(1.0, 1.0, 1.0),
                ink: c(0.07, 0.07, 0.09),
                muted: if contrast {
                    c(0.22, 0.23, 0.26)
                } else {
                    c(0.43, 0.44, 0.48)
                },
                line: if contrast {
                    c(0.35, 0.36, 0.40)
                } else {
                    c(0.85, 0.86, 0.89)
                },
                accent: c(0.07, 0.07, 0.09),
                on_accent: c(1.0, 1.0, 1.0),
            },
            (true, contrast) => Palette {
                background: c(0.07, 0.07, 0.08),
                surface: c(0.13, 0.13, 0.15),
                ink: c(0.95, 0.95, 0.97),
                muted: if contrast {
                    c(0.85, 0.86, 0.89)
                } else {
                    c(0.62, 0.63, 0.67)
                },
                line: if contrast {
                    c(0.70, 0.71, 0.75)
                } else {
                    c(0.25, 0.26, 0.29)
                },
                accent: c(0.95, 0.95, 0.97),
                on_accent: c(0.07, 0.07, 0.09),
            },
        }
    }
}

/// An app's colours for the person's look: from `Look::palette`.
#[cfg(feature = "gui")]
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    /// Behind everything.
    pub background: crate::gfx::types::Color,
    /// Cards, sheets and fields on the background.
    pub surface: crate::gfx::types::Color,
    /// Text a person must read.
    pub ink: crate::gfx::types::Color,
    /// Secondary text: dates, hints, captions.
    pub muted: crate::gfx::types::Color,
    /// Borders and dividers.
    pub line: crate::gfx::types::Color,
    /// The main button.
    pub accent: crate::gfx::types::Color,
    /// Text on the main button.
    pub on_accent: crate::gfx::types::Color,
}

/// The screen a window is on, as numbers to lay out from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Screen {
    /// Width in points.
    pub width: f32,
    /// Height in points.
    pub height: f32,
    /// The width's class.
    pub width_class: Size,
    /// The height's class.
    pub height_class: Size,
    /// A finger rather than a mouse.
    pub touch: bool,
    /// Covered space at the top, right, bottom and left edges: the notch,
    /// the home bar, the button Krate draws to close an app on a phone.
    pub safe: (f32, f32, f32, f32),
    /// How the person asked things to look: dark mode, contrast, text size.
    pub look: Look,
}

impl Screen {
    /// A screen from its numbers, with the classes worked out the way
    /// `krate:ui/screen` does. For tests, and for laying out a size other
    /// than the window's.
    pub fn new(width: f32, height: f32, touch: bool, safe: (f32, f32, f32, f32)) -> Self {
        let class = |side: f32, compact: f32, large: f32| {
            if side < compact {
                Size::Compact
            } else if side >= large {
                Size::Large
            } else {
                Size::Medium
            }
        };
        Screen {
            width,
            height,
            width_class: class(width, 600.0, 1000.0),
            height_class: class(height, 500.0, 900.0),
            touch,
            safe,
            look: Look::default(),
        }
    }

    /// The screen of a window the app owns. Ask again on every `resized`
    /// event: a phone turns and a desktop window is narrowed.
    #[cfg(feature = "gui")]
    pub fn of(window: u64) -> Self {
        use crate::ui::screen::{facts, InputKind};
        let f = facts(window);
        Screen {
            look: Look::now(),
            ..Screen::new(
                f.width,
                f.height,
                matches!(f.input, InputKind::Touch),
                (f.safe.top, f.safe.right, f.safe.bottom, f.safe.left),
            )
        }
    }

    /// A phone held upright: one column.
    pub fn compact(&self) -> bool {
        self.width_class == Size::Compact
    }

    /// A phone turned sideways: little height, so keep headers short.
    pub fn short(&self) -> bool {
        self.height_class == Size::Compact
    }

    /// The whole window.
    pub fn area(&self) -> Rect {
        Rect::new(0.0, 0.0, self.width, self.height)
    }

    /// The window inside its covered edges: where text and controls go. A
    /// background may still fill `area()`.
    pub fn safe_area(&self) -> Rect {
        let (t, r, b, l) = self.safe;
        self.area().inset(t, r, b, l)
    }

    /// The smallest a control may be in each direction to be hit reliably:
    /// 44 points for a finger, 28 for a mouse.
    pub fn tap(&self) -> f32 {
        if self.touch {
            44.0
        } else {
            28.0
        }
    }

    /// Body text size: 17 points on a phone (the size a phone's own apps
    /// use), 15 elsewhere, at the person's text size. Headings scale from
    /// this, and nothing a person must read goes below 13.
    pub fn body(&self) -> f32 {
        let base = if self.compact() { 17.0 } else { 15.0 };
        self.look.text(base)
    }

    /// Split `area` for an app with places to go between (Library and
    /// Search, Inbox and Sent): a sidebar `sidebar_w` points wide when the
    /// screen is wide enough, otherwise a tab bar along the bottom of the
    /// screen, above the home bar. A sidebar takes a third of the width at
    /// most.
    pub fn nav(&self, area: Rect, sidebar_w: f32) -> Nav {
        let mut content = area;
        if self.compact() {
            // Down to the screen's bottom edge, the home bar inside it.
            let bottom_gap = self.height - area.bottom();
            let bar_h = 56.0 + (self.safe.2 - bottom_gap).max(0.0);
            let mut tabs = content.take_bottom(bar_h);
            tabs.h = (tabs.h - (self.safe.2 - bottom_gap).max(0.0)).max(0.0);
            Nav {
                sidebar: None,
                tabs: Some(tabs),
                content,
            }
        } else {
            let sidebar = content.take_left(sidebar_w.min(area.w / 3.0));
            Nav {
                sidebar: Some(sidebar),
                tabs: None,
                content,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// A text field an app draws itself (K-1109)
// ---------------------------------------------------------------------------

/// The text, caret and selection of one field an app draws on its canvas.
///
/// Feed it what the window sends: `Event::TextInput` to [`Field::insert`]
/// (a paste the person makes with Cmd+V or Ctrl+V arrives that way too, with
/// no clipboard permission) and `Event::Key` to [`Field::key_with`]. Draw
/// [`Field::selection`] behind the text and the caret at [`Field::caret`].
/// Shift with an arrow, Home or End selects; Cmd+A or Ctrl+A selects all;
/// typing, pasting, Backspace and Delete replace what is selected.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Field {
    pub text: alloc::string::String,
    /// The caret, a byte offset on a character boundary.
    pub caret: usize,
    /// Where a selection began; it runs from here to the caret.
    pub anchor: Option<usize>,
    /// Whether Enter makes a new line.
    pub multiline: bool,
}

impl Field {
    pub fn new(multiline: bool) -> Self {
        Field {
            multiline,
            ..Field::default()
        }
    }

    /// Put text in, the caret at its end, nothing selected.
    pub fn set(&mut self, text: &str) {
        self.text = alloc::string::String::from(text);
        self.caret = self.text.len();
        self.anchor = None;
    }

    /// Replace the text from elsewhere (a teammate's edit), keeping the
    /// caret and selection where they still fit.
    pub fn replace(&mut self, text: &str) {
        self.text = alloc::string::String::from(text);
        self.caret = self.fit(self.caret);
        self.anchor = self.anchor.map(|a| self.fit(a));
    }

    fn fit(&self, at: usize) -> usize {
        let mut at = at.min(self.text.len());
        while !self.text.is_char_boundary(at) {
            at -= 1;
        }
        at
    }

    /// What is selected, as `(start, end)` byte offsets; `None` when
    /// nothing is.
    pub fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        (anchor != self.caret).then(|| (anchor.min(self.caret), anchor.max(self.caret)))
    }

    /// The selected text, empty when nothing is selected.
    pub fn selected(&self) -> &str {
        self.selection().map_or("", |(a, b)| &self.text[a..b])
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.caret = self.text.len();
    }

    /// Move the caret to `at` (a tap, a drag, a line up or down). With
    /// `extend` the selection grows from where it began; without, nothing
    /// is selected.
    pub fn move_to(&mut self, at: usize, extend: bool) {
        let at = self.fit(at);
        if extend {
            self.anchor.get_or_insert(self.caret);
        } else {
            self.anchor = None;
        }
        self.caret = at;
    }

    fn cut_selection(&mut self) -> bool {
        let Some((a, b)) = self.selection() else {
            self.anchor = None;
            return false;
        };
        self.text.replace_range(a..b, "");
        self.caret = a;
        self.anchor = None;
        true
    }

    /// Type or paste text at the caret, over what is selected. Line breaks
    /// stay only in a multi-line field; tabs become spaces; other control
    /// characters are dropped. True when the text changed.
    pub fn insert(&mut self, typed: &str) -> bool {
        let mut clean = alloc::string::String::new();
        let mut chars = typed.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '\r' || ch == '\n' {
                if ch == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                clean.push(if self.multiline { '\n' } else { ' ' });
            } else if ch == '\t' {
                clean.push(' ');
            } else if !ch.is_control() {
                clean.push(ch);
            }
        }
        if clean.is_empty() {
            return false;
        }
        self.cut_selection();
        self.text.insert_str(self.caret, &clean);
        self.caret += clean.len();
        true
    }

    /// The character boundary before `at`.
    pub fn prev(&self, at: usize) -> usize {
        self.text[..at]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0)
    }

    /// The character boundary after `at`.
    pub fn next(&self, at: usize) -> usize {
        self.text[at..]
            .chars()
            .next()
            .map(|c| at + c.len_utf8())
            .unwrap_or(at)
    }

    /// The editing keys, without modifiers. `(changed, moved)`.
    pub fn key(&mut self, key: &str) -> (bool, bool) {
        self.key_with(key, false, false)
    }

    /// The editing keys every field shares: `shift` selects, `primary` is
    /// Cmd on a Mac and Ctrl elsewhere -- pass `m.meta || m.control`, since
    /// an app cannot tell which system it runs on. `(changed, moved)`: whether the
    /// text changed, and whether the caret or selection did.
    pub fn key_with(&mut self, key: &str, shift: bool, primary: bool) -> (bool, bool) {
        if primary {
            return match key {
                "a" | "A" => {
                    self.select_all();
                    (false, true)
                }
                _ => (false, false),
            };
        }
        match key {
            "Backspace" | "Delete" if self.selection().is_some() => (self.cut_selection(), true),
            "Backspace" if self.caret > 0 => {
                let from = self.prev(self.caret);
                self.text.replace_range(from..self.caret, "");
                self.caret = from;
                self.anchor = None;
                (true, true)
            }
            "Delete" if self.caret < self.text.len() => {
                let to = self.next(self.caret);
                self.text.replace_range(self.caret..to, "");
                self.anchor = None;
                (true, true)
            }
            "ArrowLeft" | "ArrowRight" => {
                let left = key == "ArrowLeft";
                let to = match (self.selection(), shift) {
                    // An arrow without Shift leaves a selection at its edge.
                    (Some((a, b)), false) => {
                        if left {
                            a
                        } else {
                            b
                        }
                    }
                    _ if left => self.prev(self.caret),
                    _ => self.next(self.caret),
                };
                self.move_to(to, shift);
                (false, true)
            }
            "Home" => {
                let to = if self.multiline {
                    self.text[..self.caret]
                        .rfind('\n')
                        .map(|i| i + 1)
                        .unwrap_or(0)
                } else {
                    0
                };
                self.move_to(to, shift);
                (false, true)
            }
            "End" => {
                let to = if self.multiline {
                    self.text[self.caret..]
                        .find('\n')
                        .map(|i| self.caret + i)
                        .unwrap_or(self.text.len())
                } else {
                    self.text.len()
                };
                self.move_to(to, shift);
                (false, true)
            }
            "Escape" if self.anchor.is_some() => {
                self.anchor = None;
                (false, true)
            }
            _ => (false, false),
        }
    }
}

/// The selected part of one drawn line, as byte offsets into the field's
/// text: `line` is the line's `(start, end)`, `selection` the field's.
/// `None` when the selection does not touch the line.
pub fn selected_on_line(
    line: (usize, usize),
    selection: Option<(usize, usize)>,
) -> Option<(usize, usize)> {
    let (s, e) = selection?;
    let (a, b) = line;
    let (from, to) = (s.max(a), e.min(b));
    (from < to || (s <= a && e > b)).then_some((from, to.max(from)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phone() -> Screen {
        Screen::new(390.0, 844.0, true, (89.0, 0.0, 34.0, 0.0))
    }

    fn desktop() -> Screen {
        Screen::new(1280.0, 800.0, false, (0.0, 0.0, 0.0, 0.0))
    }

    #[test]
    fn body_text_follows_the_persons_text_size() {
        assert_eq!(phone().body(), 17.0);
        let large = Screen {
            look: Look {
                text_scale: 1.35,
                ..Look::default()
            },
            ..phone()
        };
        assert!((large.body() - 22.95).abs() < 0.01, "{}", large.body());
        assert_eq!(Look::default().text(20.0), 20.0);
    }

    #[test]
    fn the_classes_name_a_phone_a_turned_phone_a_tablet_and_a_desktop() {
        assert!(phone().compact() && !phone().short());
        let turned = Screen::new(844.0, 390.0, true, (50.0, 47.0, 21.0, 47.0));
        assert!(!turned.compact() && turned.short());
        let tablet = Screen::new(820.0, 1180.0, true, (0.0, 0.0, 0.0, 0.0));
        assert_eq!(tablet.width_class, Size::Medium);
        assert_eq!(desktop().width_class, Size::Large);
    }

    #[test]
    fn bands_cut_from_a_rect_leave_the_rest() {
        let mut area = Rect::new(0.0, 0.0, 390.0, 844.0);
        let header = area.take_top(60.0);
        let footer = area.take_bottom(50.0);
        let side = area.take_left(100.0);
        assert_eq!(header, Rect::new(0.0, 0.0, 390.0, 60.0));
        assert_eq!(footer, Rect::new(0.0, 794.0, 390.0, 50.0));
        assert_eq!(side, Rect::new(0.0, 60.0, 100.0, 734.0));
        assert_eq!(area, Rect::new(100.0, 60.0, 290.0, 734.0));
        // Never more than there is.
        let all = area.take_top(10_000.0);
        assert_eq!(all.h, 734.0);
        assert_eq!(area.h, 0.0);
    }

    #[test]
    fn a_grid_fits_as_many_columns_as_the_width_takes() {
        let phone_cards = Rect::new(16.0, 0.0, 358.0, 0.0).grid(6, 160.0, 200.0, 12.0);
        assert_eq!(phone_cards[1].y, 0.0, "two across on a phone");
        assert_eq!(phone_cards[2].y, 212.0, "the third wraps");
        assert!(
            phone_cards.iter().all(|c| c.right() <= 374.01),
            "nothing past the edge"
        );
        let desk_cards = Rect::new(0.0, 0.0, 1100.0, 0.0).grid(6, 160.0, 200.0, 12.0);
        assert_eq!(desk_cards[5].y, 0.0, "six across on a desktop");
        let narrow = Rect::new(0.0, 0.0, 100.0, 0.0).grid(2, 160.0, 50.0, 8.0);
        assert_eq!(
            narrow[0].w, 100.0,
            "at least one column, shrunk to the width"
        );
    }

    #[test]
    fn the_sidebar_becomes_tabs_above_the_home_bar_on_a_phone() {
        let p = phone();
        let nav = p.nav(p.safe_area(), 240.0);
        assert!(nav.sidebar.is_none());
        let tabs = nav.tabs.unwrap();
        assert_eq!(tabs.h, 56.0);
        assert_eq!(tabs.bottom(), 844.0 - 34.0, "above the home bar");
        assert_eq!(nav.content.bottom(), tabs.y);
        assert_eq!(nav.content.w, 390.0);

        let d = desktop();
        let nav = d.nav(d.area(), 240.0);
        assert_eq!(nav.sidebar.unwrap().w, 240.0);
        assert!(nav.tabs.is_none());
        assert_eq!(nav.content.x, 240.0);
    }

    #[test]
    fn a_finger_gets_bigger_targets_and_a_phone_bigger_text() {
        assert_eq!(phone().tap(), 44.0);
        assert_eq!(desktop().tap(), 28.0);
        assert!(phone().body() > desktop().body());
    }

    #[test]
    fn the_safe_area_keeps_clear_of_the_notch_and_home_bar() {
        assert_eq!(phone().safe_area(), Rect::new(0.0, 89.0, 390.0, 721.0));
    }

    #[test]
    fn a_field_selects_and_typing_or_a_paste_replaces_the_selection() {
        let mut f = Field::new(false);
        f.set("Order chairs");
        // Shift+Left twice selects "rs"; typing replaces it.
        f.key_with("ArrowLeft", true, false);
        f.key_with("ArrowLeft", true, false);
        assert_eq!(f.selected(), "rs");
        assert!(f.insert("r"));
        assert_eq!(f.text, "Order chair");
        assert_eq!(f.selection(), None);
        // Select all, then a paste of two lines into a one-line field.
        assert_eq!(f.key_with("a", false, true), (false, true));
        assert_eq!(f.selected(), "Order chair");
        assert!(f.insert("Twelve\r\nstackable"));
        assert_eq!(f.text, "Twelve stackable");
        // Backspace removes a selection whole; an arrow leaves it at its edge.
        f.move_to(0, false);
        f.key_with("End", true, false);
        f.key_with("ArrowLeft", true, false);
        assert_eq!(f.selected(), "Twelve stackabl");
        assert_eq!(f.key_with("Backspace", false, false), (true, true));
        assert_eq!(f.text, "e");
        let mut g = Field::new(true);
        g.set("one\ntwo");
        g.move_to(1, false);
        g.move_to(5, true);
        g.key_with("ArrowRight", false, false);
        assert_eq!((g.caret, g.selection()), (5, None));
        g.insert("x\ny");
        assert_eq!(g.text, "one\ntx\nywo");
        // A teammate's shorter text keeps caret and selection inside it.
        g.select_all();
        g.replace("on");
        assert_eq!(g.selection(), Some((0, 2)));
        assert_eq!(selected_on_line((0, 3), Some((1, 6))), Some((1, 3)));
        assert_eq!(selected_on_line((4, 7), Some((1, 3))), None);
    }
}
