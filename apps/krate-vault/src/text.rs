//! Small text helpers that every part of the app shares, none of which
//! allocate more than the answer they return.

use alloc::string::String;

/// A number as text, without `format!`.
pub fn num(n: usize) -> String {
    let mut s = String::new();
    push_num(&mut s, n);
    s
}

/// Append a number's digits.
pub fn push_num(out: &mut String, mut n: usize) {
    if n == 0 {
        out.push('0');
        return;
    }
    let mut digits = [0u8; 20];
    let mut k = 0;
    while n > 0 && k < digits.len() {
        if let Some(d) = digits.get_mut(k) {
            *d = b'0' + (n % 10) as u8;
        }
        n /= 10;
        k += 1;
    }
    while k > 0 {
        k -= 1;
        out.push(char::from(digits.get(k).copied().unwrap_or(b'0')));
    }
}

/// A number with thousands separators: `10,000`.
pub fn grouped(n: usize) -> String {
    let plain = num(n);
    let mut out = String::new();
    let len = plain.len();
    for (i, c) in plain.chars().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `n` followed by the singular or plural word: "1 word", "3 words".
pub fn count(n: usize, one: &str, many: &str) -> String {
    let mut s = grouped(n);
    s.push(' ');
    s.push_str(if n == 1 { one } else { many });
    s
}

/// Words in a line, the way a word counter means it: runs of characters
/// that are not whitespace and hold at least one letter or digit.
pub fn words_in(line: &str) -> usize {
    line.split_whitespace()
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .count()
}

/// Characters in a line (not bytes).
pub fn chars_in(line: &str) -> usize {
    line.chars().count()
}

/// The leading whitespace of a line.
pub fn indent_of(line: &str) -> &str {
    let end = line.len() - line.trim_start().len();
    line.get(..end).unwrap_or("")
}

/// Lower-case a string for comparing names.
pub fn lower(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii() {
            out.push(c.to_ascii_lowercase());
        } else {
            for l in c.to_lowercase() {
                out.push(l);
            }
        }
    }
    out
}

fn fold(c: char) -> char {
    if c.is_ascii() {
        c.to_ascii_lowercase()
    } else {
        c.to_lowercase().next().unwrap_or(c)
    }
}

/// Fuzzy subsequence score of `query` against `text`, case-insensitive:
/// `None` when the query's characters do not all appear in order. Higher is
/// better -- runs of consecutive characters and characters at the start of a
/// word count more, and a shorter text wins a tie. Spaces in the query are
/// ignored, so "dly nts" finds "Daily notes". It allocates nothing, because
/// the quick switcher runs it over every note in the vault on each key.
pub fn fuzzy(query: &str, text: &str) -> Option<i32> {
    let mut score = 0i32;
    let mut it = text.char_indices().peekable();
    let mut prev: Option<char> = None;
    let mut last_hit: Option<usize> = None;
    let mut index = 0usize;
    let mut any = false;
    for q in query.chars() {
        if q == ' ' {
            continue;
        }
        any = true;
        let q = fold(q);
        let mut found = false;
        for (_, c) in it.by_ref() {
            let here = index;
            index += 1;
            let before = prev;
            prev = Some(c);
            if fold(c) == q {
                score += 1;
                if last_hit.is_some_and(|l| l + 1 == here) {
                    score += 5;
                }
                let start = match before {
                    None => true,
                    Some(b) => matches!(b, ' ' | '/' | '_' | '-' | '.' | '(' | '['),
                };
                if start {
                    score += 4;
                }
                last_hit = Some(here);
                found = true;
                break;
            }
        }
        if !found {
            return None;
        }
    }
    if !any {
        return Some(0);
    }
    let len = text.chars().count().min(240) as i32;
    Some(score * 16 - len / 6)
}

/// Byte offset of the first case-insensitive occurrence of `needle` (already
/// lower-case) in `hay`, at or after `from`. ASCII is compared without
/// allocating; anything else falls back to comparing folded characters.
pub fn find_ci(hay: &str, needle: &str, from: usize) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    let h = hay.as_bytes();
    let n = needle.as_bytes();
    if needle.is_ascii() {
        if n.len() > h.len() {
            return None;
        }
        let first = n.first().copied().unwrap_or(0);
        let mut i = from;
        while i + n.len() <= h.len() {
            let b = h.get(i).copied().unwrap_or(0).to_ascii_lowercase();
            if b == first {
                let mut ok = true;
                for (k, nb) in n.iter().enumerate().skip(1) {
                    if h.get(i + k).map(u8::to_ascii_lowercase) != Some(*nb) {
                        ok = false;
                        break;
                    }
                }
                if ok && hay.is_char_boundary(i) {
                    return Some(i);
                }
            }
            i += 1;
        }
        return None;
    }
    // Non-ASCII needle: walk character starts and compare folded chars.
    let rest = hay.get(from..)?;
    for (off, _) in rest.char_indices() {
        let mut hc = rest.get(off..)?.chars();
        let mut matched = true;
        for nc in needle.chars() {
            match hc.next() {
                Some(c) if fold(c) == nc => {}
                _ => {
                    matched = false;
                    break;
                }
            }
        }
        if matched {
            return Some(from + off);
        }
    }
    None
}

/// The byte length of the match `find_ci` found at `at`: the needle's length
/// in the haystack's own characters (they can differ outside ASCII).
pub fn match_len(hay: &str, at: usize, needle: &str) -> usize {
    let n = needle.chars().count();
    hay.get(at..)
        .map(|rest| {
            rest.char_indices()
                .nth(n)
                .map(|(i, _)| i)
                .unwrap_or(rest.len())
        })
        .unwrap_or(0)
}

/// A one-line excerpt of `line` of at most about `room` characters, kept
/// around the byte range `hit` so the match is always in it. Returns the
/// excerpt and the hit's range inside it.
pub fn excerpt(line: &str, hit: (usize, usize), room: usize) -> (String, (usize, usize)) {
    let line = line.trim_end();
    let (a, b) = (hit.0.min(line.len()), hit.1.min(line.len()));
    if line.chars().count() <= room {
        return (String::from(line), (a, b));
    }
    // Start a little before the hit, on a character boundary.
    let lead = room / 3;
    let mut start = a;
    let mut taken = 0;
    while start > 0 && taken < lead {
        start -= 1;
        while start > 0 && !line.is_char_boundary(start) {
            start -= 1;
        }
        taken += 1;
    }
    let mut end = start;
    for (i, c) in line.get(start..).unwrap_or("").char_indices().take(room) {
        end = start + i + c.len_utf8();
    }
    let end = end.max(b.min(line.len()));
    let mut out = String::new();
    let mut shift = start;
    if start > 0 {
        out.push('…');
        shift = start.saturating_sub('…'.len_utf8());
    }
    out.push_str(line.get(start..end).unwrap_or(""));
    if end < line.len() {
        out.push('…');
    }
    (out, (a - shift, b - shift))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_group_by_thousands() {
        assert_eq!(num(0), "0");
        assert_eq!(num(1234567), "1234567");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(10000), "10,000");
        assert_eq!(grouped(1234567), "1,234,567");
        assert_eq!(count(1, "word", "words"), "1 word");
        assert_eq!(count(2000, "word", "words"), "2,000 words");
    }

    #[test]
    fn words_skip_punctuation_runs() {
        assert_eq!(words_in("Hello, world - this is #tag"), 5);
        assert_eq!(words_in("   "), 0);
        assert_eq!(chars_in("héllo"), 5);
    }

    #[test]
    fn fuzzy_prefers_word_starts_runs_and_short_names() {
        assert!(fuzzy("dly", "Daily notes").is_some());
        assert!(fuzzy("xyz", "Daily notes").is_none());
        assert_eq!(fuzzy("", "anything"), Some(0));
        let exact = fuzzy("meeting", "Meeting notes").unwrap_or(-1);
        let spread = fuzzy("meeting", "my extra elephant tin gang").unwrap_or(-1);
        assert!(exact > spread);
        let short = fuzzy("plan", "Plan").unwrap_or(-1);
        let long = fuzzy("plan", "Plan for the very long quarter review").unwrap_or(-1);
        assert!(short > long);
        assert!(fuzzy("É", "école").is_some());
    }

    #[test]
    fn find_ci_matches_any_case_and_unicode() {
        assert_eq!(find_ci("Hello World", "world", 0), Some(6));
        assert_eq!(find_ci("Hello World", "world", 7), None);
        assert_eq!(find_ci("aaa", "aa", 1), Some(1));
        assert_eq!(find_ci("Ünïcode ÜBER", "über", 0), Some(10));
        assert_eq!(find_ci("short", "much longer", 0), None);
        assert_eq!(match_len("Ünïcode ÜBER", 10, "über"), "ÜBER".len());
    }

    #[test]
    fn excerpt_keeps_the_hit_in_view() {
        let line = "a".repeat(200) + "NEEDLE" + &"b".repeat(200);
        let (ex, (a, b)) = excerpt(&line, (200, 206), 60);
        assert_eq!(ex.get(a..b), Some("NEEDLE"));
        assert!(ex.starts_with('…') && ex.ends_with('…'));
        let (short, hit) = excerpt("find me here", (5, 7), 60);
        assert_eq!(short, "find me here");
        assert_eq!(hit, (5, 7));
    }
}
