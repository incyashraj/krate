//! Ports errors.ts: `PeleError` and the syntax errors the parsers raise.
//!
//! Pele reports a syntax error's line and column in the source as the
//! author wrote it, mapping offsets back through every preprocessing edit
//! (`locatingIn`). A parser here locates an error in the text it read and
//! keeps the offset (`at`); `core` maps it back the same way and writes the
//! message again (`relocate`).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Ports `type PeleErrorCode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeleErrorCode {
    UnsupportedDiagram,
    Syntax,
    Semantic,
    Limit,
}

impl PeleErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            PeleErrorCode::UnsupportedDiagram => "unsupported-diagram",
            PeleErrorCode::Syntax => "syntax",
            PeleErrorCode::Semantic => "semantic",
            PeleErrorCode::Limit => "limit",
        }
    }
}

/// Ports `class PeleError`.
#[derive(Debug, Clone, PartialEq)]
pub struct PeleError {
    pub message: String,
    pub code: PeleErrorCode,
    pub kind: Option<&'static str>,
    pub mermaid_type: Option<&'static str>,
    pub line: u32,
    pub column: u32,
    pub snippet: String,
    /// A syntax error's offset in the text the parser read, whether it is
    /// lexical, and its detail, kept so it can be located in the source.
    pub(crate) at: Option<alloc::boxed::Box<(usize, bool, String)>>,
}

impl PeleError {
    pub fn new(message: impl Into<String>, code: PeleErrorCode) -> PeleError {
        PeleError {
            message: message.into(),
            code,
            kind: None,
            mermaid_type: None,
            line: 0,
            column: 0,
            snippet: String::new(),
            at: None,
        }
    }

    /// With `info.type` set.
    pub fn of(message: impl Into<String>, code: PeleErrorCode, kind: &'static str) -> PeleError {
        let mut e = PeleError::new(message, code);
        e.kind = Some(kind);
        e
    }
}

impl core::fmt::Display for PeleError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Ports `type Edit`: input range [from, to) replaced by `length` output
/// characters, all in UTF-16 units, as JS counts them.
pub type Edit = (usize, usize, usize);

/// Ports `stepInput`.
fn step_input(edits: &[Edit], at: usize) -> usize {
    let mut shift: isize = 0;
    let at = at as isize;
    for &(from, to, length) in edits {
        let (from, to, length) = (from as isize, to as isize, length as isize);
        let start = from - shift;
        if at < start {
            break;
        }
        if at < start + length {
            return (from + (at - start).min(to - from - 1)) as usize;
        }
        shift += to - from - length;
    }
    (at + shift).max(0) as usize
}

/// Ports `inputOffset`.
pub fn input_offset(steps: &[Vec<Edit>], mut at: usize) -> usize {
    for s in steps.iter().rev() {
        at = step_input(s, at);
    }
    at
}

/// Ports `locate`: the line, column and a snippet with a caret under the
/// offset (in bytes of `src`). `find` is `written`: where an offset in
/// `src` is in the source the author wrote, when `src` is the parsed text.
pub fn locate(
    src: &str,
    offset: usize,
    find: Option<&dyn Fn(usize) -> (String, usize)>,
) -> (usize, usize, String) {
    // Keep EOF errors on the last content line.
    let end = crate::pele::js::trim_end(src).len();
    let written = |at: usize| -> (String, usize) {
        match find {
            Some(f) => f(at),
            None => (String::from(src), at),
        }
    };
    let (src, offset) = if end > 0 && offset >= end {
        let last = src[..end]
            .chars()
            .next_back()
            .map(char::len_utf8)
            .unwrap_or(1);
        let (s, o) = written(end - last);
        let step = s[o.min(s.len())..]
            .chars()
            .next()
            .map(char::len_utf8)
            .unwrap_or(1);
        (s, o + step)
    } else {
        written(offset.min(src.len()))
    };
    let src = src.as_str();
    let offset = offset.min(src.len());
    let mut line = 1;
    let mut line_start = 0;
    for (i, c) in src.char_indices() {
        if i >= offset {
            break;
        }
        if c == '\n' {
            line += 1;
            line_start = i + 1;
        }
    }
    let line_end = src[offset..]
        .find('\n')
        .map(|k| offset + k)
        .unwrap_or(src.len());
    let column = crate::pele::js::len16(&src[line_start..offset]) + 1;
    let text = &src[line_start..line_end];
    (
        line,
        column,
        format!("{}\n{}^", text, "-".repeat(column - 1)),
    )
}

/// Ports `syntaxError`.
pub fn syntax_error(
    kind: &'static str,
    src: &str,
    offset: usize,
    detail: &str,
    lexical: bool,
) -> PeleError {
    let mut e = syntax_error_in(kind, src, offset, detail, lexical, None);
    e.at = Some(alloc::boxed::Box::new((
        offset,
        lexical,
        String::from(detail),
    )));
    e
}

/// `syntaxError` located through `find` (see [`locate`]).
pub fn syntax_error_in(
    kind: &'static str,
    src: &str,
    offset: usize,
    detail: &str,
    lexical: bool,
    find: Option<&dyn Fn(usize) -> (String, usize)>,
) -> PeleError {
    let (line, column, snippet) = locate(src, offset, find);
    let message = if lexical {
        format!(
            "Lexical error on line {}. Unrecognized text.\n{}",
            line, snippet
        )
    } else {
        format!("Parse error on line {}:\n{}\n{}", line, snippet, detail)
    };
    let mut e = PeleError::of(message, PeleErrorCode::Syntax, kind);
    e.line = line as u32;
    e.column = column as u32;
    e.snippet = snippet;
    e
}
