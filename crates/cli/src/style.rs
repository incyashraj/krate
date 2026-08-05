//! Terminal styling: colour, weight, and the small set of glyphs the interface
//! draws itself with.
//!
//! Two rules shape everything here.
//!
//! **Colour is never the only signal.** A ready tool reads "ready" whether or
//! not the terminal renders green, because roughly one in twelve men cannot
//! reliably separate red from green and because a great many terminals are
//! still monochrome. Colour emphasises what the words already say.
//!
//! **Everything degrades rather than breaks.** `NO_COLOR` is honoured, a pipe
//! gets no escape codes at all, and a terminal that cannot show Unicode gets
//! ASCII glyphs. The interface should look considered on a 2026 laptop and
//! stay legible over SSH to a decade-old box.

use std::io::{self, IsTerminal};
use std::sync::OnceLock;

/// Whether to emit ANSI escapes at all.
fn colour_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        // The no-color.org convention: any value, even empty, means off.
        if std::env::var_os("NO_COLOR").is_some() {
            return false;
        }
        if std::env::var("TERM")
            .map(|term| term == "dumb")
            .unwrap_or(false)
        {
            return false;
        }
        io::stdout().is_terminal()
    })
}

/// Whether the terminal can be trusted with the box-drawing and symbol glyphs.
fn unicode_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        // Every modern terminal sets one of these to a UTF-8 locale. When none
        // says so, assume it cannot and use ASCII: a wrong guess towards plain
        // costs nothing, a wrong guess towards Unicode prints mojibake.
        ["LC_ALL", "LC_CTYPE", "LANG"]
            .iter()
            .filter_map(|key| std::env::var(key).ok())
            .any(|value| {
                let value = value.to_ascii_lowercase();
                value.contains("utf-8") || value.contains("utf8")
            })
    })
}

fn wrap(code: &str, text: &str) -> String {
    if colour_enabled() {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

/// The product's accent. Used for the wordmark and the current selection only,
/// so it keeps meaning something.
pub fn accent(text: &str) -> String {
    wrap("38;5;111", text)
}

/// Secondary text: hints, paths, units. Present but out of the way.
pub fn dim(text: &str) -> String {
    wrap("2", text)
}

pub fn bold(text: &str) -> String {
    wrap("1", text)
}

/// Something that worked.
pub fn good(text: &str) -> String {
    wrap("38;5;114", text)
}

/// Something that needs attention. Deliberately amber rather than red: an AI
/// that is merely not signed in has not failed, and colouring it like an error
/// makes the whole screen read as broken.
pub fn warn(text: &str) -> String {
    wrap("38;5;179", text)
}

/// Something that genuinely failed.
pub fn bad(text: &str) -> String {
    wrap("38;5;174", text)
}

/// A number or key the person is meant to type.
pub fn key(text: &str) -> String {
    if colour_enabled() {
        format!("\x1b[1;38;5;111m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

pub struct Glyphs {
    pub tick: &'static str,
    pub cross: &'static str,
    pub dot: &'static str,
    pub arrow: &'static str,
    pub rule: &'static str,
    pub spinner: &'static [&'static str],
}

pub fn glyphs() -> Glyphs {
    if unicode_enabled() {
        Glyphs {
            tick: "✓",
            cross: "✗",
            dot: "•",
            arrow: "→",
            rule: "─",
            // Braille spinner: smooth, and one cell wide in every font that
            // has it, so the line does not jitter as it turns.
            spinner: &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"],
        }
    } else {
        Glyphs {
            tick: "+",
            cross: "x",
            dot: "*",
            arrow: "->",
            rule: "-",
            spinner: &["|", "/", "-", "\\"],
        }
    }
}

/// A horizontal rule, sized to the terminal but never sprawling.
pub fn rule(width: usize) -> String {
    dim(&glyphs().rule.repeat(width))
}

/// Terminal width, clamped to something a person can actually read across.
pub fn content_width() -> usize {
    terminal_width().clamp(40, 72)
}

fn terminal_width() -> usize {
    // COLUMNS is what a shell exports; falling back to 80 is the century-old
    // safe default and is never wrong enough to break a layout.
    std::env::var("COLUMNS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(80)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styling_is_absent_when_colour_is_off() {
        // The helpers are pure string wrapping; with colour disabled in a test
        // process (no tty) they must return the text untouched, so nothing
        // leaks escape codes into a pipe or a log.
        assert_eq!(dim("hello"), "hello");
        assert_eq!(bold("hello"), "hello");
        assert_eq!(key("1"), "1");
    }

    #[test]
    fn a_rule_is_as_wide_as_asked() {
        assert_eq!(rule(10).chars().count(), 10);
    }

    #[test]
    fn content_width_stays_readable_at_both_extremes() {
        // Whatever the terminal claims, text should not run to 300 columns nor
        // squeeze below readability.
        let width = content_width();
        assert!((40..=72).contains(&width));
    }
}
