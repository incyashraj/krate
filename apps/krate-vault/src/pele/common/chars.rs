//! Ports util/chars.ts (the character tests Pele's hand-written lexers
//! make on `charCodeAt` codes), and [`Text`]: a source read one character
//! at a time, as those lexers read it.
//!
//! Positions in a [`Text`] are character indices. JS walks UTF-16 code
//! units; every test the lexers make is on a character of the Basic
//! Multilingual Plane, so a character beyond it, which JS sees as two
//! surrogates that no test accepts, is one character no test accepts here.
//! Byte offsets (for errors, which locate in bytes) come from [`Text::byte`].

use alloc::string::String;
use alloc::vec::Vec;

/// No character: JS's NaN past the end of a string, which no test accepts.
pub const NONE: u32 = u32::MAX;

/// Ports `isSpace`: whitespace as `\s` sees it.
pub fn is_space(c: u32) -> bool {
    if c < 128 {
        return c == 32 || (9..=13).contains(&c);
    }
    c == 0xa0
        || c == 0x1680
        || (0x2000..=0x200a).contains(&c)
        || c == 0x2028
        || c == 0x2029
        || c == 0x202f
        || c == 0x205f
        || c == 0x3000
        || c == 0xfeff
}

/// Ports `isBlank`.
pub fn is_blank(c: u32) -> bool {
    c == 32 || c == 9
}

/// Ports `isLineEnd`: a line terminator, what `.` does not match.
pub fn is_line_end(c: u32) -> bool {
    c == 10 || c == 13 || c == 0x2028 || c == 0x2029
}

/// Ports `isDigit`.
pub fn is_digit(c: u32) -> bool {
    (48..=57).contains(&c)
}

/// Ports `isLetter`.
pub fn is_letter(c: u32) -> bool {
    (65..=90).contains(&c) || (97..=122).contains(&c)
}

/// Ports `isHex`.
pub fn is_hex(c: u32) -> bool {
    (48..=57).contains(&c) || (65..=70).contains(&c) || (97..=102).contains(&c)
}

/// Ports `isWord`: a character of `\w`.
pub fn is_word(c: u32) -> bool {
    (48..=57).contains(&c) || (65..=90).contains(&c) || (97..=122).contains(&c) || c == 95
}

/// A source as a lexer reads it: its characters, and where each starts.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Text {
    pub src: String,
    pub ch: Vec<u32>,
    /// Byte offset of each character, and the length at the end.
    pub off: Vec<usize>,
}

impl Text {
    pub fn new(src: &str) -> Text {
        let mut ch = Vec::with_capacity(src.len());
        let mut off = Vec::with_capacity(src.len() + 1);
        for (i, c) in src.char_indices() {
            ch.push(c as u32);
            off.push(i);
        }
        off.push(src.len());
        Text {
            src: src.into(),
            ch,
            off,
        }
    }

    /// The length in characters (`src.length`).
    pub fn len(&self) -> usize {
        self.ch.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ch.is_empty()
    }

    /// `src.charCodeAt(i)`: [`NONE`] past the end.
    pub fn c(&self, i: usize) -> u32 {
        self.ch.get(i).copied().unwrap_or(NONE)
    }

    /// The byte offset of character `i` (the length past the end).
    pub fn byte(&self, i: usize) -> usize {
        self.off[i.min(self.ch.len())]
    }

    /// The character index of a byte offset that starts a character.
    pub fn index(&self, byte: usize) -> usize {
        match self.off.binary_search(&byte) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        }
    }

    /// `src.slice(s, e)`, clamped as JS clamps.
    pub fn slice(&self, s: usize, e: usize) -> &str {
        let n = self.ch.len();
        let (s, e) = (s.min(n), e.min(n));
        if s >= e {
            return "";
        }
        &self.src[self.off[s]..self.off[e]]
    }

    /// `src.startsWith(word, p)`.
    pub fn starts_with(&self, word: &str, p: usize) -> bool {
        p <= self.ch.len() && self.src[self.off[p]..].starts_with(word)
    }

    /// `src.indexOf(needle, from)` as a character index.
    pub fn index_of(&self, needle: &str, from: usize) -> Option<usize> {
        if from > self.ch.len() {
            return if needle.is_empty() {
                Some(self.ch.len())
            } else {
                None
            };
        }
        let b = self.off[from];
        self.src[b..].find(needle).map(|k| self.index(b + k))
    }

    /// `src.indexOf(ch, from)` for one character, as a character index.
    pub fn find_char(&self, c: u32, from: usize) -> Option<usize> {
        (from..self.ch.len()).find(|&i| self.ch[i] == c)
    }

    /// `src.lastIndexOf(ch, from)` for one character.
    pub fn last_char(&self, c: u32, from: usize) -> Option<usize> {
        if self.ch.is_empty() {
            return None;
        }
        let top = from.min(self.ch.len() - 1);
        (0..=top).rev().find(|&i| self.ch[i] == c)
    }

    /// Where the line that `from` is on ends: the next `\n`, or the end.
    pub fn line_end(&self, from: usize) -> usize {
        self.find_char(10, from).unwrap_or(self.ch.len())
    }
}
