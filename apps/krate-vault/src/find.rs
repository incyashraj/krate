//! Find and replace in the open note: Cmd+F opens the bar over the note,
//! Cmd+Option+F opens it with the replace row. Matches are highlighted, the
//! current one is selected, Enter and Shift+Enter step through them,
//! Escape closes the bar and leaves the last match selected.
//!
//! Matching is case-insensitive, as the editor's own search is by default.
//! A 200,000-line note is searched once per change of the query or the
//! note, with the matches capped so a one-letter query stays quick.

use crate::app::{App, Input, Mods};
use crate::buffer::Pos;
use crate::editor::{Doc, Mode};
use crate::text::{find_ci, lower, match_len};
use alloc::string::String;
use alloc::vec::Vec;

/// The most matches kept; the bar says "10,000+" past it.
pub const MAX_MATCHES: usize = 10_000;

#[derive(Debug, Clone, Default)]
pub struct Find {
    pub query: Input,
    pub replace: Input,
    pub replacing: bool,
    /// Keys go to the replace field (else the find field).
    pub in_replace: bool,
    /// The bar has the keyboard (else the note does, with the bar open).
    pub focused: bool,
    /// Matches as (line, byte, length), in order.
    pub matches: Vec<(usize, usize, usize)>,
    pub current: Option<usize>,
    /// What the matches were found for: the query and the note's version.
    found_for: Option<(String, u64)>,
}

impl Find {
    pub fn new(query: &str, replacing: bool) -> Find {
        let mut f = Find {
            query: Input::with(query),
            replacing,
            focused: true,
            ..Find::default()
        };
        f.query.select_all();
        f
    }

    /// Find the matches again if the query or the note changed.
    pub fn refresh<'a>(
        &mut self,
        doc_lines: &dyn Fn(usize) -> &'a str,
        count: usize,
        version: u64,
    ) {
        let q = lower(&self.query.text);
        if self
            .found_for
            .as_ref()
            .is_some_and(|(fq, v)| *fq == q && *v == version)
        {
            return;
        }
        self.matches.clear();
        if !q.is_empty() {
            'all: for n in 0..count {
                let line = doc_lines(n);
                let mut from = 0;
                while let Some(at) = find_ci(line, &q, from) {
                    let len = match_len(line, at, &q).max(1);
                    self.matches.push((n, at, len));
                    if self.matches.len() >= MAX_MATCHES {
                        break 'all;
                    }
                    from = at + len;
                }
            }
        }
        self.found_for = Some((q, version));
        self.current = self.current.filter(|c| *c < self.matches.len());
    }

    /// The match at or after `p`, wrapping to the first.
    pub fn index_from(&self, p: Pos) -> Option<usize> {
        if self.matches.is_empty() {
            return None;
        }
        let i = self
            .matches
            .partition_point(|(l, c, _)| (*l, *c) < (p.line, p.col));
        Some(if i >= self.matches.len() { 0 } else { i })
    }

    /// "3 / 12", "No results", or "10,000+".
    pub fn label(&self) -> String {
        if self.query.text.is_empty() {
            return String::new();
        }
        if self.matches.is_empty() {
            return String::from("No results");
        }
        let mut s = String::new();
        if let Some(c) = self.current {
            crate::text::push_num(&mut s, c + 1);
            s.push_str(" / ");
        }
        s.push_str(&crate::text::grouped(self.matches.len()));
        if self.matches.len() >= MAX_MATCHES {
            s.push('+');
        }
        s
    }
}

fn refresh(doc: &mut Doc) {
    let count = doc.buf.line_count();
    let version = doc.buf.version;
    let Some(mut f) = doc.find.take() else {
        return;
    };
    {
        let buf = &doc.buf;
        f.refresh(&|n| buf.line(n), count, version);
    }
    doc.find = Some(f);
}

/// Select match `i` and show it.
fn select(doc: &mut Doc, i: usize) {
    let Some(&(l, c, len)) = doc.find.as_ref().and_then(|f| f.matches.get(i)) else {
        return;
    };
    if let Some(f) = doc.find.as_mut() {
        f.current = Some(i);
    }
    if doc.folded_away(l) {
        doc.jump_to(l);
    }
    doc.select_range(Pos::new(l, c), Pos::new(l, c + len));
}

/// Step to the next (or previous) match from the caret.
pub fn step(doc: &mut Doc, back: bool) {
    refresh(doc);
    let n = doc.find.as_ref().map_or(0, |f| f.matches.len());
    if n == 0 {
        return;
    }
    let cur = doc.find.as_ref().and_then(|f| f.current);
    let i = match (cur, back) {
        (Some(c), false) => (c + 1) % n,
        (Some(c), true) => (c + n - 1) % n,
        (None, _) => {
            let from = doc.selection().map_or(doc.cur, |(a, _)| a);
            let i = doc
                .find
                .as_ref()
                .and_then(|f| f.index_from(from))
                .unwrap_or(0);
            if back {
                (i + n - 1) % n
            } else {
                i
            }
        }
    };
    select(doc, i);
}

/// Replace the current match and step to the next.
pub fn replace_one(doc: &mut Doc, now: u64) {
    refresh(doc);
    let Some(f) = doc.find.as_ref() else {
        return;
    };
    let with = f.replace.text.clone();
    let Some(i) = f.current.or_else(|| f.index_from(doc.cur)) else {
        return;
    };
    let Some(&(l, c, len)) = f.matches.get(i) else {
        return;
    };
    doc.replace(Pos::new(l, c), Pos::new(l, c + len), &with, now);
    doc.cur = Pos::new(l, c + with.len());
    doc.anchor = None;
    if let Some(f) = doc.find.as_mut() {
        f.current = None;
    }
    refresh(doc);
    let next = doc
        .find
        .as_ref()
        .and_then(|f| f.index_from(Pos::new(l, c + with.len())));
    if let Some(n) = next {
        select(doc, n);
    }
}

/// Replace every match, as one undo.
pub fn replace_all(doc: &mut Doc, now: u64) -> usize {
    refresh(doc);
    let Some(f) = doc.find.as_ref() else {
        return 0;
    };
    let with = f.replace.text.clone();
    let all = f.matches.clone();
    if all.is_empty() {
        return 0;
    }
    doc.buf.begin_group(doc.cur);
    for (l, c, len) in all.iter().rev() {
        doc.replace(Pos::new(*l, *c), Pos::new(*l, c + len), &with, now);
    }
    doc.buf.end_group(doc.cur);
    doc.anchor = None;
    doc.cur = doc.buf.clamp(doc.cur);
    if let Some(f) = doc.find.as_mut() {
        f.current = None;
    }
    refresh(doc);
    all.len()
}

/// Open the bar (with the replace row when `replacing`), seeded with the
/// selection.
pub fn open(doc: &mut Doc, replacing: bool) {
    let seed = doc.selected_text();
    let seed = if seed.contains('\n') {
        String::new()
    } else {
        seed
    };
    match doc.find.as_mut() {
        Some(f) => {
            if !seed.is_empty() {
                f.query = Input::with(&seed);
            }
            f.query.select_all();
            f.replacing |= replacing;
            f.in_replace = false;
            f.focused = true;
        }
        None => {
            let prev = seed;
            doc.find = Some(Find::new(&prev, replacing));
        }
    }
    refresh(doc);
    if let Some(f) = doc.find.as_mut() {
        f.current = None;
    }
}

pub fn close(doc: &mut Doc) {
    doc.find = None;
}

/// Keys, before the app's own: Cmd+F and Cmd+Option+F anywhere in a note,
/// and everything while the bar has the keyboard. `None` lets the app
/// handle the key.
pub fn key(app: &mut App, key: &str, m: Mods) -> Option<bool> {
    let now = app.now;
    let doc = app.doc_mut()?;
    let k = lower(key);
    // With Option held a Mac sends the letter Option makes: "ƒ".
    if m.primary && (k == "f" || k == "ƒ") && !m.shift {
        open(doc, m.alt && doc.mode != Mode::Reading);
        return Some(true);
    }
    let focused = doc.find.as_ref().is_some_and(|f| f.focused);
    if !focused {
        if key == "Escape" && doc.find.is_some() && !doc.has_extra() && doc.anchor.is_none() {
            close(doc);
            return Some(true);
        }
        // F3 and Cmd+G step without the keyboard in the bar.
        if doc.find.is_some() && (key == "F3" || (m.primary && k == "g")) {
            step(doc, m.shift);
            return Some(true);
        }
        return None;
    }
    let replacing = doc.find.as_ref().is_some_and(|f| f.replacing);
    let in_replace = doc.find.as_ref().is_some_and(|f| f.in_replace);
    match key {
        "Escape" => {
            close(doc);
        }
        "Enter" if in_replace && m.primary && m.alt => {
            replace_all(doc, now);
        }
        "Enter" if in_replace => replace_one(doc, now),
        "Enter" => step(doc, m.shift),
        "Tab" if replacing => {
            if let Some(f) = doc.find.as_mut() {
                f.in_replace = !f.in_replace;
            }
        }
        "ArrowDown" | "ArrowUp" if !in_replace => step(doc, key == "ArrowUp"),
        _ => {
            if m.primary && k == "a" {
                if let Some(f) = doc.find.as_mut() {
                    if f.in_replace {
                        f.replace.select_all();
                    } else {
                        f.query.select_all();
                    }
                }
                return Some(true);
            }
            if m.primary && k == "g" {
                step(doc, m.shift);
                return Some(true);
            }
            if m.primary {
                // Other chords (Cmd+S, Cmd+W, Cmd+Z) still reach the app.
                return None;
            }
            let changed = doc.find.as_mut().is_some_and(|f| {
                if f.in_replace {
                    f.replace.key(key, m);
                    false
                } else {
                    f.query.key(key, m)
                }
            });
            if changed {
                after_query_edit(doc);
            }
        }
    }
    Some(true)
}

fn after_query_edit(doc: &mut Doc) {
    if let Some(f) = doc.find.as_mut() {
        f.current = None;
    }
    refresh(doc);
    // The first match at or after the caret is selected as the query grows.
    let from = doc.selection().map_or(doc.cur, |(a, _)| a);
    if let Some(i) = doc.find.as_ref().and_then(|f| f.index_from(from)) {
        select(doc, i);
    }
}

/// Typed text, before the note gets it: it goes to the bar while the bar
/// has the keyboard. True when the bar took it.
pub fn text(app: &mut App, s: &str) -> bool {
    let Some(doc) = app.doc_mut() else {
        return false;
    };
    let Some(f) = doc.find.as_mut().filter(|f| f.focused) else {
        return false;
    };
    if f.in_replace {
        f.replace.insert(s);
        return true;
    }
    f.query.insert(s);
    after_query_edit(doc);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(t: &str) -> Doc {
        Doc::new("n.md", "n", t)
    }

    #[test]
    fn find_steps_through_matches_and_wraps() {
        let mut d = doc("Apple pie\nbanana APPLE\napple");
        open(&mut d, false);
        for c in "apple".chars() {
            let mut b = [0u8; 4];
            let f = d.find.as_mut().unwrap();
            f.query.insert(c.encode_utf8(&mut b));
            after_query_edit(&mut d);
        }
        let f = d.find.as_ref().unwrap();
        assert_eq!(f.matches, [(0, 0, 5), (1, 7, 5), (2, 0, 5)]);
        assert_eq!(d.selected_text(), "Apple");
        step(&mut d, false);
        assert_eq!(d.selection().unwrap().0, Pos::new(1, 7));
        assert_eq!(d.find.as_ref().unwrap().label(), "2 / 3");
        step(&mut d, false);
        step(&mut d, false);
        assert_eq!(
            d.selection().unwrap().0,
            Pos::new(0, 0),
            "wraps to the first"
        );
        step(&mut d, true);
        assert_eq!(
            d.selection().unwrap().0,
            Pos::new(2, 0),
            "and back to the last"
        );
    }

    #[test]
    fn replace_one_and_all_follow_the_note() {
        let mut d = doc("cat cat\ndog cat");
        open(&mut d, true);
        d.find.as_mut().unwrap().query = Input::with("cat");
        d.find.as_mut().unwrap().replace = Input::with("lion");
        step(&mut d, false);
        replace_one(&mut d, 0);
        assert_eq!(d.text(), "lion cat\ndog cat");
        assert_eq!(
            d.selection().unwrap().0,
            Pos::new(0, 5),
            "the next one is selected"
        );
        assert_eq!(replace_all(&mut d, 0), 2);
        assert_eq!(d.text(), "lion lion\ndog lion");
        assert_eq!(d.find.as_ref().unwrap().label(), "No results");
        d.undo();
        assert_eq!(d.text(), "lion cat\ndog cat", "replace all is one undo");
    }

    #[test]
    fn a_huge_note_caps_the_matches() {
        let mut t = String::new();
        for _ in 0..12_000 {
            t.push_str("a\n");
        }
        let mut d = doc(&t);
        open(&mut d, false);
        d.find.as_mut().unwrap().query = Input::with("a");
        refresh(&mut d);
        let f = d.find.as_ref().unwrap();
        assert_eq!(f.matches.len(), MAX_MATCHES);
        assert_eq!(f.label(), "10,000+");
    }
}
