//! Small text helpers the other modules share: character classes for word
//! motions, and the mapping between byte columns and the columns a person
//! sees on screen (tabs take several).

use alloc::string::String;

/// The three kinds of character word motions care about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Blank,
    Punct,
    Word,
}

pub fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

pub fn class(c: Option<char>) -> Class {
    match c {
        None => Class::Blank,
        Some(c) if c.is_whitespace() => Class::Blank,
        Some(c) if is_word_char(c) => Class::Word,
        Some(_) => Class::Punct,
    }
}

/// For WORD motions (`W`, `B`, `E`): anything that is not blank is one word.
pub fn big_class(c: Option<char>) -> Class {
    match class(c) {
        Class::Blank => Class::Blank,
        _ => Class::Word,
    }
}

/// The screen column of byte `col` in `line`, with tabs to `tab` stops.
pub fn display_col(line: &str, col: usize, tab: usize) -> usize {
    let tab = tab.max(1);
    let mut d = 0;
    for (i, c) in line.char_indices() {
        if i >= col {
            break;
        }
        d = advance(d, c, tab);
    }
    d
}

fn advance(d: usize, c: char, tab: usize) -> usize {
    if c == '\t' {
        (d / tab + 1) * tab
    } else {
        d + 1
    }
}

/// The byte column whose screen column is at or just before `want`.
pub fn col_for_display(line: &str, want: usize, tab: usize) -> usize {
    let tab = tab.max(1);
    let mut d = 0;
    for (i, c) in line.char_indices() {
        let next = advance(d, c, tab);
        if next > want {
            return i;
        }
        d = next;
    }
    line.len()
}

/// The byte column nearest a screen column, rounding to the closer edge --
/// what a mouse click wants.
pub fn col_for_display_nearest(line: &str, want_x: f32, tab: usize) -> usize {
    let tab = tab.max(1);
    let mut d = 0usize;
    for (i, c) in line.char_indices() {
        let next = advance(d, c, tab);
        let mid = (d as f32 + next as f32) * 0.5;
        if want_x < mid {
            return i;
        }
        d = next;
    }
    line.len()
}

/// How many screen columns the whole line takes.
pub fn display_width(line: &str, tab: usize) -> usize {
    display_col(line, line.len(), tab)
}

/// `line` with tabs turned into spaces, for drawing on a grid.
pub fn expand_tabs(line: &str, tab: usize) -> String {
    if !line.contains('\t') {
        return String::from(line);
    }
    let tab = tab.max(1);
    let mut out = String::new();
    let mut d = 0;
    for c in line.chars() {
        if c == '\t' {
            let next = (d / tab + 1) * tab;
            while d < next {
                out.push(' ');
                d += 1;
            }
        } else {
            out.push(c);
            d += 1;
        }
    }
    out
}

/// A number as text, without `format!`.
pub fn num(mut n: usize) -> String {
    if n == 0 {
        return String::from("0");
    }
    let mut digits = [0u8; 20];
    let mut k = 0;
    while n > 0 && k < digits.len() {
        digits[k] = b'0' + (n % 10) as u8;
        n /= 10;
        k += 1;
    }
    let mut out = String::new();
    while k > 0 {
        k -= 1;
        out.push(digits[k] as char);
    }
    out
}

/// The leading whitespace of a line.
pub fn indent_of(line: &str) -> &str {
    let end = line.len() - line.trim_start().len();
    line.get(..end).unwrap_or("")
}

/// Lowercase ASCII fuzzy subsequence score: `None` when `query`'s letters do
/// not all appear in order in `text`. Higher is better: consecutive letters
/// and letters at word starts score more.
pub fn fuzzy_score(query: &str, text: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let t: alloc::vec::Vec<char> = text.chars().collect();
    let mut score = 0i32;
    let mut ti = 0usize;
    let mut last: Option<usize> = None;
    for q in query.chars() {
        let q = q.to_ascii_lowercase();
        if q == ' ' {
            continue;
        }
        let mut found = None;
        while ti < t.len() {
            let c = t[ti].to_ascii_lowercase();
            if c == q {
                found = Some(ti);
                ti += 1;
                break;
            }
            ti += 1;
        }
        let at = found?;
        score += 1;
        if last.is_some_and(|l| l + 1 == at) {
            score += 4;
        }
        let start = at == 0 || matches!(t.get(at - 1), Some(' ' | '/' | '_' | '-' | '.' | ':'));
        if start {
            score += 3;
        }
        last = Some(at);
    }
    // Shorter texts win ties.
    Some(score * 16 - (t.len().min(200) as i32) / 8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_columns_expand_tabs_to_stops() {
        let line = "\tab\tc";
        assert_eq!(display_col(line, 0, 4), 0);
        assert_eq!(display_col(line, 1, 4), 4);
        assert_eq!(display_col(line, 3, 4), 6);
        assert_eq!(display_col(line, 4, 4), 8);
        assert_eq!(col_for_display(line, 5, 4), 2);
        assert_eq!(col_for_display(line, 4, 4), 1);
        assert_eq!(col_for_display(line, 2, 4), 0);
        assert_eq!(expand_tabs(line, 4), "    ab  c");
        assert_eq!(display_width("héllo", 4), 5);
    }

    #[test]
    fn nearest_column_rounds_to_the_closer_edge() {
        assert_eq!(col_for_display_nearest("abc", 0.4, 4), 0);
        assert_eq!(col_for_display_nearest("abc", 0.6, 4), 1);
        assert_eq!(col_for_display_nearest("abc", 9.0, 4), 3);
    }

    #[test]
    fn numbers_and_indents() {
        assert_eq!(num(0), "0");
        assert_eq!(num(1234567), "1234567");
        assert_eq!(indent_of("    let x"), "    ");
        assert_eq!(indent_of("x"), "");
    }

    #[test]
    fn fuzzy_prefers_word_starts_and_runs() {
        assert!(fuzzy_score("sv", "Save file").is_some());
        assert!(fuzzy_score("xyz", "Save file").is_none());
        let a = fuzzy_score("split", "Split vertically").unwrap();
        let b = fuzzy_score("split", "Show palette list items too").unwrap_or(-999);
        assert!(a > b);
        let main = fuzzy_score("main", "src/main.rs").unwrap();
        let other = fuzzy_score("main", "src/my_animation.rs").unwrap();
        assert!(main > other);
    }
}
